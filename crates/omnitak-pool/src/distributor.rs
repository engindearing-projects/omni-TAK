//! Message Distributor
//!
//! Receives CoT messages from any source, applies filters to determine
//! destinations, and distributes to relevant connections with backpressure
//! handling for slow consumers.

use anyhow::Result;
use flume::{Receiver, Sender};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::metrics::DistributorMetrics;
use crate::pool::{ConnectionId, ConnectionPool, PoolMessage};

/// Filter rule for message distribution
#[derive(Clone)]
pub enum FilterRule {
    /// Always send to this connection
    AlwaysSend,
    /// Never send to this connection
    NeverSend,
    /// Send based on message type
    ByType(Vec<String>),
    /// Send based on callsign pattern
    ByCallsign(String),
    /// Send based on geographic bounds (lat, lon, radius_km)
    ByGeoBounds { lat: f64, lon: f64, radius_km: f64 },
    /// Custom filter function
    Custom(Arc<dyn Fn(&[u8]) -> bool + Send + Sync>),
}

impl std::fmt::Debug for FilterRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlwaysSend => write!(f, "AlwaysSend"),
            Self::NeverSend => write!(f, "NeverSend"),
            Self::ByType(types) => f.debug_tuple("ByType").field(types).finish(),
            Self::ByCallsign(pattern) => f.debug_tuple("ByCallsign").field(pattern).finish(),
            Self::ByGeoBounds {
                lat,
                lon,
                radius_km,
            } => f
                .debug_struct("ByGeoBounds")
                .field("lat", lat)
                .field("lon", lon)
                .field("radius_km", radius_km)
                .finish(),
            Self::Custom(_) => write!(f, "Custom(<function>)"),
        }
    }
}

impl FilterRule {
    /// Check if message matches this filter
    pub fn matches(&self, message: &[u8]) -> bool {
        match self {
            FilterRule::AlwaysSend => true,
            FilterRule::NeverSend => false,
            FilterRule::ByType(types) => {
                // Simple type detection - in real impl would parse XML
                let msg_str = String::from_utf8_lossy(message);
                types.iter().any(|t| msg_str.contains(t))
            }
            FilterRule::ByCallsign(pattern) => {
                let msg_str = String::from_utf8_lossy(message);
                msg_str.contains(pattern)
            }
            FilterRule::ByGeoBounds { .. } => {
                // Would need to parse CoT and extract location
                // Simplified for now
                true
            }
            FilterRule::Custom(func) => func(message),
        }
    }
}

/// Distribution strategy for handling slow consumers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DistributionStrategy {
    /// Drop messages if channel is full (default)
    #[default]
    DropOnFull,
    /// Block until space available (may cause head-of-line blocking)
    BlockOnFull,
    /// Try for timeout, then drop
    TryForTimeout(Duration),
}

/// Configuration for message distributor
#[derive(Debug, Clone)]
pub struct DistributorConfig {
    /// Inbound message channel capacity
    pub channel_capacity: usize,
    /// Distribution strategy
    pub strategy: DistributionStrategy,
    /// Max concurrent distribution tasks
    pub max_workers: usize,
    /// Batch size for distribution
    pub batch_size: usize,
    /// Flush interval for batching
    pub flush_interval: Duration,
}

impl Default for DistributorConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 10_000,
            strategy: DistributionStrategy::DropOnFull,
            max_workers: 16,
            batch_size: 100,
            flush_interval: Duration::from_millis(10),
        }
    }
}

/// Message to be distributed
#[derive(Debug, Clone)]
pub struct DistributionMessage {
    /// Message payload
    pub data: Vec<u8>,
    /// Optional source connection ID
    pub source: Option<ConnectionId>,
    /// Timestamp when received
    pub timestamp: Instant,
}

/// Message Distributor
///
/// Receives messages, applies filters, and distributes to matching connections
/// with backpressure handling for slow consumers.
pub struct MessageDistributor {
    /// Connection pool
    pool: Arc<ConnectionPool>,
    /// Filter rules per connection
    filters: Arc<parking_lot::RwLock<HashMap<ConnectionId, Vec<FilterRule>>>>,
    /// Inbound message channel
    rx: Receiver<DistributionMessage>,
    /// Sender for external submission
    tx: Sender<DistributionMessage>,
    /// Configuration
    config: DistributorConfig,
    /// Metrics
    metrics: Arc<DistributorMetrics>,
    /// Worker task handles
    workers: Arc<parking_lot::RwLock<Vec<JoinHandle<()>>>>,
}

