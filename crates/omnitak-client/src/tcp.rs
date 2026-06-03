use crate::client::{
    ClientConfig, CotMessage, HealthCheck, HealthStatus, MessageMetadata, TakClient,
    calculate_backoff,
};
use crate::state::{ConnectionState, ConnectionStatus};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use bytes::{Buf, BytesMut};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, instrument, warn};

/// Frame delimiter for newline-delimited protocol
const NEWLINE_DELIMITER: u8 = b'\n';

/// Maximum frame size (10MB)
const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;

/// Protocol framing mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramingMode {
    /// Newline-delimited frames
    Newline,
    /// Length-prefixed frames (4-byte big-endian length header)
    LengthPrefixed,
    /// XML-delimited frames (read until '>' character for TAK CoT messages)
    Xml,
}

/// Configuration specific to TCP client
#[derive(Debug, Clone)]
pub struct TcpClientConfig {
    /// Base client configuration
    pub base: ClientConfig,
    /// Framing mode
    pub framing: FramingMode,
    /// Enable TCP keepalive
    pub keepalive: bool,
    /// TCP keepalive interval
    pub keepalive_interval: Option<std::time::Duration>,
    /// Enable Nagle's algorithm (disable TCP_NODELAY)
    pub nagle: bool,
}

impl Default for TcpClientConfig {
    fn default() -> Self {
        Self {
            base: ClientConfig::default(),
            framing: FramingMode::Newline,
            keepalive: true,
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            nagle: false,
        }
    }
}

/// TCP client for TAK server connections
pub struct TcpClient {
    config: TcpClientConfig,
    status: Arc<ConnectionStatus>,
    stream: Option<TcpStream>,
    recv_tx: Option<Sender<Result<CotMessage>>>,
    recv_rx: Option<Receiver<Result<CotMessage>>>,
    shutdown_tx: Option<Sender<()>>,
}

impl TcpClient {
    /// Create a new TCP client
    pub fn new(config: TcpClientConfig) -> Self {
        let (recv_tx, recv_rx) = mpsc::channel(config.base.recv_buffer_size);

        Self {
            config,
            status: Arc::new(ConnectionStatus::new()),
            stream: None,
            recv_tx: Some(recv_tx),
            recv_rx: Some(recv_rx),
            shutdown_tx: None,
        }
    }

    /// Get connection status
    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    /// Configure TCP socket options
    fn configure_socket(&self, stream: &TcpStream) -> Result<()> {
        // Set TCP_NODELAY (disable Nagle's algorithm)
        stream
            .set_nodelay(!self.config.nagle)
            .context("Failed to set TCP_NODELAY")?;

        // Configure keepalive
        if self.config.keepalive {
            let keepalive = socket2::TcpKeepalive::new();
            let keepalive = if let Some(interval) = self.config.keepalive_interval {
                keepalive.with_time(interval)
            } else {
                keepalive
            };

            let socket = socket2::SockRef::from(stream);
            socket
                .set_tcp_keepalive(&keepalive)
                .context("Failed to set TCP keepalive")?;
        }

        Ok(())
    }

    /// Establish TCP connection
    #[instrument(skip(self))]
    async fn establish_connection(&mut self) -> Result<()> {
        self.status.set_state(ConnectionState::Connecting);

        info!("Connecting to {}", self.config.base.server_addr);

        let stream = timeout(
            self.config.base.connect_timeout,
            TcpStream::connect(&self.config.base.server_addr),
        )
        .await
        .context("Connection timeout")?
        .context("Failed to connect")?;

        self.configure_socket(&stream)?;

        self.stream = Some(stream);
        self.status.set_state(ConnectionState::Connected);
        self.status.metrics().mark_connected();

        info!("Successfully connected to {}", self.config.base.server_addr);

        Ok(())
    }

    /// Connect to the server without starting the receive task
    /// This is useful when you want to manually manage reading and writing
    pub async fn connect_only(&mut self) -> Result<()> {
        let config = self.config.base.reconnect.clone();

        // Inline retry logic to avoid closure capture issues

        if !config.enabled {
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
        }
    }

    /// Get a mutable reference to the stream for manual reading/writing
    pub fn stream_mut(&mut self) -> Option<&mut TcpStream> {
        self.stream.as_mut()
    }

    /// Get the framing mode
    pub fn framing(&self) -> FramingMode {
        self.config.framing
    }

