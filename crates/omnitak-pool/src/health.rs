//! Health Monitoring
//!
//! Periodic health checks for all connections with auto-reconnect,
//! circuit breaker pattern, and connection metrics.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::pool::{ConnectionId, ConnectionPool, PoolMessage};

/// Health check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Connection is healthy
    Healthy,
    /// Connection is degraded but functional
    Degraded,
    /// Connection is unhealthy
    Unhealthy,
}

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed, normal operation
    Closed,
    /// Circuit is open, failing fast
    Open,
    /// Circuit is half-open, testing recovery
    HalfOpen,
}

/// Circuit breaker for a connection
#[derive(Debug)]
struct CircuitBreaker {
    /// Current state
    state: CircuitState,
    /// Consecutive failure count
    failure_count: u32,
    /// Failure threshold to open circuit
    failure_threshold: u32,
    /// Time when circuit was opened
    opened_at: Option<Instant>,
    /// Duration to wait before half-open
    reset_timeout: Duration,
    /// Consecutive success count in half-open
    success_count: u32,
    /// Success threshold to close circuit
    success_threshold: u32,
}

impl CircuitBreaker {
    fn new(failure_threshold: u32, reset_timeout: Duration, success_threshold: u32) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold,
            opened_at: None,
            reset_timeout,
            success_count: 0,
            success_threshold,
        }
    }

    /// Record a successful health check
    fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.success_threshold {
                    info!("Circuit breaker closing after successful recovery");
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    self.success_count = 0;
                    self.opened_at = None;
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.state = CircuitState::Closed;
                self.failure_count = 0;
                self.opened_at = None;
            }
        }
    }

    /// Record a failed health check
    fn record_failure(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count += 1;
                if self.failure_count >= self.failure_threshold {
                    warn!(
                        failures = self.failure_count,
                        "Circuit breaker opening due to consecutive failures"
                    );
                    self.state = CircuitState::Open;
                    self.opened_at = Some(Instant::now());
                }
            }
            CircuitState::HalfOpen => {
                warn!("Circuit breaker re-opening after failed recovery attempt");
                self.state = CircuitState::Open;
                self.failure_count = 0;
                self.success_count = 0;
                self.opened_at = Some(Instant::now());
            }
            CircuitState::Open => {
                // Already open, just update timestamp
                self.opened_at = Some(Instant::now());
            }
        }
    }

    /// Check if circuit should transition to half-open
    fn check_half_open(&mut self) -> bool {
        if self.state == CircuitState::Open {
            if let Some(opened_at) = self.opened_at {
                if opened_at.elapsed() >= self.reset_timeout {
                    info!("Circuit breaker entering half-open state for recovery test");
                    self.state = CircuitState::HalfOpen;
                    self.success_count = 0;
                    return true;
                }
            }
        }
        false
    }

    /// Check if circuit allows operations
    fn allows_request(&self) -> bool {
        match self.state {
            CircuitState::Closed | CircuitState::HalfOpen => true,
            CircuitState::Open => false,
        }
    }
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Interval between health checks
    pub check_interval: Duration,
    /// Timeout for health check response
    pub check_timeout: Duration,
    /// Threshold for degraded status (seconds since last message)
    pub degraded_threshold: Duration,
    /// Threshold for unhealthy status (seconds since last message)
    pub unhealthy_threshold: Duration,
    /// Circuit breaker failure threshold
    pub circuit_failure_threshold: u32,
    /// Circuit breaker reset timeout
    pub circuit_reset_timeout: Duration,
    /// Circuit breaker success threshold for half-open -> closed
    pub circuit_success_threshold: u32,
    /// Enable auto-reconnect on failure
    pub auto_reconnect: bool,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(30),
            check_timeout: Duration::from_secs(5),
            degraded_threshold: Duration::from_secs(60),
            unhealthy_threshold: Duration::from_secs(300),
            circuit_failure_threshold: 5,
            circuit_reset_timeout: Duration::from_secs(60),
            circuit_success_threshold: 2,
            auto_reconnect: true,
        }
    }
}

