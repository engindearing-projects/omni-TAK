//! Message Aggregator
//!
//! Collects CoT messages from all sources, deduplicates by UID
//! with time-based deduplication window, and forwards unique messages
//! to the distributor.

use anyhow::Result;
use dashmap::DashMap;
use flume::{Receiver, Sender};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::distributor::{DistributionMessage, MessageDistributor};
use crate::metrics::AggregatorMetrics;
use crate::pool::ConnectionId;

/// Message unique identifier (extracted from CoT XML)
pub type MessageUid = String;

/// Deduplication entry
#[derive(Debug, Clone)]
struct DeduplicationEntry {
    /// Message UID
    uid: MessageUid,
    /// First seen timestamp
    first_seen: Instant,
    /// Source connections that sent this message
    sources: Vec<ConnectionId>,
    /// Message hash for quick comparison
    hash: u64,
}

/// Configuration for message aggregator
#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    /// Deduplication window duration
    pub dedup_window: Duration,
    /// Maximum entries in deduplication cache
    pub max_cache_entries: usize,
    /// Cleanup interval for expired entries
    pub cleanup_interval: Duration,
    /// Inbound channel capacity
    pub channel_capacity: usize,
    /// Number of aggregator workers
    pub worker_count: usize,
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        Self {
            dedup_window: Duration::from_secs(60),
            max_cache_entries: 100_000,
            cleanup_interval: Duration::from_secs(10),
            channel_capacity: 10_000,
            worker_count: 4,
        }
    }
}

/// Inbound message with source information
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Message payload
    pub data: Vec<u8>,
    /// Source connection ID
    pub source: ConnectionId,
    /// Received timestamp
    pub timestamp: Instant,
}

/// Deduplication cache
///
/// LRU-style cache with time-based expiration for message deduplication.
struct DeduplicationCache {
    /// Cache entries indexed by UID
    entries: DashMap<MessageUid, DeduplicationEntry>,
    /// Entry queue for LRU eviction
    queue: Arc<parking_lot::Mutex<VecDeque<(MessageUid, Instant)>>>,
    /// Maximum cache size
    max_size: usize,
    /// Deduplication window
    window: Duration,
}

impl DeduplicationCache {
    fn new(max_size: usize, window: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            queue: Arc::new(parking_lot::Mutex::new(VecDeque::new())),
            max_size,
            window,
        }
    }

    /// Check if message is duplicate and record it
    ///
    /// Returns true if message is a duplicate (should be dropped)
    fn check_and_record(&self, uid: MessageUid, source: ConnectionId, hash: u64) -> bool {
        let now = Instant::now();

        // Check if UID exists in cache
        if let Some(mut entry) = self.entries.get_mut(&uid) {
            // Check if entry is still within deduplication window
            if entry.first_seen.elapsed() < self.window {
                // Duplicate message - record source and return true
                if !entry.sources.contains(&source) {
                    entry.sources.push(source);
                }
                return true;
            } else {
                // Entry expired - will be replaced
                drop(entry);
                self.entries.remove(&uid);
            }
        }

        // Not a duplicate - add new entry
        let entry = DeduplicationEntry {
            uid: uid.clone(),
            first_seen: now,
            sources: vec![source],
            hash,
        };

        self.entries.insert(uid.clone(), entry);

        // Add to queue for LRU eviction
        let mut queue = self.queue.lock();
        queue.push_back((uid, now));

        // Enforce max size
        while queue.len() > self.max_size {
            if let Some((old_uid, _)) = queue.pop_front() {
                self.entries.remove(&old_uid);
            }
        }

        false
    }

    /// Clean up expired entries
    fn cleanup(&self) {
        let now = Instant::now();
        let mut queue = self.queue.lock();
        let mut to_remove = Vec::new();

        // Find expired entries from front of queue
        while let Some((uid, timestamp)) = queue.front() {
            if now.duration_since(*timestamp) > self.window {
                to_remove.push(uid.clone());
                queue.pop_front();
            } else {
                break; // Queue is ordered by time
            }
        }

        drop(queue);

        // Remove expired entries
        for uid in to_remove {
            self.entries.remove(&uid);
        }
    }

    /// Get cache statistics
    fn stats(&self) -> (usize, usize) {
        let entry_count = self.entries.len();
        let queue_count = self.queue.lock().len();
        (entry_count, queue_count)
    }
}