impl MessageDistributor {
    /// Create a new message distributor
    pub fn new(pool: Arc<ConnectionPool>, config: DistributorConfig) -> Self {
        let (tx, rx) = flume::bounded(config.channel_capacity);

        Self {
            pool,
            filters: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            rx,
            tx,
            config,
            metrics: Arc::new(DistributorMetrics::new()),
            workers: Arc::new(parking_lot::RwLock::new(Vec::new())),
        }
    }

    /// Get sender for submitting messages
    pub fn sender(&self) -> Sender<DistributionMessage> {
        self.tx.clone()
    }

    /// Add filter rule for a connection
    pub fn add_filter(&self, connection_id: ConnectionId, rule: FilterRule) {
        let mut filters = self.filters.write();
        filters.entry(connection_id).or_default().push(rule);
    }

    /// Remove all filters for a connection
    pub fn remove_filters(&self, connection_id: &ConnectionId) {
        let mut filters = self.filters.write();
        filters.remove(connection_id);
    }

    /// Set filters for a connection (replaces existing)
    pub fn set_filters(&self, connection_id: ConnectionId, rules: Vec<FilterRule>) {
        let mut filters = self.filters.write();
        filters.insert(connection_id, rules);
    }

    /// Start the distributor
    pub async fn start(&self) {
        info!("Starting message distributor");

        // Spawn worker tasks, then store the handles. Collect first so the
        // workers lock isn't held across the spawn_worker await points.
        let mut handles = Vec::with_capacity(self.config.max_workers);
        for worker_id in 0..self.config.max_workers {
            handles.push(self.spawn_worker(worker_id).await);
        }
        self.workers.write().extend(handles);

        info!(
            worker_count = self.config.max_workers,
            "Message distributor started"
        );
    }

    /// Spawn a distribution worker
    async fn spawn_worker(&self, worker_id: usize) -> JoinHandle<()> {
        let rx = self.rx.clone();
        let pool = Arc::clone(&self.pool);
        let filters = Arc::clone(&self.filters);
        let metrics = Arc::clone(&self.metrics);
        let config = self.config.clone();

        tokio::spawn(async move {
            debug!(worker_id, "Distribution worker started");

            let mut batch = Vec::with_capacity(config.batch_size);
            let mut last_flush = Instant::now();

            loop {
                // Use async receive with tokio timeout for periodic flushing
                // This avoids blocking the tokio runtime worker threads
                let timeout_duration = config.flush_interval.saturating_sub(last_flush.elapsed());

                match tokio::time::timeout(timeout_duration, rx.recv_async()).await {
                    Ok(Ok(msg)) => {
                        batch.push(msg);

                        // Flush if batch is full or flush interval elapsed
                        if batch.len() >= config.batch_size
                            || last_flush.elapsed() >= config.flush_interval
                        {
                            Self::distribute_batch(&pool, &filters, &metrics, &config, &mut batch)
                                .await;
                            last_flush = Instant::now();
                        }
                    }
                    Ok(Err(flume::RecvError::Disconnected)) => {
                        warn!(worker_id, "Distribution channel disconnected");
                        break;
                    }
                    Err(_elapsed) => {
                        // Timeout - flush any pending messages
                        if !batch.is_empty() {
                            Self::distribute_batch(&pool, &filters, &metrics, &config, &mut batch)
                                .await;
                            last_flush = Instant::now();
                        }
                    }
                }
            }

            debug!(worker_id, "Distribution worker stopped");
        })
    }

