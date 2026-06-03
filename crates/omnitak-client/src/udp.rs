use crate::client::{
    ClientConfig, CotMessage, HealthCheck, HealthStatus, MessageMetadata, TakClient,
};
use crate::state::{ConnectionState, ConnectionStatus};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use bytes::Bytes;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, instrument, warn};

/// Maximum UDP packet size (standard MTU - headers)
const MAX_UDP_PACKET_SIZE: usize = 1472; // 1500 MTU - 20 IP header - 8 UDP header

/// Configuration specific to UDP client
#[derive(Debug, Clone)]
pub struct UdpClientConfig {
    /// Base client configuration
    pub base: ClientConfig,
    /// Local bind address (None = automatic)
    pub local_addr: Option<SocketAddr>,
    /// Enable multicast support
    pub multicast: bool,
    /// Multicast interface address (for joining multicast groups)
    pub multicast_interface: Option<IpAddr>,
    /// Multicast TTL
    pub multicast_ttl: u32,
    /// Buffer size for receiving packets
    pub recv_buffer_size: usize,
}

impl Default for UdpClientConfig {
    fn default() -> Self {
        Self {
            base: ClientConfig::default(),
            local_addr: None,
            multicast: false,
            multicast_interface: None,
            multicast_ttl: 1,
            recv_buffer_size: 65536,
        }
    }
}

/// UDP client for TAK server connections
///
/// Note: UDP is connectionless, so "connection" refers to socket binding
/// and configuration, not an actual connection state.
pub struct UdpClient {
    config: UdpClientConfig,
    status: Arc<ConnectionStatus>,
    socket: Option<Arc<UdpSocket>>,
    remote_addr: Option<SocketAddr>,
    recv_tx: Option<Sender<Result<CotMessage>>>,
    recv_rx: Option<Receiver<Result<CotMessage>>>,
    shutdown_tx: Option<Sender<()>>,
}

impl UdpClient {
    /// Create a new UDP client
    pub fn new(config: UdpClientConfig) -> Self {
        let (recv_tx, recv_rx) = mpsc::channel(config.base.recv_buffer_size);

        Self {
            config,
            status: Arc::new(ConnectionStatus::new()),
            socket: None,
            remote_addr: None,
            recv_tx: Some(recv_tx),
            recv_rx: Some(recv_rx),
            shutdown_tx: None,
        }
    }

    /// Get connection status
    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    /// Parse remote address from configuration
    fn parse_remote_addr(&self) -> Result<SocketAddr> {
        self.config
            .base
            .server_addr
            .parse()
            .context("Failed to parse server address")
    }

    /// Establish UDP "connection" (bind socket and configure)
    #[instrument(skip(self))]
    async fn establish_connection(&mut self) -> Result<()> {
        self.status.set_state(ConnectionState::Connecting);

        let remote_addr = self.parse_remote_addr()?;
        self.remote_addr = Some(remote_addr);

        // Determine local bind address
        let local_addr = self
            .config
            .local_addr
            .unwrap_or_else(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0));

        info!("Binding UDP socket to {}", local_addr);

        // Create socket with socket2 to configure buffer sizes
        let socket2 = socket2::Socket::new(
            if local_addr.is_ipv4() {
                socket2::Domain::IPV4
            } else {
                socket2::Domain::IPV6
            },
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .context("Failed to create UDP socket")?;

        // Set socket buffer sizes (best-effort, ignore errors)
        let _ = socket2.set_recv_buffer_size(self.config.recv_buffer_size);

        // Set socket to non-blocking for tokio
        socket2.set_nonblocking(true)?;
        socket2.bind(&local_addr.into())?;

        // Convert to tokio UdpSocket
        let socket: UdpSocket = UdpSocket::from_std(socket2.into())?;

        // Configure multicast if enabled
        if self.config.multicast {
            self.configure_multicast(&socket, &remote_addr)?;
        }

        self.socket = Some(Arc::new(socket));
        self.status.set_state(ConnectionState::Connected);
        self.status.metrics().mark_connected();

        info!(
            "UDP socket bound to {} (remote: {})",
            local_addr, remote_addr
        );

        Ok(())
    }

    /// Configure multicast options
    fn configure_multicast(&self, socket: &UdpSocket, remote_addr: &SocketAddr) -> Result<()> {
        info!("Configuring multicast for {}", remote_addr);

        match remote_addr.ip() {
            IpAddr::V4(multicast_addr) => {
                let interface = self
                    .config
                    .multicast_interface
                    .and_then(|ip| match ip {
                        IpAddr::V4(v4) => Some(v4),
                        _ => None,
                    })
                    .unwrap_or(Ipv4Addr::UNSPECIFIED);

                socket
                    .join_multicast_v4(multicast_addr, interface)
                    .context("Failed to join multicast group")?;

                socket
                    .set_multicast_ttl_v4(self.config.multicast_ttl)
                    .context("Failed to set multicast TTL")?;

                info!("Joined IPv4 multicast group {}", multicast_addr);
            }
            IpAddr::V6(multicast_addr) => {
                socket
                    .join_multicast_v6(&multicast_addr, 0)
                    .context("Failed to join IPv6 multicast group")?;

                info!("Joined IPv6 multicast group {}", multicast_addr);
            }
        }

        Ok(())
    }