/// Message Aggregator
///
/// Collects messages from multiple sources, deduplicates by UID,
/// and forwards unique messages to the distributor.
pub struct MessageAggregator {
    /// Inbound message receiver
    rx: Receiver<InboundMessage>,
    /// Sender for external submission
    tx: Sender<InboundMessage>,
    /// Message distributor
    distributor: Arc<MessageDistributor>,
    /// Deduplication cache
    dedup_cache: Arc<DeduplicationCache>,
    /// Configuration
    config: AggregatorConfig,
    /// Metrics
    metrics: Arc<AggregatorMetrics>,
    /// Worker task handles
    workers: Arc<parking_lot::RwLock<Vec<JoinHandle<()>>>>,
    /// Cleanup task handle
    cleanup_task: Arc<parking_lot::RwLock<Option<JoinHandle<()>>>>,
}

impl MessageAggregator {
    /// Create a new message aggregator
    pub fn new(distributor: Arc<MessageDistributor>, config: AggregatorConfig) -> Self {
        let (tx, rx) = flume::bounded(config.channel_capacity);
        let dedup_cache = Arc::new(DeduplicationCache::new(
            config.max_cache_entries,
            config.dedup_window,
        ));

        Self {
            rx,
            tx,
            distributor,
            dedup_cache,
            config,
            metrics: Arc::new(AggregatorMetrics::new()),
            workers: Arc::new(parking_lot::RwLock::new(Vec::new())),
            cleanup_task: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Get sender for submitting messages
    pub fn sender(&self) -> Sender<InboundMessage> {
        self.tx.clone()
    }

    /// Extract UID from CoT message
    ///
    /// Simplified extraction - real implementation would parse XML
    fn extract_uid(data: &[u8]) -> Option<MessageUid> {
        let msg_str = String::from_utf8_lossy(data);

        // Look for uid="..." in XML
        if let Some(start) = msg_str.find("uid=\"") {
            let uid_start = start + 5;
            if let Some(end) = msg_str[uid_start..].find('"') {
                return Some(msg_str[uid_start..uid_start + end].to_string());
            }
        }

        None
    }

    /// Calculate message hash for quick comparison
    fn calculate_hash(data: &[u8]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        hasher.finish()
    }

    /// Start the aggregator
    pub async fn start(&self) {
        info!("Starting message aggregator");

        // Spawn worker tasks
        let mut workers = self.workers.write();
        for worker_id in 0..self.config.worker_count {
            let handle = self.spawn_worker(worker_id).await;
            workers.push(handle);
        }

        // Spawn cleanup task
        let cleanup_handle = self.spawn_cleanup_task().await;
        *self.cleanup_task.write() = Some(cleanup_handle);

        info!(
            worker_count = self.config.worker_count,
            "Message aggregator started"
        );
    }

    /// Spawn an aggregator worker
    async fn spawn_worker(&self, worker_id: usize) -> JoinHandle<()> {
        let rx = self.rx.clone();
        let distributor = Arc::clone(&self.distributor);
        let dedup_cache = Arc::clone(&self.dedup_cache);
        let metrics = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            debug!(worker_id, "Aggregator worker started");

            while let Ok(msg) = rx.recv_async().await {
                metrics.record_message_received();

                // Extract UID from message
                let uid = match Self::extract_uid(&msg.data) {
                    Some(uid) => uid,
                    None => {
                        // No UID found - forward message anyway
                        debug!(
                            worker_id,
                            "Message has no UID, forwarding without deduplication"
                        );
                        metrics.record_no_uid();

                        let dist_msg = DistributionMessage {
                            data: msg.data,
                            source: Some(msg.source),
                            timestamp: msg.timestamp,
                        };

                        if let Err(e) = distributor.sender().send_async(dist_msg).await {
                            warn!(worker_id, error = %e, "Failed to forward message to distributor");
                        }
                        continue;
                    }
                };

                // Calculate message hash
                let hash = Self::calculate_hash(&msg.data);

                // Check for duplicate
                let is_duplicate =
                    dedup_cache.check_and_record(uid.clone(), msg.source.clone(), hash);

                if is_duplicate {
                    metrics.record_duplicate();
                    debug!(
                        worker_id,
                        uid = %uid,
                        "Duplicate message detected, dropping"
                    );
                    continue;
                }

                // Unique message - forward to distributor
                metrics.record_unique();

                let dist_msg = DistributionMessage {
                    data: msg.data,
                    source: Some(msg.source),
                    timestamp: msg.timestamp,
                };

                if let Err(e) = distributor.sender().send_async(dist_msg).await {
                    warn!(worker_id, error = %e, "Failed to forward message to distributor");
                } else {
                    debug!(worker_id, uid = %uid, "Unique message forwarded to distributor");
                }
            }

            debug!(worker_id, "Aggregator worker stopped");
        })
    }

