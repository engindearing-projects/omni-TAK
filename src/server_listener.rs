//! Server Listener Infrastructure
//!
//! Provides TCP and TLS listener implementations for accepting incoming connections
//! from ATAK clients, enabling bidirectional message aggregation.
//!
//! # Architecture
//!
//! ```text
//!                    ATAK Clients
//!                         │
//!          ┌──────────────┼──────────────┐
//!          │              │              │
//!    ┌─────▼─────┐  ┌─────▼─────┐  ┌────▼──────┐
//!    │ TCP:8087  │  │ TLS:8089  │  │ TLS:8089  │
//!    └─────┬─────┘  └─────┬─────┘  └────┬──────┘
//!          │              │              │
//!    ┌─────▼──────────────▼──────────────▼─────┐
//!    │         TcpListener / TlsListener        │
//!    │  - Accept loop (SO_REUSEPORT)            │
//!    │  - Connection limiting                   │
//!    │  - Per-connection handler spawn          │
//!    └────────────────┬─────────────────────────┘
//!                     │
//!          ┌──────────┼──────────┐
//!          │          │          │
//!    ┌─────▼────┐ ┌──▼─────┐ ┌──▼─────┐
//!    │ Handler  │ │Handler │ │Handler │
//!    │ Read Task│ │ReadTask│ │ReadTask│ ← XML framing (</event>)
//!    └────┬─────┘ └────┬───┘ └────┬───┘
//!         │            │          │
//!         └────────────┼──────────┘
//!                      ▼
//!            MessageAggregator
//!                      │
//!                      ▼
//!            MessageDistributor
//!                      │
//!         ┌────────────┼────────────┐
//!         ▼            ▼            ▼
//!    Write Task   Write Task   Write Task  ← Send to clients
//! ```

use anyhow::{anyhow, Context, Result};
use bytes::BytesMut;
use omnitak_pool::{ConnectionPool, InboundMessage, MessageAggregator};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::{self, ServerConfig as TlsServerConfig};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

/// Maximum frame size for CoT messages (10MB)
const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;

/// XML end token for TAK CoT message framing
const XML_END_TOKEN: &[u8] = b"</event>";

/// Initial buffer capacity for reading
const INITIAL_BUFFER_CAPACITY: usize = 8192;

/// Listener protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenerProtocol {
    /// Plain TCP
    Tcp,
    /// TLS encrypted
    Tls,
}

/// TLS client authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientAuthConfig {
    /// Require client certificate
    pub required: bool,
    /// Path to CA certificate for client verification
    pub ca_path: String,
}

/// TLS listener configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsListenerConfig {
    /// Path to server certificate (PEM format)
    pub cert_path: String,
    /// Path to server private key (PEM format)
    pub key_path: String,
    /// Optional client authentication
    pub client_auth: Option<ClientAuthConfig>,
}

/// Listener configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    /// Unique identifier for this listener
    pub id: String,
    /// Enable this listener
    pub enabled: bool,
    /// Bind address (e.g., "0.0.0.0:8087")
    pub bind_addr: String,
    /// Protocol type
    pub protocol: ListenerProtocol,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// TLS configuration (required if protocol is TLS)
    pub tls: Option<TlsListenerConfig>,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            id: "tcp-listener-8087".to_string(),
            enabled: true,
            bind_addr: "0.0.0.0:8087".to_string(),
            protocol: ListenerProtocol::Tcp,
            max_connections: 1000,
            tls: None,
        }
    }
}

/// Listener statistics
#[derive(Debug, Clone)]
pub struct ListenerStats {
    /// Total connections accepted
    pub total_accepted: u64,
    /// Total connections rejected (due to limits)
    pub total_rejected: u64,
    /// Current active connections
    pub active_connections: usize,
    /// Total bytes received
    pub total_bytes_received: u64,
    /// Total bytes sent
    pub total_bytes_sent: u64,
    /// Total messages received
    pub total_messages_received: u64,
    /// Total messages sent
    pub total_messages_sent: u64,
}

/// Shared listener state for tracking metrics
#[derive(Debug)]
struct ListenerState {
    accepted: AtomicU64,
    rejected: AtomicU64,
    active: AtomicU64,
    bytes_received: AtomicU64,
    bytes_sent: AtomicU64,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
    shutdown: AtomicBool,
}