    /// Distribute a batch of messages
    async fn distribute_batch(
        pool: &Arc<ConnectionPool>,
        filters: &Arc<parking_lot::RwLock<HashMap<ConnectionId, Vec<FilterRule>>>>,
        metrics: &Arc<DistributorMetrics>,
        config: &DistributorConfig,
        batch: &mut Vec<DistributionMessage>,
    ) {
        if batch.is_empty() {
            return;
        }

        let batch_start = Instant::now();
        let batch_size = batch.len();

        // Get active connections
        let connections = pool.get_active_connections();

        // Clone filter rules to avoid holding the lock across awaits
        let connection_filters: HashMap<String, Vec<FilterRule>> = {
            let filter_map = filters.read();
            filter_map.clone()
        }; // filter_map guard is dropped here

        for msg in batch.drain(..) {
            metrics.record_message_received();

            let mut distributed_count = 0;

            for connection in &connections {
                // Skip source connection to avoid loops
                if let Some(ref source) = msg.source {
                    if source == &connection.id {
                        continue;
                    }
                }

                // Check filters
                let should_send = if let Some(rules) = connection_filters.get(&connection.id) {
                    rules.iter().any(|rule| rule.matches(&msg.data))
                } else {
                    // No filters = send to all (default behavior)
                    true
                };

                if !should_send {
                    continue;
                }

                // Attempt to send based on strategy
                let send_result: Result<(), String> = match config.strategy {
                    DistributionStrategy::DropOnFull => connection
                        .tx
                        .try_send(PoolMessage::Cot(msg.data.clone()))
                        .map_err(|e| e.to_string()),
                    DistributionStrategy::BlockOnFull => connection
                        .tx
                        .send_async(PoolMessage::Cot(msg.data.clone()))
                        .await
                        .map_err(|e| e.to_string()),
                    DistributionStrategy::TryForTimeout(timeout) => {
                        tokio::select! {
                            result = connection.tx.send_async(PoolMessage::Cot(msg.data.clone())) => {
                                result.map_err(|e| e.to_string())
                            }
                            _ = tokio::time::sleep(timeout) => {
                                Err("Send timeout".to_string())
                            }
                        }
                    }
                };

                match send_result {
                    Ok(_) => {
                        connection.state.record_sent();
                        distributed_count += 1;
                        metrics.record_message_sent();
                    }
                    Err(_) => {
                        // Channel full or disconnected
                        metrics.record_drop();
                        debug!(
                            connection_id = %connection.id,
                            "Failed to send message (channel full or disconnected)"
                        );
                    }
                }
            }

            // Record distribution latency
            let latency = msg.timestamp.elapsed();
            metrics.record_distribution_latency(latency);

            if distributed_count == 0 {
                debug!("Message not distributed to any connection");
            }
        }

        let batch_duration = batch_start.elapsed();
        metrics.record_batch_processed(batch_size, batch_duration);
    }

    /// Stop the distributor
    pub async fn stop(&self) {
        info!("Stopping message distributor");

        // Abort and join all workers. Workers loop on rx.recv_async() forever
        // (the channel is never closed), so awaiting without aborting deadlocks.
        // Collect handles first so the lock guard isn't held across .await.
        let handles: Vec<_> = self.workers.write().drain(..).collect();
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }

        info!("Message distributor stopped");
    }

    /// Get distributor metrics
    pub fn metrics(&self) -> Arc<DistributorMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Get pending message count
    pub fn pending_count(&self) -> usize {
        self.rx.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::PoolConfig;

    #[tokio::test]
    async fn test_filter_always_send() {
        let rule = FilterRule::AlwaysSend;
        assert!(rule.matches(b"test message"));
    }

    #[tokio::test]
    async fn test_filter_never_send() {
        let rule = FilterRule::NeverSend;
        assert!(!rule.matches(b"test message"));
    }

    #[tokio::test]
    async fn test_filter_by_type() {
        let rule = FilterRule::ByType(vec!["a-f-G".to_string()]);
        assert!(rule.matches(b"<event type=\"a-f-G\">"));
        assert!(!rule.matches(b"<event type=\"a-h-G\">"));
    }

    #[tokio::test]
    async fn test_distributor_creation() {
        let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
        let config = DistributorConfig::default();
        let distributor = MessageDistributor::new(pool, config);

        assert_eq!(distributor.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_filter_management() {
        let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
        let distributor = MessageDistributor::new(pool, DistributorConfig::default());

        let conn_id = "test-1".to_string();
        distributor.add_filter(conn_id.clone(), FilterRule::AlwaysSend);

        let filters = distributor.filters.read();
        assert!(filters.contains_key(&conn_id));
        assert_eq!(filters[&conn_id].len(), 1);
    }
}