    /// Spawn cleanup task for deduplication cache
    async fn spawn_cleanup_task(&self) -> JoinHandle<()> {
        let dedup_cache = Arc::clone(&self.dedup_cache);
        let interval = self.config.cleanup_interval;
        let metrics = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            debug!("Cleanup task started");

            loop {
                tokio::time::sleep(interval).await;

                let (before_entries, _) = dedup_cache.stats();
                dedup_cache.cleanup();
                let (after_entries, _) = dedup_cache.stats();

                let cleaned = before_entries.saturating_sub(after_entries);
                if cleaned > 0 {
                    debug!(
                        cleaned_entries = cleaned,
                        remaining_entries = after_entries,
                        "Deduplication cache cleanup completed"
                    );
                    metrics.record_cache_cleanup(cleaned);
                }
            }
        })
    }

    /// Stop the aggregator
    pub async fn stop(&self) {
        info!("Stopping message aggregator");

        // Stop cleanup task
        if let Some(task) = self.cleanup_task.write().take() {
            task.abort();
            let _ = task.await;
        }

        // Abort and join all workers. Workers loop on rx.recv_async() forever
        // (the channel is never closed), so awaiting without aborting deadlocks.
        // Collect handles first so the lock guard isn't held across .await.
        let handles: Vec<_> = self.workers.write().drain(..).collect();
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }

        info!("Message aggregator stopped");
    }

    /// Get aggregator metrics
    pub fn metrics(&self) -> Arc<AggregatorMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Get pending message count
    pub fn pending_count(&self) -> usize {
        self.rx.len()
    }

    /// Get deduplication cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        self.dedup_cache.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributor::DistributorConfig;
    use crate::pool::{ConnectionPool, PoolConfig};

    #[test]
    fn test_extract_uid() {
        let msg = b"<event uid=\"test-123\" type=\"a-f-G\">";
        let uid = MessageAggregator::extract_uid(msg);
        assert_eq!(uid, Some("test-123".to_string()));
    }

    #[test]
    fn test_extract_uid_missing() {
        let msg = b"<event type=\"a-f-G\">";
        let uid = MessageAggregator::extract_uid(msg);
        assert_eq!(uid, None);
    }

    #[test]
    fn test_dedup_cache_basic() {
        let cache = DeduplicationCache::new(100, Duration::from_secs(60));

        let is_dup = cache.check_and_record("uid-1".to_string(), "conn-1".to_string(), 12345);
        assert!(!is_dup);

        let is_dup = cache.check_and_record("uid-1".to_string(), "conn-2".to_string(), 12345);
        assert!(is_dup);
    }

    #[test]
    fn test_dedup_cache_expiration() {
        let cache = DeduplicationCache::new(100, Duration::from_millis(100));

        let is_dup = cache.check_and_record("uid-1".to_string(), "conn-1".to_string(), 12345);
        assert!(!is_dup);

        std::thread::sleep(Duration::from_millis(150));

        let is_dup = cache.check_and_record("uid-1".to_string(), "conn-2".to_string(), 12345);
        assert!(!is_dup); // Expired, treated as new
    }

    #[test]
    fn test_dedup_cache_lru_eviction() {
        let cache = DeduplicationCache::new(2, Duration::from_secs(60));

        cache.check_and_record("uid-1".to_string(), "conn-1".to_string(), 1);
        cache.check_and_record("uid-2".to_string(), "conn-1".to_string(), 2);
        cache.check_and_record("uid-3".to_string(), "conn-1".to_string(), 3);

        let (entries, _) = cache.stats();
        assert_eq!(entries, 2); // Only 2 entries due to LRU eviction
    }

    #[tokio::test]
    async fn test_aggregator_creation() {
        let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
        let distributor = Arc::new(MessageDistributor::new(pool, DistributorConfig::default()));
        let aggregator = MessageAggregator::new(distributor, AggregatorConfig::default());

        assert_eq!(aggregator.pending_count(), 0);
    }
}
