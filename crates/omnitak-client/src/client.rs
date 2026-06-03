use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};

/// Configuration for auto-reconnect behavior
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Enable auto-reconnect
    pub enabled: bool,
    /// Initial backoff duration
    pub initial_backoff: Duration,
    /// Maximum backoff duration
    pub max_backoff: Duration,
    /// Backoff multiplier (for exponential backoff)
    pub backoff_multiplier: f64,
    /// Maximum number of reconnect attempts (None = infinite)
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            max_attempts: None,
        }
    }
}

/// Health status of a TAK client connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Connection is healthy
    Healthy,
    /// Connection is degraded but operational
    Degraded,
    /// Connection is unhealthy
    Unhealthy,
    /// Connection is disconnected
    Disconnected,
}

/// Result of a health check
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Current health status
    pub status: HealthStatus,
    /// Optional message describing the status
    pub message: Option<String>,
    /// Round-trip time (if applicable)
    pub rtt: Option<Duration>,
}

/// Configuration for TAK client connections
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Remote server address
    pub server_addr: String,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Read timeout
    pub read_timeout: Duration,
    /// Write timeout
    pub write_timeout: Duration,
    /// Auto-reconnect configuration
    pub reconnect: ReconnectConfig,
    /// Buffer size for receiving messages
    pub recv_buffer_size: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_addr: String::new(),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(10),
            reconnect: ReconnectConfig::default(),
            recv_buffer_size: 1024,
        }
    }
}

/// CoT message wrapper for transmission
#[derive(Debug, Clone)]
pub struct CotMessage {
    /// Raw CoT XML data
    pub data: Bytes,
    /// Optional message metadata
    pub metadata: Option<MessageMetadata>,
}

/// Metadata associated with a CoT message
#[derive(Debug, Clone)]
pub struct MessageMetadata {
    /// Timestamp when message was received
    pub received_at: std::time::SystemTime,
    /// Source address (if available)
    pub source_addr: Option<String>,
}

/// Async trait for TAK protocol clients
#[async_trait]
pub trait TakClient: Send + Sync {
    /// Connect to the TAK server
    ///
    /// This method establishes a connection to the configured server.
    /// If auto-reconnect is enabled, connection failures will be retried
    /// with exponential backoff.
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect from the TAK server
    ///
    /// Gracefully closes the connection and cleans up resources.
    async fn disconnect(&mut self) -> Result<()>;

    /// Send a CoT message to the server
    ///
    /// # Arguments
    /// * `message` - The CoT message to send
    ///
    /// # Returns
    /// * `Ok(())` if the message was sent successfully
    /// * `Err` if the send operation failed
    async fn send_cot(&mut self, message: CotMessage) -> Result<()>;

    /// Receive CoT messages from the server
    ///
    /// Returns a stream of CoT messages. The stream will continue until
    /// the connection is closed or an error occurs.
    ///
    /// # Returns
    /// A stream of CoT messages wrapped in Results
    fn receive_cot(&mut self) -> ReceiverStream<Result<CotMessage>>;

    /// Perform a health check on the connection
    ///
    /// # Returns
    /// Current health status of the connection
    async fn health_check(&self) -> HealthCheck;

    /// Check if the client is currently connected
    fn is_connected(&self) -> bool;
}

/// Helper function to calculate exponential backoff duration
pub fn calculate_backoff(attempt: u32, config: &ReconnectConfig) -> Duration {
    let backoff_secs =
        config.initial_backoff.as_secs_f64() * config.backoff_multiplier.powi(attempt as i32);
    let capped_secs = backoff_secs.min(config.max_backoff.as_secs_f64());
    Duration::from_secs_f64(capped_secs)
}

/// Auto-reconnect helper that wraps connection attempts with retry logic
pub async fn connect_with_retry<F, Fut>(mut connect_fn: F, config: &ReconnectConfig) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    if !config.enabled {
        return connect_fn().await;
    }

    let mut attempt = 0u32;

    loop {
        match connect_fn().await {
            Ok(()) => {
                if attempt > 0 {
                    info!(
                        attempt = attempt,
                        "Successfully reconnected after {} attempts", attempt
                    );
                }
                return Ok(());
            }
            Err(e) => {
                attempt += 1;

                if let Some(max) = config.max_attempts
                    && attempt >= max
                {
                    error!(
                        attempt = attempt,
                        error = %e,
                        "Max reconnect attempts reached"
                    );
                    return Err(e);
                }

                let backoff = calculate_backoff(attempt - 1, config);
                warn!(
                    attempt = attempt,
                    backoff_secs = backoff.as_secs(),
                    error = %e,
                    "Connection attempt failed, retrying after backoff"
                );

                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_calculation() {
        let config = ReconnectConfig {
            enabled: true,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            max_attempts: None,
        };

        assert_eq!(calculate_backoff(0, &config), Duration::from_secs(1));
        assert_eq!(calculate_backoff(1, &config), Duration::from_secs(2));
        assert_eq!(calculate_backoff(2, &config), Duration::from_secs(4));
        assert_eq!(calculate_backoff(3, &config), Duration::from_secs(8));
        assert_eq!(calculate_backoff(10, &config), Duration::from_secs(60)); // capped
    }

    #[test]
    fn test_default_config() {
        let config = ClientConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert!(config.reconnect.enabled);
    }
}