    /// Start background receive task
    fn start_receive_task(&mut self) {
        let socket = Arc::clone(self.socket.as_ref().unwrap());
        let status = Arc::clone(&self.status);
        let tx = self.recv_tx.as_ref().unwrap().clone();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        let read_timeout = self.config.base.read_timeout;

        tokio::spawn(async move {
            let mut buffer = vec![0u8; MAX_UDP_PACKET_SIZE];

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        debug!("UDP receive task shutting down");
                        break;
                    }
                    result = timeout(read_timeout, socket.recv_from(&mut buffer)) => {
                        match result {
                            Ok(Ok((size, source_addr))) => {
                                if size > 0 {
                                    let data = Bytes::copy_from_slice(&buffer[..size]);
                                    status.metrics().record_bytes_received(size as u64);
                                    status.metrics().record_message_received();

                                    let message = CotMessage {
                                        data,
                                        metadata: Some(MessageMetadata {
                                            received_at: std::time::SystemTime::now(),
                                            source_addr: Some(source_addr.to_string()),
                                        }),
                                    };

                                    if tx.send(Ok(message)).await.is_err() {
                                        debug!("Receive channel closed");
                                        break;
                                    }
                                }
                            }
                            Ok(Err(e)) => {
                                error!(error = %e, "Error receiving UDP packet");
                                status.metrics().record_error();
                                let _ = tx.send(Err(e.into())).await;
                            }
                            Err(_) => {
                                // Timeout - this is normal for UDP, just continue
                                continue;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Send a datagram to the remote address
    async fn send_datagram(&self, data: &[u8]) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| anyhow!("Socket not bound"))?;

        let remote_addr = self
            .remote_addr
            .ok_or_else(|| anyhow!("Remote address not set"))?;

        // Check packet size
        if data.len() > MAX_UDP_PACKET_SIZE {
            warn!(
                size = data.len(),
                max_size = MAX_UDP_PACKET_SIZE,
                "UDP packet exceeds recommended size, may be fragmented or dropped"
            );
        }

        let sent = timeout(
            self.config.base.write_timeout,
            socket.send_to(data, remote_addr),
        )
        .await
        .context("Send timeout")?
        .context("Send error")?;

        if sent != data.len() {
            warn!(
                expected = data.len(),
                actual = sent,
                "Partial UDP packet sent"
            );
        }

        self.status.metrics().record_bytes_sent(sent as u64);

        Ok(())
    }
}

#[async_trait]
impl TakClient for UdpClient {
    async fn connect(&mut self) -> Result<()> {
        // UDP doesn't have traditional connection, just bind and configure
        self.establish_connection().await?;
        self.start_receive_task();
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Closing UDP socket");

        // Signal shutdown to receive task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        // Leave multicast groups if applicable
        if self.config.multicast
            && let (Some(socket), Some(remote_addr)) = (&self.socket, &self.remote_addr)
        {
            match remote_addr.ip() {
                IpAddr::V4(multicast_addr) => {
                    let interface = self
                        .config
                        .multicast_interface
                        .and_then(|ip| match ip {
                            IpAddr::V4(v4) => Some(v4),
                            _ => None,
                        })
                        .unwrap_or(Ipv4Addr::UNSPECIFIED);

                    let _ = socket.leave_multicast_v4(multicast_addr, interface);
                }
                IpAddr::V6(multicast_addr) => {
                    let _ = socket.leave_multicast_v6(&multicast_addr, 0);
                }
            }
        }

        self.socket = None;
        self.remote_addr = None;
        self.status.set_state(ConnectionState::Disconnected);
        self.status.metrics().mark_disconnected();

        Ok(())
    }

    async fn send_cot(&mut self, message: CotMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(anyhow!("Socket not bound"));
        }

        debug!(size = message.data.len(), "Sending CoT message via UDP");

        // Handle fragmentation for large messages
        if message.data.len() > MAX_UDP_PACKET_SIZE {
            warn!(
                size = message.data.len(),
                "CoT message exceeds UDP packet size, may be lost"
            );
        }

        self.send_datagram(&message.data).await?;
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
                // For UDP, check if socket is still valid
                if self.socket.is_some() {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                }
            }
            ConnectionState::Connecting | ConnectionState::Reconnecting => HealthStatus::Degraded,
            ConnectionState::Disconnected => HealthStatus::Disconnected,
            ConnectionState::Failed => HealthStatus::Unhealthy,
        };

        HealthCheck {
            status: health_status,
            message: self.status.error_message(),
            rtt: None, // UDP doesn't have built-in RTT measurement
        }
    }

    fn is_connected(&self) -> bool {
        self.status.is_connected() && self.socket.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_udp_client_creation() {
        let config = UdpClientConfig::default();
        let client = UdpClient::new(config);
        assert!(!client.is_connected());
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // intentional invariant check on the constant
    fn test_max_packet_size() {
        assert!(MAX_UDP_PACKET_SIZE <= 1500);
        assert!(MAX_UDP_PACKET_SIZE >= 1400);
    }

    #[test]
    fn test_multicast_config() {
        let config = UdpClientConfig {
            multicast: true,
            multicast_ttl: 5,
            ..Default::default()
        };
        assert_eq!(config.multicast_ttl, 5);
    }
}
