use crate::client::{
    ClientConfig, CotMessage, HealthCheck, HealthStatus, MessageMetadata, TakClient,
    calculate_backoff,
};
use crate::state::{ConnectionState, ConnectionStatus};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::{Duration, interval, timeout};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};
use tracing::{debug, error, info, instrument, warn};

/// Configuration specific to WebSocket client
#[derive(Debug, Clone)]
pub struct WebSocketClientConfig {
    /// Base client configuration
    pub base: ClientConfig,
    /// WebSocket URL (ws:// or wss://)
    pub url: String,
    /// Enable ping/pong keepalive
    pub keepalive: bool,
    /// Keepalive ping interval
    pub ping_interval: Duration,
    /// Pong timeout (time to wait for pong response)
    pub pong_timeout: Duration,
    /// Maximum message size (in bytes)
    pub max_message_size: usize,
    /// Use binary frames instead of text frames
    pub use_binary: bool,
    /// Additional WebSocket headers
    pub headers: Vec<(String, String)>,
}

impl Default for WebSocketClientConfig {
    fn default() -> Self {
        Self {
            base: ClientConfig::default(),
            url: String::new(),
            keepalive: true,
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(10),
            max_message_size: 10 * 1024 * 1024, // 10MB
            use_binary: false,
            headers: Vec::new(),
        }
    }
}

impl WebSocketClientConfig {
    /// Create a new WebSocket client configuration
    pub fn new(url: String) -> Self {
        Self {
            url,
            ..Default::default()
        }
    }

    /// Add a custom header
    pub fn add_header(mut self, key: String, value: String) -> Self {
        self.headers.push((key, value));
        self
    }
}

/// WebSocket client for TAK server connections
pub struct WebSocketClient {
    config: WebSocketClientConfig,
    status: Arc<ConnectionStatus>,
    ws_stream: Option<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
    recv_tx: Option<Sender<Result<CotMessage>>>,
    recv_rx: Option<Receiver<Result<CotMessage>>>,
    shutdown_tx: Option<Sender<()>>,
}

impl WebSocketClient {
    /// Create a new WebSocket client
    pub fn new(config: WebSocketClientConfig) -> Self {
        let (recv_tx, recv_rx) = mpsc::channel(config.base.recv_buffer_size);

        Self {
            config,
            status: Arc::new(ConnectionStatus::new()),
            ws_stream: None,
            recv_tx: Some(recv_tx),
            recv_rx: Some(recv_rx),
            shutdown_tx: None,
        }
    }

    /// Get connection status
    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    /// Establish WebSocket connection
    #[instrument(skip(self))]
    async fn establish_connection(&mut self) -> Result<()> {
        self.status.set_state(ConnectionState::Connecting);

        info!("Connecting to WebSocket: {}", self.config.url);

        // Connect with timeout
        let (ws_stream, response) = timeout(
            self.config.base.connect_timeout,
            connect_async(&self.config.url),
        )
        .await
        .context("Connection timeout")?
        .context("Failed to connect to WebSocket")?;

        info!("WebSocket connected (status: {})", response.status());

        self.ws_stream = Some(ws_stream);
        self.status.set_state(ConnectionState::Connected);
        self.status.metrics().mark_connected();

        Ok(())
    }