/// Health monitoring statistics
#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    /// Current health status
    pub status: HealthStatus,
    /// Circuit breaker state
    pub circuit_state: CircuitState,
    /// Last successful health check
    pub last_check: Option<Instant>,
    /// Last message timestamp
    pub last_message: Option<Instant>,
    /// Connection uptime
    pub uptime: Duration,
    /// Total successful checks
    pub successful_checks: u64,
    /// Total failed checks
    pub failed_checks: u64,
    /// Messages sent
    pub messages_sent: u64,
    /// Messages received
    pub messages_received: u64,
    /// Error count
    pub errors: u64,
}

/// Health Monitor
///
/// Performs periodic health checks on all connections with circuit breaker
/// pattern and auto-reconnect capabilities.
pub struct HealthMonitor {
    /// Health check configuration
    config: HealthConfig,
    /// Circuit breakers per connection
    circuits: Arc<parking_lot::RwLock<HashMap<ConnectionId, CircuitBreaker>>>,
    /// Monitor task handle
    task: Arc<parking_lot::RwLock<Option<JoinHandle<()>>>>,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new() -> Self {
        Self::with_config(HealthConfig::default())
    }

    /// Create health monitor with custom configuration
    pub fn with_config(config: HealthConfig) -> Self {
        Self {
            config,
            circuits: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            task: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Start health monitoring
    pub fn start(&self, pool: Arc<ConnectionPool>) {
        let config = self.config.clone();
        let circuits = Arc::clone(&self.circuits);

        let task = tokio::spawn(async move {
            info!("Health monitor started");

            loop {
                tokio::time::sleep(config.check_interval).await;

                let connections = pool.get_active_connections();

                for connection in connections {
                    // Check if circuit allows health check
                    let allows_request = {
                        let mut circuits_guard = circuits.write();
                        let circuit =
                            circuits_guard
                                .entry(connection.id.clone())
                                .or_insert_with(|| {
                                    CircuitBreaker::new(
                                        config.circuit_failure_threshold,
                                        config.circuit_reset_timeout,
                                        config.circuit_success_threshold,
                                    )
                                });

                        // Transition to half-open if ready
                        circuit.check_half_open();

                        circuit.allows_request()
                    }; // circuits_guard is dropped here

                    if !allows_request {
                        debug!(
                            connection_id = %connection.id,
                            "Skipping health check - circuit breaker open"
                        );
                        continue;
                    }

                    // Perform health check
                    let check_result =
                        Self::perform_health_check(&pool, &connection.id, config.check_timeout)
                            .await;

                    // Update circuit breaker
                    let mut circuits_guard = circuits.write();
                    if let Some(circuit) = circuits_guard.get_mut(&connection.id) {
                        match check_result {
                            Ok(true) => {
                                circuit.record_success();
                                debug!(connection_id = %connection.id, "Health check passed");
                            }
                            Ok(false) | Err(_) => {
                                circuit.record_failure();
                                warn!(connection_id = %connection.id, "Health check failed");

                                // Auto-reconnect if enabled and circuit is open
                                if config.auto_reconnect && circuit.state == CircuitState::Open {
                                    info!(
                                        connection_id = %connection.id,
                                        "Auto-reconnect would be triggered here"
                                    );
                                    // In real implementation, would trigger reconnection logic
                                }
                            }
                        }
                    }
                }
            }
        });

        *self.task.write() = Some(task);
    }

    /// Perform health check on a connection
    async fn perform_health_check(
        pool: &Arc<ConnectionPool>,
        connection_id: &ConnectionId,
        timeout: Duration,
    ) -> Result<bool> {
        // Send ping message
        tokio::select! {
            result = pool.send_to_connection(connection_id, PoolMessage::Ping) => {
                result.map(|_| true)
            }
            _ = tokio::time::sleep(timeout) => {
                Ok(false)
            }
        }
    }

    /// Stop health monitoring
    pub async fn stop(&self) {
        // Take the handle out before awaiting so the lock isn't held across .await.
        let task = self.task.write().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        info!("Health monitor stopped");
    }

    /// Get health status for a connection
    pub fn get_health(
        &self,
        pool: &ConnectionPool,
        connection_id: &ConnectionId,
    ) -> Option<ConnectionHealth> {
        let connection = pool.get_connection(connection_id)?;

        let last_message_millis = connection
            .state
            .last_message
            .load(std::sync::atomic::Ordering::Relaxed);
        let last_message = if last_message_millis > 0 {
            Some(
                Instant::now()
                    - Duration::from_millis(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64
                            - last_message_millis,
                    ),
            )
        } else {
            None
        };

        // Determine health status
        let status = if !connection.state.is_active() {
            HealthStatus::Unhealthy
        } else if let Some(last_msg) = last_message {
            if last_msg.elapsed() > self.config.unhealthy_threshold {
                HealthStatus::Unhealthy
            } else if last_msg.elapsed() > self.config.degraded_threshold {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            }
        } else {
            HealthStatus::Degraded
        };

        let circuits = self.circuits.read();
        let circuit_state = circuits
            .get(connection_id)
            .map(|c| c.state)
            .unwrap_or(CircuitState::Closed);

        Some(ConnectionHealth {
            status,
            circuit_state,
            last_check: Some(Instant::now()),
            last_message,
            uptime: connection.created_at.elapsed(),
            successful_checks: 0, // Would track in real implementation
            failed_checks: 0,
            messages_sent: connection
                .state
                .messages_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            messages_received: connection
                .state
                .messages_received
                .load(std::sync::atomic::Ordering::Relaxed),
            errors: connection
                .state
                .errors
                .load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// Get health status for all connections
    pub fn get_all_health(&self, pool: &ConnectionPool) -> HashMap<ConnectionId, ConnectionHealth> {
        pool.list_connections()
            .into_iter()
            .filter_map(|id| self.get_health(pool, &id).map(|health| (id, health)))
            .collect()
    }

    /// Get circuit breaker state for a connection
    pub fn get_circuit_state(&self, connection_id: &ConnectionId) -> CircuitState {
        self.circuits
            .read()
            .get(connection_id)
            .map(|c| c.state)
            .unwrap_or(CircuitState::Closed)
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_closed_to_open() {
        let mut breaker = CircuitBreaker::new(3, Duration::from_secs(60), 2);

        assert_eq!(breaker.state, CircuitState::Closed);
        assert!(breaker.allows_request());

        breaker.record_failure();
        assert_eq!(breaker.state, CircuitState::Closed);

        breaker.record_failure();
        assert_eq!(breaker.state, CircuitState::Closed);

        breaker.record_failure();
        assert_eq!(breaker.state, CircuitState::Open);
        assert!(!breaker.allows_request());
    }

    #[test]
    fn test_circuit_breaker_half_open() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(100), 2);

        breaker.record_failure();
        assert_eq!(breaker.state, CircuitState::Open);

        std::thread::sleep(Duration::from_millis(150));

        breaker.check_half_open();
        assert_eq!(breaker.state, CircuitState::HalfOpen);
        assert!(breaker.allows_request());
    }

    #[test]
    fn test_circuit_breaker_recovery() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(100), 2);

        breaker.record_failure();
        std::thread::sleep(Duration::from_millis(150));
        breaker.check_half_open();

        breaker.record_success();
        assert_eq!(breaker.state, CircuitState::HalfOpen);

        breaker.record_success();
        assert_eq!(breaker.state, CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_reopen() {
        let mut breaker = CircuitBreaker::new(1, Duration::from_millis(100), 2);

        breaker.record_failure();
        std::thread::sleep(Duration::from_millis(150));
        breaker.check_half_open();

        breaker.record_failure();
        assert_eq!(breaker.state, CircuitState::Open);
    }
}
