//! Concurrency Control
//!
//! Limits max concurrent connections, implements connection queue,
//! priority queue for critical connections, and semaphore-based rate limiting.

use anyhow::{Context, Result};
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::pool::ConnectionId;

/// Connection priority (higher number = higher priority)
pub type Priority = u8;

/// Connection request in queue
#[derive(Debug, Clone)]
pub struct ConnectionRequest {
    /// Connection ID
    pub id: ConnectionId,
    /// Display name
    pub name: String,
    /// Server address
    pub address: String,
    /// Priority (higher = more important)
    pub priority: Priority,
    /// Requested timestamp
    pub requested_at: Instant,
}

impl ConnectionRequest {
    pub fn new(id: ConnectionId, name: String, address: String, priority: Priority) -> Self {
        Self {
            id,
            name,
            address,
            priority,
            requested_at: Instant::now(),
        }
    }
}

impl PartialEq for ConnectionRequest {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.requested_at == other.requested_at
    }
}

impl Eq for ConnectionRequest {}

impl PartialOrd for ConnectionRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConnectionRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher priority first, then earlier timestamp
        match self.priority.cmp(&other.priority) {
            std::cmp::Ordering::Equal => other.requested_at.cmp(&self.requested_at),
            other => other,
        }
    }
}

/// Concurrency control configuration
#[derive(Debug, Clone)]
pub struct ConcurrencyConfig {
    /// Maximum concurrent connections
    pub max_concurrent: usize,
    /// Maximum queue size
    pub max_queue_size: usize,
    /// Queue timeout (requests older than this are rejected)
    pub queue_timeout: Duration,
    /// Queue processing interval
    pub processing_interval: Duration,
    /// Enable rate limiting
    pub enable_rate_limit: bool,
    /// Rate limit: max operations per second
    pub rate_limit_ops_per_sec: u32,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10_000,
            max_queue_size: 1_000,
            queue_timeout: Duration::from_secs(30),
            processing_interval: Duration::from_millis(100),
            enable_rate_limit: false,
            rate_limit_ops_per_sec: 1000,
        }
    }
}

/// Concurrency limiter statistics
#[derive(Debug, Clone)]
pub struct LimiterStats {
    /// Current active connections
    pub active_connections: usize,
    /// Queued requests
    pub queued_requests: usize,
    /// Total accepted requests
    pub total_accepted: u64,
    /// Total rejected requests
    pub total_rejected: u64,
    /// Total timed out requests
    pub total_timeouts: u64,
    /// Available permits
    pub available_permits: usize,
}

/// Concurrency Limiter
///
/// Limits maximum concurrent connections using semaphores,
/// implements priority queue for pending requests, and provides
/// rate limiting capabilities.
pub struct ConcurrencyLimiter {
    /// Semaphore for connection slots
    semaphore: Arc<Semaphore>,
    /// Priority queue for pending requests
    queue: Arc<parking_lot::Mutex<BinaryHeap<ConnectionRequest>>>,
    /// Configuration
    config: ConcurrencyConfig,
    /// Rate limiter semaphore
    rate_limiter: Option<Arc<Semaphore>>,
    /// Rate limiter refill task
    rate_limiter_task: Arc<parking_lot::RwLock<Option<JoinHandle<()>>>>,
    /// Statistics
    stats: Arc<LimiterStatistics>,
}

/// Internal statistics tracking
#[derive(Debug)]
struct LimiterStatistics {
    accepted: std::sync::atomic::AtomicU64,
    rejected: std::sync::atomic::AtomicU64,
    timeouts: std::sync::atomic::AtomicU64,
}