impl ListenerState {
    fn new() -> Self {
        Self {
            accepted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            active: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    fn stats(&self) -> ListenerStats {
        ListenerStats {
            total_accepted: self.accepted.load(Ordering::Relaxed),
            total_rejected: self.rejected.load(Ordering::Relaxed),
            active_connections: self.active.load(Ordering::Relaxed) as usize,
            total_bytes_received: self.bytes_received.load(Ordering::Relaxed),
            total_bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            total_messages_received: self.messages_received.load(Ordering::Relaxed),
            total_messages_sent: self.messages_sent.load(Ordering::Relaxed),
        }
    }
}

/// TCP Listener for accepting ATAK client connections
pub struct TcpListener {
    config: ListenerConfig,
    pool: Arc<ConnectionPool>,
    aggregator: Arc<MessageAggregator>,
    state: Arc<ListenerState>,
    accept_task: Option<JoinHandle<()>>,
}

impl TcpListener {
    /// Create a new TCP listener
    pub fn new(
        config: ListenerConfig,
        pool: Arc<ConnectionPool>,
        aggregator: Arc<MessageAggregator>,
    ) -> Self {
        Self {
            config,
            pool,
            aggregator,
            state: Arc::new(ListenerState::new()),
            accept_task: None,
        }
    }

    /// Start the listener
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            info!(
                listener_id = %self.config.id,
                "Listener disabled, not starting"
            );
            return Ok(());
        }

        let bind_addr: SocketAddr = self
            .config
            .bind_addr
            .parse()
            .context("Invalid bind address")?;

        info!(
            listener_id = %self.config.id,
            bind_addr = %bind_addr,
            protocol = "TCP",
            "Starting TCP listener"
        );

        // Create TCP listener with SO_REUSEPORT
        let listener = TokioTcpListener::bind(bind_addr).await?;

        // Enable SO_REUSEPORT for load balancing across multiple instances
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = listener.as_raw_fd();
            unsafe {
                let optval: libc::c_int = 1;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEPORT,
                    &optval as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&optval) as libc::socklen_t,
                );
            }
        }

        let pool = Arc::clone(&self.pool);
        let aggregator = Arc::clone(&self.aggregator);
        let state = Arc::clone(&self.state);
        let max_connections = self.config.max_connections;
        let listener_id = self.config.id.clone();

        // Spawn accept loop
        let accept_task = tokio::spawn(async move {
            info!(
                listener_id = %listener_id,
                "TCP accept loop started"
            );

            loop {
                // Check shutdown signal
                if state.shutdown.load(Ordering::Relaxed) {
                    info!(listener_id = %listener_id, "Shutdown signal received");
                    break;
                }

                match listener.accept().await {
                    Ok((stream, remote_addr)) => {
                        let current_active = state.active.load(Ordering::Relaxed) as usize;

                        // Check connection limit
                        if current_active >= max_connections {
                            warn!(
                                listener_id = %listener_id,
                                remote_addr = %remote_addr,
                                current = current_active,
                                max = max_connections,
                                "Connection limit reached, rejecting"
                            );
                            state.rejected.fetch_add(1, Ordering::Relaxed);
                            drop(stream);
                            continue;
                        }

                        state.accepted.fetch_add(1, Ordering::Relaxed);
                        state.active.fetch_add(1, Ordering::Relaxed);

                        info!(
                            listener_id = %listener_id,
                            remote_addr = %remote_addr,
                            active = current_active + 1,
                            "Accepted TCP connection"
                        );

                        // Spawn connection handler
                        let pool_clone = Arc::clone(&pool);
                        let aggregator_clone = Arc::clone(&aggregator);
                        let state_clone = Arc::clone(&state);
                        let listener_id_clone = listener_id.clone();

                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_connection(
                                stream,
                                remote_addr,
                                pool_clone,
                                aggregator_clone,
                                state_clone,
                                listener_id_clone,
                            )
                            .await
                            {
                                error!(
                                    remote_addr = %remote_addr,
                                    error = %e,
                                    "Connection handler error"
                                );
                            }
                        });
                    }
                    Err(e) => {
                        error!(
                            listener_id = %listener_id,
                            error = %e,
                            "Accept error"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }

            info!(listener_id = %listener_id, "TCP accept loop stopped");
        });

        self.accept_task = Some(accept_task);

        info!(
            listener_id = %self.config.id,
            bind_addr = %bind_addr,
            "TCP listener started successfully"
        );

        Ok(())
    }

    /// Handle an accepted TCP connection
    async fn handle_connection(
        stream: TcpStream,
        remote_addr: SocketAddr,
        pool: Arc<ConnectionPool>,
        aggregator: Arc<MessageAggregator>,
        state: Arc<ListenerState>,
        listener_id: String,
    ) -> Result<()> {
        // Generate unique connection ID
        let connection_id = format!("atak-client-{}", remote_addr);

        debug!(
            listener_id = %listener_id,
            connection_id = %connection_id,
            remote_addr = %remote_addr,
            "Setting up connection handler"
        );

        // Create bidirectional channels for this connection
        let (_tx, mut rx) = mpsc::channel::<Vec<u8>>(1000);

        // Register connection with pool
        pool.add_connection(
            connection_id.clone(),
            format!("ATAK Client {}", remote_addr),
            remote_addr.to_string(),
            5, // Default priority
        )
        .await?;

        // Get the connection from pool to access its channels
        let connection = pool
            .get_connection(&connection_id)
            .context("Failed to get connection from pool")?;

        // Clone for tasks
        let connection_id_read = connection_id.clone();
        let connection_id_write = connection_id.clone();
        let aggregator_sender = aggregator.sender();
        let state_read = Arc::clone(&state);
        let state_write = Arc::clone(&state);

        // Split stream for concurrent read/write
        let (mut read_half, mut write_half) = tokio::io::split(stream);

        // Spawn read task: receives CoT from client → feeds to aggregator
        let read_task = tokio::spawn(async move {
            let mut buffer = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);

            loop {
                match Self::read_xml_frame(&mut read_half, &mut buffer).await {
                    Ok(Some(frame)) => {
                        let frame_len = frame.len();
                        state_read
                            .bytes_received
                            .fetch_add(frame_len as u64, Ordering::Relaxed);
                        state_read.messages_received.fetch_add(1, Ordering::Relaxed);

                        debug!(
                            connection_id = %connection_id_read,
                            size = frame_len,
                            "Received CoT message from client"
                        );

                        // Send to aggregator
                        let inbound_msg = InboundMessage {
                            data: frame.to_vec(),
                            source: connection_id_read.clone(),
                            timestamp: Instant::now(),
                        };

                        if let Err(e) = aggregator_sender.send_async(inbound_msg).await {
                            error!(
                                connection_id = %connection_id_read,
                                error = %e,
                                "Failed to send message to aggregator"
                            );
                            break;
                        }
                    }
                    Ok(None) => {
                        info!(
                            connection_id = %connection_id_read,
                            "Client closed connection"
                        );
                        break;
                    }
                    Err(e) => {
                        error!(
                            connection_id = %connection_id_read,
                            error = %e,
                            "Read error"
                        );
                        break;
                    }
                }
            }

            debug!(connection_id = %connection_id_read, "Read task ended");
        });

        // Spawn write task: reads from connection's rx channel → sends to client
        let connection_rx = connection.rx.clone();
        let write_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Read from pool connection channel
                    msg = connection_rx.recv_async() => {
                        match msg {
                            Ok(pool_msg) => {
                                if let omnitak_pool::PoolMessage::Cot(data) = pool_msg {
                                    match write_half.write_all(&data).await {
                                        Ok(_) => {
                                            if let Err(e) = write_half.flush().await {
                                                error!(
                                                    connection_id = %connection_id_write,
                                                    error = %e,
                                                    "Flush error"
                                                );
                                                break;
                                            }
                                            state_write.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
                                            state_write.messages_sent.fetch_add(1, Ordering::Relaxed);

                                            debug!(
                                                connection_id = %connection_id_write,
                                                size = data.len(),
                                                "Sent CoT message to client"
                                            );
                                        }
                                        Err(e) => {
                                            error!(
                                                connection_id = %connection_id_write,
                                                error = %e,
                                                "Write error"
                                            );
                                            break;
                                        }
                                    }
                                } else if let omnitak_pool::PoolMessage::Shutdown = pool_msg {
                                    info!(connection_id = %connection_id_write, "Shutdown message received");
                                    break;
                                }
                            }
                            Err(_) => {
                                warn!(connection_id = %connection_id_write, "Connection channel closed");
                                break;
                            }
                        }
                    }
                    // Also listen to local mpsc channel (if needed for direct writes)
                    data = rx.recv() => {
                        if let Some(data) = data {
                            if let Err(e) = write_half.write_all(&data).await {
                                error!(
                                    connection_id = %connection_id_write,
                                    error = %e,
                                    "Write error"
                                );
                                break;
                            }
                            if let Err(e) = write_half.flush().await {
                                error!(
                                    connection_id = %connection_id_write,
                                    error = %e,
                                    "Flush error"
                                );
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }

            debug!(connection_id = %connection_id_write, "Write task ended");
        });

        // Wait for both tasks to complete
        let _ = tokio::join!(read_task, write_task);

        // Clean up
        state.active.fetch_sub(1, Ordering::Relaxed);
        if let Err(e) = pool.remove_connection(&connection_id).await {
            warn!(
                connection_id = %connection_id,
                error = %e,
                "Failed to remove connection from pool"
            );
        }

        info!(
            connection_id = %connection_id,
            remote_addr = %remote_addr,
            "Connection closed"
        );

        Ok(())
    }

    /// Read an XML-delimited frame (CoT message ending with </event>)
    async fn read_xml_frame<R>(
        stream: &mut R,
        buffer: &mut BytesMut,
    ) -> Result<Option<bytes::Bytes>>
    where
        R: AsyncReadExt + Unpin,
    {
        loop {
            // Search for </event> token in buffer
            if buffer.len() >= XML_END_TOKEN.len() {
                if let Some(pos) = buffer
                    .windows(XML_END_TOKEN.len())
                    .position(|window| window == XML_END_TOKEN)
                {
                    // Found complete message
                    let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                    let frame_bytes = frame.freeze();

                    // Validate XML starts with '<'
                    if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                        warn!("Invalid frame (not starting with '<'), skipping");
                        continue;
                    }

                    return Ok(Some(frame_bytes));
                }
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("Frame too large (> {})", MAX_FRAME_SIZE));
            }

            // Read more data
            let n = stream.read_buf(buffer).await?;

            if n == 0 {
                // Connection closed
                if buffer.is_empty() {
                    return Ok(None); // Clean close
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Stop the listener
    pub async fn stop(&mut self) -> Result<()> {
        info!(listener_id = %self.config.id, "Stopping TCP listener");

        self.state.shutdown.store(true, Ordering::Relaxed);

        if let Some(task) = self.accept_task.take() {
            task.abort();
            let _ = task.await;
        }

        info!(listener_id = %self.config.id, "TCP listener stopped");
        Ok(())
    }

    /// Get listener statistics
    pub fn stats(&self) -> ListenerStats {
        self.state.stats()
    }
}

/// TLS Listener for accepting secure ATAK client connections
pub struct TlsListener {
    config: ListenerConfig,
    pool: Arc<ConnectionPool>,
    aggregator: Arc<MessageAggregator>,
    state: Arc<ListenerState>,
    tls_acceptor: Option<TlsAcceptor>,
    accept_task: Option<JoinHandle<()>>,
}

impl TlsListener {
    /// Create a new TLS listener
    pub fn new(
        config: ListenerConfig,
        pool: Arc<ConnectionPool>,
        aggregator: Arc<MessageAggregator>,
    ) -> Result<Self> {
        // Build TLS configuration
        let tls_acceptor = if config.enabled {
            Some(Self::build_tls_acceptor(&config)?)
        } else {
            None
        };

        Ok(Self {
            config,
            pool,
            aggregator,
            state: Arc::new(ListenerState::new()),
            tls_acceptor,
            accept_task: None,
        })
    }

    /// Build TLS acceptor from configuration
    fn build_tls_acceptor(config: &ListenerConfig) -> Result<TlsAcceptor> {
        let tls_config = config
            .tls
            .as_ref()
            .ok_or_else(|| anyhow!("TLS configuration required for TLS listener"))?;

        info!(
            listener_id = %config.id,
            cert_path = %tls_config.cert_path,
            key_path = %tls_config.key_path,
            "Building TLS server configuration"
        );

        // Load server certificate and key
        let cert_file = std::fs::File::open(&tls_config.cert_path)
            .context("Failed to open server certificate")?;
        let mut cert_reader = std::io::BufReader::new(cert_file);

        let key_file = std::fs::File::open(&tls_config.key_path)
            .context("Failed to open server private key")?;
        let mut key_reader = std::io::BufReader::new(key_file);

        let certs = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse server certificate")?;

        let key = rustls_pemfile::private_key(&mut key_reader)
            .context("Failed to read private key")?
            .ok_or_else(|| anyhow!("No private key found in key file"))?;

        // Build server config
        let mut server_config = TlsServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("Invalid certificate or key")?;

        // Configure client authentication if specified
        if let Some(client_auth) = &tls_config.client_auth {
            if client_auth.required {
                info!(
                    listener_id = %config.id,
                    ca_path = %client_auth.ca_path,
                    "Configuring mutual TLS with client certificate verification"
                );

                let ca_file = std::fs::File::open(&client_auth.ca_path)
                    .context("Failed to open CA certificate")?;
                let mut ca_reader = std::io::BufReader::new(ca_file);

                let mut root_cert_store = rustls::RootCertStore::empty();
                let ca_certs = rustls_pemfile::certs(&mut ca_reader)
                    .collect::<Result<Vec<_>, _>>()
                    .context("Failed to parse CA certificate")?;

                for cert in ca_certs {
                    root_cert_store
                        .add(cert)
                        .context("Failed to add CA certificate")?;
                }

                let client_verifier =
                    rustls::server::WebPkiClientVerifier::builder(Arc::new(root_cert_store))
                        .build()
                        .context("Failed to build client verifier")?;

                // Rebuild with client auth
                server_config = TlsServerConfig::builder()
                    .with_client_cert_verifier(client_verifier)
                    .with_single_cert(
                        rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(
                            &tls_config.cert_path,
                        )?))
                        .collect::<Result<Vec<_>, _>>()?,
                        rustls_pemfile::private_key(&mut std::io::BufReader::new(
                            std::fs::File::open(&tls_config.key_path)?,
                        ))?
                        .ok_or_else(|| anyhow!("No private key found"))?,
                    )
                    .context("Failed to configure mutual TLS")?;
            }
        }

        // Set protocol versions (TLS 1.2 and 1.3)
        server_config.alpn_protocols = vec![b"http/1.1".to_vec()];

        Ok(TlsAcceptor::from(Arc::new(server_config)))
    }

    /// Start the listener
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            info!(
                listener_id = %self.config.id,
                "Listener disabled, not starting"
            );
            return Ok(());
        }

        let tls_acceptor = self
            .tls_acceptor
            .clone()
            .ok_or_else(|| anyhow!("TLS acceptor not initialized"))?;

        let bind_addr: SocketAddr = self
            .config
            .bind_addr
            .parse()
            .context("Invalid bind address")?;

        info!(
            listener_id = %self.config.id,
            bind_addr = %bind_addr,
            protocol = "TLS",
            "Starting TLS listener"
        );

        let listener = TokioTcpListener::bind(bind_addr).await?;

        // Enable SO_REUSEPORT
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = listener.as_raw_fd();
            unsafe {
                let optval: libc::c_int = 1;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEPORT,
                    &optval as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&optval) as libc::socklen_t,
                );
            }
        }

        let pool = Arc::clone(&self.pool);
        let aggregator = Arc::clone(&self.aggregator);
        let state = Arc::clone(&self.state);
        let max_connections = self.config.max_connections;
        let listener_id = self.config.id.clone();

        // Spawn accept loop
        let accept_task = tokio::spawn(async move {
            info!(listener_id = %listener_id, "TLS accept loop started");

            loop {
                if state.shutdown.load(Ordering::Relaxed) {
                    info!(listener_id = %listener_id, "Shutdown signal received");
                    break;
                }

                match listener.accept().await {
                    Ok((stream, remote_addr)) => {
                        let current_active = state.active.load(Ordering::Relaxed) as usize;

                        if current_active >= max_connections {
                            warn!(
                                listener_id = %listener_id,
                                remote_addr = %remote_addr,
                                current = current_active,
                                max = max_connections,
                                "Connection limit reached, rejecting"
                            );
                            state.rejected.fetch_add(1, Ordering::Relaxed);
                            drop(stream);
                            continue;
                        }

                        // Perform TLS handshake
                        let tls_acceptor_clone = tls_acceptor.clone();
                        let pool_clone = Arc::clone(&pool);
                        let aggregator_clone = Arc::clone(&aggregator);
                        let state_clone = Arc::clone(&state);
                        let listener_id_clone = listener_id.clone();

                        tokio::spawn(async move {
                            match tls_acceptor_clone.accept(stream).await {
                                Ok(tls_stream) => {
                                    state_clone.accepted.fetch_add(1, Ordering::Relaxed);
                                    state_clone.active.fetch_add(1, Ordering::Relaxed);

                                    info!(
                                        listener_id = %listener_id_clone,
                                        remote_addr = %remote_addr,
                                        active = current_active + 1,
                                        "Accepted TLS connection"
                                    );

                                    if let Err(e) = Self::handle_tls_connection(
                                        tls_stream,
                                        remote_addr,
                                        pool_clone,
                                        aggregator_clone,
                                        state_clone,
                                        listener_id_clone,
                                    )
                                    .await
                                    {
                                        error!(
                                            remote_addr = %remote_addr,
                                            error = %e,
                                            "TLS connection handler error"
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        listener_id = %listener_id_clone,
                                        remote_addr = %remote_addr,
                                        error = %e,
                                        "TLS handshake failed"
                                    );
                                    state_clone.rejected.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!(listener_id = %listener_id, error = %e, "Accept error");
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            }

            info!(listener_id = %listener_id, "TLS accept loop stopped");
        });

        self.accept_task = Some(accept_task);

        info!(
            listener_id = %self.config.id,
            bind_addr = %bind_addr,
            "TLS listener started successfully"
        );

        Ok(())
    }

    /// Handle an accepted TLS connection
    async fn handle_tls_connection(
        stream: tokio_rustls::server::TlsStream<TcpStream>,
        remote_addr: SocketAddr,
        pool: Arc<ConnectionPool>,
        aggregator: Arc<MessageAggregator>,
        state: Arc<ListenerState>,
        listener_id: String,
    ) -> Result<()> {
        let connection_id = format!("atak-client-tls-{}", remote_addr);

        debug!(
            listener_id = %listener_id,
            connection_id = %connection_id,
            remote_addr = %remote_addr,
            "Setting up TLS connection handler"
        );

        // Create bidirectional channels
        let (_tx, mut rx) = mpsc::channel::<Vec<u8>>(1000);

        // Register with pool
        pool.add_connection(
            connection_id.clone(),
            format!("ATAK Client TLS {}", remote_addr),
            remote_addr.to_string(),
            5,
        )
        .await?;

        let connection = pool
            .get_connection(&connection_id)
            .context("Failed to get connection from pool")?;

        let connection_id_read = connection_id.clone();
        let connection_id_write = connection_id.clone();
        let aggregator_sender = aggregator.sender();
        let state_read = Arc::clone(&state);
        let state_write = Arc::clone(&state);

        // Split TLS stream
        let (mut read_half, mut write_half) = tokio::io::split(stream);

        // Read task
        let read_task = tokio::spawn(async move {
            let mut buffer = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);

            loop {
                match Self::read_xml_frame(&mut read_half, &mut buffer).await {
                    Ok(Some(frame)) => {
                        let frame_len = frame.len();
                        state_read
                            .bytes_received
                            .fetch_add(frame_len as u64, Ordering::Relaxed);
                        state_read.messages_received.fetch_add(1, Ordering::Relaxed);

                        debug!(
                            connection_id = %connection_id_read,
                            size = frame_len,
                            "Received CoT message from TLS client"
                        );

                        let inbound_msg = InboundMessage {
                            data: frame.to_vec(),
                            source: connection_id_read.clone(),
                            timestamp: Instant::now(),
                        };

                        if let Err(e) = aggregator_sender.send_async(inbound_msg).await {
                            error!(
                                connection_id = %connection_id_read,
                                error = %e,
                                "Failed to send to aggregator"
                            );
                            break;
                        }
                    }
                    Ok(None) => {
                        info!(connection_id = %connection_id_read, "Client closed connection");
                        break;
                    }
                    Err(e) => {
                        error!(connection_id = %connection_id_read, error = %e, "Read error");
                        break;
                    }
                }
            }
        });

        // Write task
        let connection_rx = connection.rx.clone();
        let write_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = connection_rx.recv_async() => {
                        match msg {
                            Ok(pool_msg) => {
                                if let omnitak_pool::PoolMessage::Cot(data) = pool_msg {
                                    if let Err(e) = write_half.write_all(&data).await {
                                        error!(
                                            connection_id = %connection_id_write,
                                            error = %e,
                                            "Write error"
                                        );
                                        break;
                                    }
                                    if let Err(e) = write_half.flush().await {
                                        error!(
                                            connection_id = %connection_id_write,
                                            error = %e,
                                            "Flush error"
                                        );
                                        break;
                                    }
                                    state_write.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
                                    state_write.messages_sent.fetch_add(1, Ordering::Relaxed);
                                } else if let omnitak_pool::PoolMessage::Shutdown = pool_msg {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    data = rx.recv() => {
                        if let Some(data) = data {
                            if write_half.write_all(&data).await.is_err() {
                                break;
                            }
                            if write_half.flush().await.is_err() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        });

        let _ = tokio::join!(read_task, write_task);

        state.active.fetch_sub(1, Ordering::Relaxed);
        if let Err(e) = pool.remove_connection(&connection_id).await {
            warn!(connection_id = %connection_id, error = %e, "Failed to remove connection");
        }

        info!(connection_id = %connection_id, "TLS connection closed");
        Ok(())
    }

    /// Read XML frame from TLS stream
    async fn read_xml_frame<R>(
        stream: &mut R,
        buffer: &mut BytesMut,
    ) -> Result<Option<bytes::Bytes>>
    where
        R: AsyncReadExt + Unpin,
    {
        loop {
            if buffer.len() >= XML_END_TOKEN.len() {
                if let Some(pos) = buffer
                    .windows(XML_END_TOKEN.len())
                    .position(|window| window == XML_END_TOKEN)
                {
                    let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                    let frame_bytes = frame.freeze();

                    if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                        warn!("Invalid XML frame, skipping");
                        continue;
                    }

                    return Ok(Some(frame_bytes));
                }
            }

            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("Frame too large"));
            }

            let n = stream.read_buf(buffer).await?;

            if n == 0 {
                if buffer.is_empty() {
                    return Ok(None);
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Stop the listener
    pub async fn stop(&mut self) -> Result<()> {
        info!(listener_id = %self.config.id, "Stopping TLS listener");

        self.state.shutdown.store(true, Ordering::Relaxed);

        if let Some(task) = self.accept_task.take() {
            task.abort();
            let _ = task.await;
        }

        info!(listener_id = %self.config.id, "TLS listener stopped");
        Ok(())
    }

    /// Get listener statistics
    pub fn stats(&self) -> ListenerStats {
        self.state.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnitak_pool::{AggregatorConfig, DistributorConfig, MessageDistributor, PoolConfig};

    #[test]
    fn test_listener_config_default() {
        let config = ListenerConfig::default();
        assert_eq!(config.protocol, ListenerProtocol::Tcp);
        assert_eq!(config.bind_addr, "0.0.0.0:8087");
        assert_eq!(config.max_connections, 1000);
    }

    #[test]
    fn test_listener_config_tls() {
        let config = ListenerConfig {
            id: "tls-listener".to_string(),
            enabled: true,
            bind_addr: "0.0.0.0:8089".to_string(),
            protocol: ListenerProtocol::Tls,
            max_connections: 500,
            tls: Some(TlsListenerConfig {
                cert_path: "/path/to/cert.pem".to_string(),
                key_path: "/path/to/key.pem".to_string(),
                client_auth: Some(ClientAuthConfig {
                    required: true,
                    ca_path: "/path/to/ca.pem".to_string(),
                }),
            }),
        };

        assert_eq!(config.protocol, ListenerProtocol::Tls);
        assert!(config.tls.is_some());
    }

    #[tokio::test]
    async fn test_tcp_listener_creation() {
        let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
        let distributor = Arc::new(MessageDistributor::new(
            Arc::clone(&pool),
            DistributorConfig::default(),
        ));
        let aggregator = Arc::new(MessageAggregator::new(
            distributor,
            AggregatorConfig::default(),
        ));

        let config = ListenerConfig::default();
        let listener = TcpListener::new(config, pool, aggregator);

        let stats = listener.stats();
        assert_eq!(stats.total_accepted, 0);
        assert_eq!(stats.active_connections, 0);
    }
}