    /// Start background receive and keepalive task
    fn start_tasks(&mut self) {
        let status = Arc::clone(&self.status);
        let tx = self.recv_tx.as_ref().unwrap().clone();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let keepalive_enabled = self.config.keepalive;
        let ping_interval_duration = self.config.ping_interval;
        let pong_timeout_duration = self.config.pong_timeout;

        // Move the stream out for the task
        if let Some(mut ws_stream) = self.ws_stream.take() {
            tokio::spawn(async move {
                let mut ping_interval = interval(ping_interval_duration);
                ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut waiting_for_pong = false;
                let mut pong_deadline = tokio::time::Instant::now();

                loop {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            debug!("WebSocket task shutting down");
                            let _ = ws_stream.close(Some(CloseFrame {
                                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                                reason: "Client shutdown".into(),
                            })).await;
                            break;
                        }

                        _ = ping_interval.tick(), if keepalive_enabled => {
                            if waiting_for_pong {
                                if tokio::time::Instant::now() > pong_deadline {
                                    error!("Pong timeout - connection appears dead");
                                    status.set_error("Pong timeout".to_string());
                                    break;
                                }
                            } else {
                                // Send ping
                                debug!("Sending WebSocket ping");
                                if let Err(e) = ws_stream.send(Message::Ping(vec![])).await {
                                    error!(error = %e, "Failed to send ping");
                                    status.metrics().record_error();
                                    break;
                                }
                                waiting_for_pong = true;
                                pong_deadline = tokio::time::Instant::now() + pong_timeout_duration;
                            }
                        }

                        message = ws_stream.next() => {
                            match message {
                                Some(Ok(msg)) => {
                                    match msg {
                                        Message::Text(text) => {
                                            let data = Bytes::from(text);
                                            status.metrics().record_bytes_received(data.len() as u64);
                                            status.metrics().record_message_received();

                                            let cot_message = CotMessage {
                                                data,
                                                metadata: Some(MessageMetadata {
                                                    received_at: std::time::SystemTime::now(),
                                                    source_addr: None,
                                                }),
                                            };

                                            if tx.send(Ok(cot_message)).await.is_err() {
                                                debug!("Receive channel closed");
                                                break;
                                            }
                                        }
                                        Message::Binary(data) => {
                                            let bytes = Bytes::from(data);
                                            status.metrics().record_bytes_received(bytes.len() as u64);
                                            status.metrics().record_message_received();

                                            let cot_message = CotMessage {
                                                data: bytes,
                                                metadata: Some(MessageMetadata {
                                                    received_at: std::time::SystemTime::now(),
                                                    source_addr: None,
                                                }),
                                            };

                                            if tx.send(Ok(cot_message)).await.is_err() {
                                                debug!("Receive channel closed");
                                                break;
                                            }
                                        }
                                        Message::Ping(payload) => {
                                            debug!("Received ping, sending pong");
                                            if let Err(e) = ws_stream.send(Message::Pong(payload)).await {
                                                error!(error = %e, "Failed to send pong");
                                                break;
                                            }
                                        }
                                        Message::Pong(_) => {
                                            debug!("Received pong");
                                            waiting_for_pong = false;
                                        }
                                        Message::Close(frame) => {
                                            info!("WebSocket closed by remote: {:?}", frame);
                                            status.set_state(ConnectionState::Disconnected);
                                            break;
                                        }
                                        Message::Frame(_) => {
                                            // Raw frames should not be received in this mode
                                            warn!("Received unexpected raw frame");
                                        }
                                    }
                                }
                                Some(Err(e)) => {
                                    error!(error = %e, "WebSocket error");
                                    status.metrics().record_error();
                                    let _ = tx.send(Err(e.into())).await;
                                    break;
                                }
                                None => {
                                    info!("WebSocket connection closed");
                                    status.set_state(ConnectionState::Disconnected);
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    /// Send a WebSocket message
    async fn send_message(&mut self, data: Bytes) -> Result<()> {
        let stream = self
            .ws_stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        let message = if self.config.use_binary {
            Message::Binary(data.to_vec())
        } else {
            // Try to convert to text, fall back to binary if invalid UTF-8
            match String::from_utf8(data.to_vec()) {
                Ok(text) => Message::Text(text),
                Err(_) => {
                    warn!("Data is not valid UTF-8, sending as binary");
                    Message::Binary(data.to_vec())
                }
            }
        };

        timeout(self.config.base.write_timeout, stream.send(message))
            .await
            .context("Send timeout")?
            .context("Failed to send message")?;

        self.status.metrics().record_bytes_sent(data.len() as u64);

        Ok(())
    }
}

#[async_trait]
impl TakClient for WebSocketClient {
    async fn connect(&mut self) -> Result<()> {
        let config = self.config.base.reconnect.clone();

        // Inline retry logic to avoid closure capture issues
        let result = if !config.enabled {
            self.establish_connection().await
        } else {
            let mut attempt = 0u32;
            loop {
                match self.establish_connection().await {
                    Ok(()) => {
                        if attempt > 0 {
                            info!(
                                attempt = attempt,
                                "Successfully reconnected after {} attempts", attempt
                            );
                        }
                        break Ok(());
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
                            break Err(e);
                        }

                        let backoff = calculate_backoff(attempt - 1, &config);
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
        };

        if result.is_ok() {
            self.start_tasks();
        }

        result
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from WebSocket: {}", self.config.url);

        // Signal shutdown to task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        // Close the WebSocket stream
        if let Some(mut stream) = self.ws_stream.take() {
            let _ = timeout(
                Duration::from_secs(5),
                stream.close(Some(CloseFrame {
                    code:
                        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                    reason: "Client disconnect".into(),
                })),
            )
            .await;
        }

        self.status.set_state(ConnectionState::Disconnected);
        self.status.metrics().mark_disconnected();

        Ok(())
    }

    async fn send_cot(&mut self, message: CotMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(anyhow!("Not connected"));
        }

        // Check message size
        if message.data.len() > self.config.max_message_size {
            return Err(anyhow!(
                "Message size {} exceeds maximum {}",
                message.data.len(),
                self.config.max_message_size
            ));
        }

        debug!(
            size = message.data.len(),
            binary = self.config.use_binary,
            "Sending CoT message via WebSocket"
        );

        self.send_message(message.data).await?;
        self.status.metrics().record_message_sent();

        Ok(())
    }

    fn receive_cot(&mut self) -> ReceiverStream<Result<CotMessage>> {
        let rx = self.recv_rx.take().unwrap_or_else(|| {
            let (_, rx) = mpsc::channel(1);
            rx
        });

        ReceiverStream::new(rx)
    }

    async fn health_check(&self) -> HealthCheck {
        let status = self.status.state();

        let health_status = match status {
            ConnectionState::Connected => {
                let time_since_activity = self.status.metrics().time_since_last_activity();
                if time_since_activity > Duration::from_secs(60) {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Healthy
                }
            }
            ConnectionState::Connecting | ConnectionState::Reconnecting => HealthStatus::Degraded,
            ConnectionState::Disconnected => HealthStatus::Disconnected,
            ConnectionState::Failed => HealthStatus::Unhealthy,
        };

        HealthCheck {
            status: health_status,
            message: self.status.error_message(),
            rtt: None, // Could measure via ping/pong latency
        }
    }

    fn is_connected(&self) -> bool {
        self.status.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_config_creation() {
        let config = WebSocketClientConfig::new("wss://example.com/tak".to_string());
        assert_eq!(config.url, "wss://example.com/tak");
        assert!(config.keepalive);
    }

    #[test]
    fn test_websocket_config_headers() {
        let config = WebSocketClientConfig::new("wss://example.com".to_string())
            .add_header("Authorization".to_string(), "Bearer token".to_string());

        assert_eq!(config.headers.len(), 1);
        assert_eq!(config.headers[0].0, "Authorization");
    }

    #[test]
    fn test_websocket_client_creation() {
        let config = WebSocketClientConfig::new("wss://example.com/tak".to_string());
        let client = WebSocketClient::new(config);
        assert!(!client.is_connected());
    }
}