    /// Read a frame from the stream
    async fn read_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        match self.config.framing {
            FramingMode::Newline => self.read_newline_frame(buffer).await,
            FramingMode::LengthPrefixed => self.read_length_prefixed_frame(buffer).await,
            FramingMode::Xml => self.read_xml_frame(buffer).await,
        }
    }

    /// Read a newline-delimited frame
    async fn read_newline_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;
        loop {
            // Check if we have a complete frame in the buffer
            if let Some(pos) = buffer.iter().position(|&b| b == NEWLINE_DELIMITER) {
                let frame = buffer.split_to(pos + 1);
                let mut frame_bytes = frame.freeze();

                // Remove the newline delimiter
                if frame_bytes.last() == Some(&NEWLINE_DELIMITER) {
                    frame_bytes.truncate(frame_bytes.len() - 1);
                }

                self.status
                    .metrics()
                    .record_bytes_received(frame_bytes.len() as u64);
                return Ok(Some(frame_bytes));
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("Frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Read a length-prefixed frame
    async fn read_length_prefixed_frame(
        &mut self,
        buffer: &mut BytesMut,
    ) -> Result<Option<bytes::Bytes>> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;
        loop {
            // Check if we have at least 4 bytes for the length header
            if buffer.len() >= 4 {
                let mut length_bytes = [0u8; 4];
                length_bytes.copy_from_slice(&buffer[..4]);
                let frame_length = u32::from_be_bytes(length_bytes) as usize;

                // Validate frame length
                if frame_length > MAX_FRAME_SIZE {
                    return Err(anyhow!("Frame length {} exceeds maximum", frame_length));
                }

                // Check if we have the complete frame
                if buffer.len() >= 4 + frame_length {
                    buffer.advance(4); // Skip length header
                    let frame = buffer.split_to(frame_length).freeze();
                    self.status
                        .metrics()
                        .record_bytes_received(frame.len() as u64);
                    return Ok(Some(frame));
                }
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE + 4 {
                return Err(anyhow!("Frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Read an XML-delimited frame (for TAK CoT messages)
    /// TAK Protocol: Messages are delimited by the "</event>" token
    /// Per TAK spec: "Messages are delimited and broken apart by searching for
    /// the token '</event>' and breaking apart immediately after that token."
    pub async fn read_xml_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        const XML_END_TOKEN: &[u8] = b"</event>";

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        loop {
            // Search for the complete </event> token
            if buffer.len() >= XML_END_TOKEN.len()
                && let Some(pos) = buffer
                    .windows(XML_END_TOKEN.len())
                    .position(|window| window == XML_END_TOKEN)
            {
                // Split immediately after the </event> token
                let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                let frame_bytes = frame.freeze();

                // Validate that it looks like XML (starts with '<')
                if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                    warn!("Received data not starting with '<', skipping invalid frame");
                    continue;
                }

                self.status
                    .metrics()
                    .record_bytes_received(frame_bytes.len() as u64);
                return Ok(Some(frame_bytes));
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("XML frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete XML frame"));
                }
            }
        }
    }

    /// Write a frame to the stream (public method for direct access)
    pub async fn write_frame_direct(&mut self, data: &[u8]) -> Result<()> {
        self.write_frame(data).await
    }

    /// Write a frame to the stream
    async fn write_frame(&mut self, data: &[u8]) -> Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        match self.config.framing {
            FramingMode::Newline => {
                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;

                timeout(
                    self.config.base.write_timeout,
                    stream.write_all(&[NEWLINE_DELIMITER]),
                )
                .await
                .context("Write timeout")?
                .context("Write error")?;
            }
            FramingMode::LengthPrefixed => {
                let length = data.len() as u32;
                let length_bytes = length.to_be_bytes();

                timeout(
                    self.config.base.write_timeout,
                    stream.write_all(&length_bytes),
                )
                .await
                .context("Write timeout")?
                .context("Write error")?;

                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;
            }
            FramingMode::Xml => {
                // For XML framing, write the data as-is (should already be complete XML)
                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;
            }
        }

        stream.flush().await.context("Flush error")?;
        self.status.metrics().record_bytes_sent(data.len() as u64);

        Ok(())
    }

    /// Start background receive task
    fn start_receive_task(&mut self) {
        let mut buffer = BytesMut::with_capacity(8192);
        let status = Arc::clone(&self.status);
        let tx = self.recv_tx.as_ref().unwrap().clone();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        let framing = self.config.framing;

        // Move the stream out for the task
        if let Some(mut stream) = self.stream.take() {
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            debug!("Receive task shutting down");
                            break;
                        }
                        result = Self::read_frame_static(&mut stream, &mut buffer, &status, framing) => {
                            match result {
                                Ok(Some(frame)) => {
                                    status.metrics().record_message_received();
                                    let message = CotMessage {
                                        data: frame,
                                        metadata: Some(MessageMetadata {
                                            received_at: std::time::SystemTime::now(),
                                            source_addr: None,
                                        }),
                                    };

                                    if tx.send(Ok(message)).await.is_err() {
                                        debug!("Receive channel closed");
                                        break;
                                    }
                                }
                                Ok(None) => {
                                    info!("Connection closed by remote");
                                    status.set_state(ConnectionState::Disconnected);
                                    break;
                                }
                                Err(e) => {
                                    error!(error = %e, "Error reading frame");
                                    status.metrics().record_error();
                                    let _ = tx.send(Err(e)).await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    /// Static helper for reading frames (used in async task)
    pub async fn read_frame_static(
        stream: &mut TcpStream,
        buffer: &mut BytesMut,
        status: &ConnectionStatus,
        framing: FramingMode,
    ) -> Result<Option<bytes::Bytes>> {
        match framing {
            FramingMode::Newline => loop {
                if let Some(pos) = buffer.iter().position(|&b| b == NEWLINE_DELIMITER) {
                    let frame = buffer.split_to(pos + 1);
                    let mut frame_bytes = frame.freeze();

                    if frame_bytes.last() == Some(&NEWLINE_DELIMITER) {
                        frame_bytes.truncate(frame_bytes.len() - 1);
                    }

                    status
                        .metrics()
                        .record_bytes_received(frame_bytes.len() as u64);
                    return Ok(Some(frame_bytes));
                }

                if buffer.len() >= MAX_FRAME_SIZE {
                    return Err(anyhow!("Frame too large"));
                }

                let n = stream.read_buf(buffer).await.context("Read error")?;

                if n == 0 {
                    if buffer.is_empty() {
                        return Ok(None);
                    } else {
                        return Err(anyhow!("Connection closed with incomplete frame"));
                    }
                }
            },
            FramingMode::LengthPrefixed => {
                loop {
                    // Check if we have at least 4 bytes for the length header
                    if buffer.len() >= 4 {
                        let mut length_bytes = [0u8; 4];
                        length_bytes.copy_from_slice(&buffer[..4]);
                        let frame_length = u32::from_be_bytes(length_bytes) as usize;

                        // Validate frame length
                        if frame_length > MAX_FRAME_SIZE {
                            return Err(anyhow!("Frame length {} exceeds maximum", frame_length));
                        }

                        // Check if we have the complete frame
                        if buffer.len() >= 4 + frame_length {
                            buffer.advance(4); // Skip length header
                            let frame = buffer.split_to(frame_length).freeze();
                            status.metrics().record_bytes_received(frame.len() as u64);
                            return Ok(Some(frame));
                        }
                    }

                    // Check buffer size limit
                    if buffer.len() >= MAX_FRAME_SIZE + 4 {
                        return Err(anyhow!("Frame too large"));
                    }

                    let n = stream.read_buf(buffer).await.context("Read error")?;

                    if n == 0 {
                        if buffer.is_empty() {
                            return Ok(None);
                        } else {
                            return Err(anyhow!("Connection closed with incomplete frame"));
                        }
                    }
                }
            }
            FramingMode::Xml => {
                // TAK Protocol: Messages are delimited by searching for "</event>" token
                // Per TAK spec: "Messages are delimited and broken apart by searching for
                // the token '</event>' and breaking apart immediately after that token."
                const XML_END_TOKEN: &[u8] = b"</event>";

                loop {
                    // Search for the complete </event> token
                    if buffer.len() >= XML_END_TOKEN.len() {
                        // Look for the </event> token in the buffer
                        if let Some(pos) = buffer
                            .windows(XML_END_TOKEN.len())
                            .position(|window| window == XML_END_TOKEN)
                        {
                            // Split immediately after the </event> token
                            let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                            let frame_bytes = frame.freeze();

                            // Validate that it looks like XML (starts with '<')
                            // Most CoT messages start with <?xml but some may start directly with <event>
                            if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                                // Skip invalid data - continue reading
                                warn!(
                                    "Received data not starting with '<', skipping invalid frame"
                                );
                                continue;
                            }

                            status
                                .metrics()
                                .record_bytes_received(frame_bytes.len() as u64);
                            return Ok(Some(frame_bytes));
                        }
                    }

                    // Check buffer size limit
                    if buffer.len() >= MAX_FRAME_SIZE {
                        return Err(anyhow!("XML frame too large"));
                    }

                    let n = stream.read_buf(buffer).await.context("Read error")?;

                    if n == 0 {
                        if buffer.is_empty() {
                            return Ok(None);
                        } else {
                            return Err(anyhow!("Connection closed with incomplete XML frame"));
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl TakClient for TcpClient {
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
            self.start_receive_task();
        }

        result
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from {}", self.config.base.server_addr);

        // Signal shutdown to receive task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        // Close the stream
        if let Some(mut stream) = self.stream.take() {
            stream
                .shutdown()
                .await
                .context("Failed to shutdown stream")?;
        }

        self.status.set_state(ConnectionState::Disconnected);
        self.status.metrics().mark_disconnected();

        Ok(())
    }

    async fn send_cot(&mut self, message: CotMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(anyhow!("Not connected"));
        }

        debug!(size = message.data.len(), "Sending CoT message");

        self.write_frame(&message.data).await?;
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
                if time_since_activity > std::time::Duration::from_secs(60) {
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
            rtt: None,
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
    fn test_tcp_client_creation() {
        let config = TcpClientConfig::default();
        let client = TcpClient::new(config);
        assert!(!client.is_connected());
    }

    #[test]
    fn test_framing_mode() {
        assert_eq!(FramingMode::Newline, FramingMode::Newline);
        assert_ne!(FramingMode::Newline, FramingMode::LengthPrefixed);
    }
}