impl LimiterStatistics {
    fn new() -> Self {
        Self {
            accepted: std::sync::atomic::AtomicU64::new(0),
            rejected: std::sync::atomic::AtomicU64::new(0),
            timeouts: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl ConcurrencyLimiter {
    /// Create a new concurrency limiter
    pub fn new(config: ConcurrencyConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        let rate_limiter = if config.enable_rate_limit {
            Some(Arc::new(Semaphore::new(
                config.rate_limit_ops_per_sec as usize,
            )))
        } else {
            None
        };

        Self {
            semaphore,
            queue: Arc::new(parking_lot::Mutex::new(BinaryHeap::new())),
            config,
            rate_limiter,
            rate_limiter_task: Arc::new(parking_lot::RwLock::new(None)),
            stats: Arc::new(LimiterStatistics::new()),
        }
    }

    /// Start the concurrency limiter (starts rate limiter refill if enabled)
    pub async fn start(&self) {
        if let Some(rate_limiter) = &self.rate_limiter {
            let task = self
                .spawn_rate_limiter_refill(Arc::clone(rate_limiter))
                .await;
            *self.rate_limiter_task.write() = Some(task);
            info!(
                ops_per_sec = self.config.rate_limit_ops_per_sec,
                "Rate limiter started"
            );
        }
    }

    /// Spawn rate limiter refill task
    async fn spawn_rate_limiter_refill(&self, rate_limiter: Arc<Semaphore>) -> JoinHandle<()> {
        let ops_per_sec = self.config.rate_limit_ops_per_sec as usize;

        tokio::spawn(async move {
            // Refill permits every second
            let mut interval = tokio::time::interval(Duration::from_secs(1));

            loop {
                interval.tick().await;

                // Add back permits up to the limit
                let available = rate_limiter.available_permits();
                if available < ops_per_sec {
                    let to_add = ops_per_sec - available;
                    rate_limiter.add_permits(to_add);
                }
            }
        })
    }

    /// Stop the concurrency limiter
    pub async fn stop(&self) {
        // Take the handle out before awaiting so the lock isn't held across .await.
        let task = self.rate_limiter_task.write().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        info!("Concurrency limiter stopped");
    }

    /// Acquire a connection permit
    ///
    /// Returns a permit if available immediately, otherwise returns None.
    pub async fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        self.semaphore.try_acquire().ok()
    }

    /// Acquire a connection permit with timeout
    pub async fn acquire_timeout(&self, timeout: Duration) -> Result<SemaphorePermit<'_>> {
        tokio::select! {
            permit = self.semaphore.acquire() => {
                permit.context("Failed to acquire semaphore permit")
            }
            _ = tokio::time::sleep(timeout) => {
                anyhow::bail!("Timeout waiting for connection permit")
            }
        }
    }

    /// Acquire a connection permit (blocking)
    pub async fn acquire(&self) -> Result<SemaphorePermit<'_>> {
        self.stats
            .accepted
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.semaphore
            .acquire()
            .await
            .context("Failed to acquire semaphore permit")
    }

    /// Acquire rate limit permit if enabled
    pub async fn acquire_rate_limit(&self) -> Result<Option<SemaphorePermit<'_>>> {
        if let Some(rate_limiter) = &self.rate_limiter {
            let permit = rate_limiter
                .acquire()
                .await
                .context("Failed to acquire rate limit permit")?;
            Ok(Some(permit))
        } else {
            Ok(None)
        }
    }

    /// Try to acquire rate limit permit without waiting
    pub fn try_acquire_rate_limit(&self) -> Option<SemaphorePermit<'_>> {
        if let Some(rate_limiter) = &self.rate_limiter {
            rate_limiter.try_acquire().ok()
        } else {
            None
        }
    }

    /// Add a connection request to the queue
    pub fn enqueue(&self, request: ConnectionRequest) -> Result<()> {
        let mut queue = self.queue.lock();

        if queue.len() >= self.config.max_queue_size {
            self.stats
                .rejected
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            anyhow::bail!(
                "Connection queue full: {}/{}",
                queue.len(),
                self.config.max_queue_size
            );
        }

        queue.push(request);
        Ok(())
    }

    /// Dequeue the highest priority request
    pub fn dequeue(&self) -> Option<ConnectionRequest> {
        let mut queue = self.queue.lock();

        // Remove expired requests
        let now = Instant::now();
        let mut temp_queue = BinaryHeap::new();

        while let Some(req) = queue.pop() {
            if now.duration_since(req.requested_at) <= self.config.queue_timeout {
                temp_queue.push(req);
            } else {
                self.stats
                    .timeouts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    connection_id = %req.id,
                    "Connection request timed out in queue"
                );
            }
        }

        *queue = temp_queue;

        queue.pop()
    }

    /// Get the number of queued requests
    pub fn queue_len(&self) -> usize {
        self.queue.lock().len()
    }

    /// Get the number of available permits
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Get limiter statistics
    pub fn stats(&self) -> LimiterStats {
        LimiterStats {
            active_connections: self.config.max_concurrent - self.available_permits(),
            queued_requests: self.queue_len(),
            total_accepted: self
                .stats
                .accepted
                .load(std::sync::atomic::Ordering::Relaxed),
            total_rejected: self
                .stats
                .rejected
                .load(std::sync::atomic::Ordering::Relaxed),
            total_timeouts: self
                .stats
                .timeouts
                .load(std::sync::atomic::Ordering::Relaxed),
            available_permits: self.available_permits(),
        }
    }

    /// Clear all queued requests
    pub fn clear_queue(&self) {
        let mut queue = self.queue.lock();
        queue.clear();
    }
}

