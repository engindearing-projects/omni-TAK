//! Connection Pool Manager
//!
//! Manages thousands of concurrent TAK server connections with dynamic
//! add/remove, health tracking, and graceful shutdown.

use anyhow::{Context, Result};
use dashmap::DashMap;
use flume::{Receiver, Sender};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::health::HealthMonitor;
use crate::metrics::PoolMetrics;

/// Unique identifier for a connection
pub type ConnectionId = String;

/// Message types that can flow through the pool
#[derive(Debug, Clone)]
pub enum PoolMessage {
    /// Cursor on Target message
    Cot(Vec<u8>),
    /// Health check ping
    Ping,
    /// Graceful shutdown signal
    Shutdown,
}

/// Connection metadata and state
#[derive(Debug)]
pub struct Connection {
    /// Unique identifier
    pub id: ConnectionId,
    /// Display name for logging
    pub name: String,
    /// Server address
    pub address: String,
    /// Connection priority (0 = lowest)
    pub priority: u8,
    /// Inbound message sender
    pub tx: Sender<PoolMessage>,
    /// Outbound message receiver
    pub rx: Receiver<PoolMessage>,
    /// Task handle for this connection
    pub task: JoinHandle<()>,
    /// Connection state
    pub state: Arc<ConnectionState>,
    /// Created timestamp
    pub created_at: Instant,
}

/// Byte length of a CoT payload carried by a pool message (0 for control messages).
fn cot_byte_len(message: &PoolMessage) -> u64 {
    match message {
        PoolMessage::Cot(data) => data.len() as u64,
        _ => 0,
    }
}

/// Runtime connection state
#[derive(Debug)]
pub struct ConnectionState {
    /// Is connection currently active
    pub active: AtomicBool,
    /// Last successful message timestamp (epoch millis)
    pub last_message: AtomicU64,
    /// Total messages sent
    pub messages_sent: AtomicU64,
    /// Total messages received
    pub messages_received: AtomicU64,
    /// Total bytes sent
    pub bytes_sent: AtomicU64,
    /// Total bytes received
    pub bytes_received: AtomicU64,
    /// Connection errors
    pub errors: AtomicU64,
    /// Last error message
    pub last_error: RwLock<Option<String>>,
}

impl ConnectionState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(true),
            last_message: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            last_error: RwLock::new(None),
        }
    }

    pub fn record_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        self.last_message.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    pub fn record_received(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.last_message.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            Ordering::Relaxed,
        );
    }

    pub fn record_error(&self, error: String) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        *self.last_error.write() = Some(error);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for connection pool
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Channel capacity per connection
    pub channel_capacity: usize,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Inactive timeout
    pub inactive_timeout: Duration,
    /// Enable auto-reconnect
    pub auto_reconnect: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            channel_capacity: 1000,
            health_check_interval: Duration::from_secs(30),
            inactive_timeout: Duration::from_secs(300),
            auto_reconnect: true,
        }
    }
}

/// Connection Pool Manager
///
/// Thread-safe pool that manages thousands of concurrent connections
/// with dynamic add/remove, health monitoring, and graceful shutdown.
pub struct ConnectionPool {
    /// Active connections indexed by ID
    connections: Arc<DashMap<ConnectionId, Arc<Connection>>>,
    /// Pool configuration
    config: PoolConfig,
    /// Health monitor
    health_monitor: Arc<HealthMonitor>,
    /// Metrics collector
    metrics: Arc<PoolMetrics>,
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
}

impl ConnectionPool {
    /// Create a new connection pool
    pub fn new(config: PoolConfig) -> Self {
        let metrics = Arc::new(PoolMetrics::new());
        Self {
            connections: Arc::new(DashMap::new()),
            config,
            health_monitor: Arc::new(HealthMonitor::new()),
            metrics,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get connection count
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Add a new connection to the pool
    ///
    /// Spawns a tokio task to manage the connection lifecycle.
    /// Returns an error if the pool is at capacity.
    pub async fn add_connection(
        &self,
        id: ConnectionId,
        name: String,
        address: String,
        priority: u8,
    ) -> Result<ConnectionId> {
        if self.connection_count() >= self.config.max_connections {
            anyhow::bail!(
                "Connection pool at capacity: {}/{}",
                self.connection_count(),
                self.config.max_connections
            );
        }

        if self.connections.contains_key(&id) {
            anyhow::bail!("Connection with ID '{}' already exists", id);
        }

        // Create channels for this connection
        let (tx, rx_internal) = flume::bounded(self.config.channel_capacity);
        let (tx_internal, rx) = flume::bounded(self.config.channel_capacity);

        let state = Arc::new(ConnectionState::new());
        let state_clone = Arc::clone(&state);
        let id_clone = id.clone();
        let address_clone = address.clone();
        let metrics = Arc::clone(&self.metrics);
        let shutdown = Arc::clone(&self.shutdown);

        // Spawn connection handler task
        let task = tokio::spawn(async move {
            info!(
                connection_id = %id_clone,
                address = %address_clone,
                "Connection handler started"
            );

            loop {
                if shutdown.load(Ordering::Relaxed) {
                    debug!(connection_id = %id_clone, "Shutdown signal received");
                    break;
                }

                tokio::select! {
                    // Handle incoming messages from connection
                    msg = rx_internal.recv_async() => {
                        match msg {
                            Ok(PoolMessage::Cot(data)) => {
                                state_clone.record_received();
                                state_clone
                                    .bytes_received
                                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                                metrics.record_message_received();

                                // Forward to outbound channel
                                if let Err(e) = tx_internal.send_async(PoolMessage::Cot(data)).await {
                                    error!(
                                        connection_id = %id_clone,
                                        error = %e,
                                        "Failed to forward message"
                                    );
                                    state_clone.record_error(e.to_string());
                                }
                            }
                            Ok(PoolMessage::Ping) => {
                                state_clone.record_received();
                                // Respond to ping
                                let _ = tx_internal.send_async(PoolMessage::Ping).await;
                            }
                            Ok(PoolMessage::Shutdown) => {
                                info!(connection_id = %id_clone, "Shutdown requested");
                                break;
                            }
                            Err(_) => {
                                warn!(connection_id = %id_clone, "Connection channel closed");
                                break;
                            }
                        }
                    }

                    // Timeout to check shutdown signal periodically
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        continue;
                    }
                }
            }

            state_clone.deactivate();
            info!(connection_id = %id_clone, "Connection handler stopped");
        });

        let connection = Arc::new(Connection {
            id: id.clone(),
            name,
            address,
            priority,
            tx,
            rx,
            task,
            state,
            created_at: Instant::now(),
        });

        self.connections.insert(id.clone(), connection);
        self.metrics.record_connection_added();

        info!(
            connection_id = %id,
            total_connections = self.connection_count(),
            "Connection added to pool"
        );

        Ok(id)
    }

    /// Remove a connection from the pool
    pub async fn remove_connection(&self, id: &ConnectionId) -> Result<()> {
        let connection = self
            .connections
            .remove(id)
            .context("Connection not found")?
            .1;

        // Send shutdown signal
        let _ = connection.tx.send_async(PoolMessage::Shutdown).await;

        // Deactivate state
        connection.state.deactivate();

        // Wait for task to complete (with timeout)
        // Try to unwrap the Arc to get ownership of the task
        match Arc::try_unwrap(connection) {
            Ok(mut conn) => {
                tokio::select! {
                    _ = &mut conn.task => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        warn!(connection_id = %id, "Connection task did not complete in time");
                    }
                }
            }
            Err(arc_conn) => {
                // Arc is still shared, just abort the task
                warn!(connection_id = %id, "Cannot unwrap Arc, aborting task");
                arc_conn.task.abort();
            }
        }

        self.metrics.record_connection_removed();

        info!(
            connection_id = %id,
            remaining_connections = self.connection_count(),
            "Connection removed from pool"
        );

        Ok(())
    }

    /// Get a connection by ID
    pub fn get_connection(&self, id: &ConnectionId) -> Option<Arc<Connection>> {
        self.connections.get(id).map(|entry| Arc::clone(&*entry))
    }