/// Connection permit guard
///
/// Wrapper that holds both connection and rate limit permits
pub struct ConnectionPermit<'a> {
    _connection_permit: SemaphorePermit<'a>,
    _rate_limit_permit: Option<SemaphorePermit<'a>>,
}

impl<'a> ConnectionPermit<'a> {
    pub fn new(
        connection_permit: SemaphorePermit<'a>,
        rate_limit_permit: Option<SemaphorePermit<'a>>,
    ) -> Self {
        Self {
            _connection_permit: connection_permit,
            _rate_limit_permit: rate_limit_permit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_permit() {
        let config = ConcurrencyConfig {
            max_concurrent: 2,
            ..Default::default()
        };
        let limiter = ConcurrencyLimiter::new(config);

        let permit1 = limiter.acquire().await.unwrap();
        let permit2 = limiter.acquire().await.unwrap();

        // Third acquire should timeout
        let result = tokio::time::timeout(Duration::from_millis(100), limiter.acquire()).await;
        assert!(result.is_err());

        drop(permit1);
        drop(permit2);
    }

    #[tokio::test]
    async fn test_priority_queue() {
        let config = ConcurrencyConfig::default();
        let limiter = ConcurrencyLimiter::new(config);

        let req1 = ConnectionRequest::new(
            "low".to_string(),
            "Low Priority".to_string(),
            "localhost:8087".to_string(),
            1,
        );

        let req2 = ConnectionRequest::new(
            "high".to_string(),
            "High Priority".to_string(),
            "localhost:8088".to_string(),
            10,
        );

        limiter.enqueue(req1).unwrap();
        limiter.enqueue(req2.clone()).unwrap();

        let dequeued = limiter.dequeue().unwrap();
        assert_eq!(dequeued.id, req2.id);
        assert_eq!(dequeued.priority, 10);
    }

    #[tokio::test]
    async fn test_queue_full() {
        let config = ConcurrencyConfig {
            max_queue_size: 2,
            ..Default::default()
        };
        let limiter = ConcurrencyLimiter::new(config);

        let req1 = ConnectionRequest::new(
            "1".to_string(),
            "Test 1".to_string(),
            "localhost:8087".to_string(),
            5,
        );

        let req2 = ConnectionRequest::new(
            "2".to_string(),
            "Test 2".to_string(),
            "localhost:8088".to_string(),
            5,
        );

        let req3 = ConnectionRequest::new(
            "3".to_string(),
            "Test 3".to_string(),
            "localhost:8089".to_string(),
            5,
        );

        limiter.enqueue(req1).unwrap();
        limiter.enqueue(req2).unwrap();

        let result = limiter.enqueue(req3);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_queue_timeout() {
        let config = ConcurrencyConfig {
            queue_timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let limiter = ConcurrencyLimiter::new(config);

        let req = ConnectionRequest::new(
            "test".to_string(),
            "Test".to_string(),
            "localhost:8087".to_string(),
            5,
        );

        limiter.enqueue(req).unwrap();
        assert_eq!(limiter.queue_len(), 1);

        tokio::time::sleep(Duration::from_millis(150)).await;

        let dequeued = limiter.dequeue();
        assert!(dequeued.is_none());
        assert_eq!(limiter.queue_len(), 0);
    }

    #[tokio::test]
    async fn test_stats() {
        let config = ConcurrencyConfig {
            max_concurrent: 10,
            ..Default::default()
        };
        let limiter = ConcurrencyLimiter::new(config);

        let stats = limiter.stats();
        assert_eq!(stats.active_connections, 0);
        assert_eq!(stats.available_permits, 10);

        let _permit1 = limiter.acquire().await.unwrap();
        let _permit2 = limiter.acquire().await.unwrap();

        let stats = limiter.stats();
        assert_eq!(stats.active_connections, 2);
        assert_eq!(stats.available_permits, 8);
    }
}