    /// Get all connection IDs
    pub fn list_connections(&self) -> Vec<ConnectionId> {
        self.connections
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get all active connections
    pub fn get_active_connections(&self) -> Vec<Arc<Connection>> {
        self.connections
            .iter()
            .filter(|entry| entry.value().state.is_active())
            .map(|entry| Arc::clone(&*entry))
            .collect()
    }

    /// Get connections by priority (sorted descending)
    pub fn get_connections_by_priority(&self) -> Vec<Arc<Connection>> {
        let mut connections: Vec<_> = self
            .connections
            .iter()
            .map(|entry| Arc::clone(&*entry))
            .collect();

        connections.sort_by(|a, b| b.priority.cmp(&a.priority));
        connections
    }

    /// Send message to a specific connection
    pub async fn send_to_connection(&self, id: &ConnectionId, message: PoolMessage) -> Result<()> {
        let connection = self.get_connection(id).context("Connection not found")?;

        let bytes = cot_byte_len(&message);
        connection
            .tx
            .send_async(message)
            .await
            .context("Failed to send message to connection")?;

        connection.state.record_sent();
        connection
            .state
            .bytes_sent
            .fetch_add(bytes, Ordering::Relaxed);
        self.metrics.record_message_sent();

        Ok(())
    }

    /// Broadcast message to all active connections
    pub async fn broadcast(&self, message: PoolMessage) -> usize {
        let connections = self.get_active_connections();
        let bytes = cot_byte_len(&message);
        let mut sent_count = 0;

        for connection in connections {
            match connection.tx.send_async(message.clone()).await {
                Ok(_) => {
                    connection.state.record_sent();
                    connection
                        .state
                        .bytes_sent
                        .fetch_add(bytes, Ordering::Relaxed);
                    self.metrics.record_message_sent();
                    sent_count += 1;
                }
                Err(e) => {
                    error!(
                        connection_id = %connection.id,
                        error = %e,
                        "Failed to broadcast message"
                    );
                    connection.state.record_error(e.to_string());
                }
            }
        }

        sent_count
    }

    /// Graceful shutdown of all connections
    pub async fn shutdown(&self) -> Result<()> {
        info!("Initiating graceful shutdown of connection pool");

        self.shutdown.store(true, Ordering::Relaxed);

        // Send shutdown to all connections
        let connection_ids: Vec<_> = self.list_connections();

        for id in &connection_ids {
            if let Err(e) = self.remove_connection(id).await {
                warn!(connection_id = %id, error = %e, "Error during connection removal");
            }
        }

        info!("Connection pool shutdown complete");
        Ok(())
    }

    /// Get pool metrics
    pub fn metrics(&self) -> Arc<PoolMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        let connections: Vec<_> = self.connections.iter().map(|e| Arc::clone(&*e)).collect();

        let active_count = connections.iter().filter(|c| c.state.is_active()).count();

        let total_sent: u64 = connections
            .iter()
            .map(|c| c.state.messages_sent.load(Ordering::Relaxed))
            .sum();

        let total_received: u64 = connections
            .iter()
            .map(|c| c.state.messages_received.load(Ordering::Relaxed))
            .sum();

        let total_errors: u64 = connections
            .iter()
            .map(|c| c.state.errors.load(Ordering::Relaxed))
            .sum();

        PoolStats {
            total_connections: connections.len(),
            active_connections: active_count,
            inactive_connections: connections.len() - active_count,
            total_messages_sent: total_sent,
            total_messages_received: total_received,
            total_errors,
        }
    }
}

/// Pool statistics snapshot
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub inactive_connections: usize,
    pub total_messages_sent: u64,
    pub total_messages_received: u64,
    pub total_errors: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_remove_connection() {
        let pool = ConnectionPool::new(PoolConfig::default());

        let id = pool
            .add_connection(
                "test-1".to_string(),
                "Test Connection".to_string(),
                "localhost:8087".to_string(),
                5,
            )
            .await
            .unwrap();

        assert_eq!(pool.connection_count(), 1);

        pool.remove_connection(&id).await.unwrap();

        assert_eq!(pool.connection_count(), 0);
    }

    #[tokio::test]
    async fn test_connection_capacity() {
        let config = PoolConfig {
            max_connections: 2,
            ..Default::default()
        };
        let pool = ConnectionPool::new(config);

        pool.add_connection(
            "test-1".to_string(),
            "Test 1".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

        pool.add_connection(
            "test-2".to_string(),
            "Test 2".to_string(),
            "localhost:8088".to_string(),
            5,
        )
        .await
        .unwrap();

        let result = pool
            .add_connection(
                "test-3".to_string(),
                "Test 3".to_string(),
                "localhost:8089".to_string(),
                5,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_broadcast() {
        let pool = ConnectionPool::new(PoolConfig::default());

        pool.add_connection(
            "test-1".to_string(),
            "Test 1".to_string(),
            "localhost:8087".to_string(),
            5,
        )
        .await
        .unwrap();

        pool.add_connection(
            "test-2".to_string(),
            "Test 2".to_string(),
            "localhost:8088".to_string(),
            5,
        )
        .await
        .unwrap();

        let sent = pool.broadcast(PoolMessage::Ping).await;
        assert_eq!(sent, 2);
    }
}
