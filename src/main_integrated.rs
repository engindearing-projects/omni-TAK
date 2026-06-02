mod server_listener;

use anyhow::{Context, Result};
use clap::Parser;
use omnitak_api::{ServerBuilder, ServerConfig};
use omnitak_client::{
    tcp::{TcpClient, TcpClientConfig},
    tls::{TlsClient, TlsClientConfig},
    Bytes, CotMessage, TakClient,
};
use omnitak_pool::{
    AggregatorConfig, ConnectionPool, DistributorConfig, FilterRule, HealthMonitor, InboundMessage,
    MessageAggregator, MessageDistributor, PoolConfig, PoolMessage,
};
use serde::Deserialize;
use server_listener::{
    ClientAuthConfig as ServerClientAuthConfig, ListenerConfig as ServerListenerConfig,
    ListenerProtocol as ServerListenerProtocol, TcpListener as ServerTcpListener,
    TlsListener as ServerTlsListener, TlsListenerConfig as ServerTlsListenerConfig,
};
use std::collections::HashSet;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::signal;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};

/// OmniTAK - High-performance TAK aggregator and message broker
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/config.yaml")]
    config: PathBuf,

    /// Override bind address
    #[arg(short, long)]
    bind: Option<SocketAddr>,

    /// Admin username for initial setup
    #[arg(long, default_value = "admin")]
    admin_user: String,

    /// Admin password for initial setup
    #[arg(long, env = "OMNITAK_ADMIN_PASSWORD", default_value = "changeme")]
    admin_password: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    api: ApiConfig,
    #[serde(default)]
    servers: Vec<TakServerDef>,
    #[serde(default)]
    listeners: Vec<ListenerConfig>,
}

#[derive(Debug, Deserialize)]
struct ApiConfig {
    #[serde(default = "default_bind_addr")]
    bind_addr: String,
    #[serde(default = "default_enable_tls")]
    enable_tls: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct TakServerDef {
    id: String,
    address: String,
    protocol: String,
    tls: Option<TlsConfigDef>,
}

#[derive(Debug, Deserialize, Clone)]
struct TlsConfigDef {
    cert_path: String,
    key_path: String,
    ca_path: String,
    #[serde(default = "default_verify_server")]
    verify_server: bool,
}

fn default_verify_server() -> bool {
    true
}

/// Listener protocol type
#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ListenerProtocol {
    Tcp,
    Tls,
}

/// Client authentication configuration for TLS listeners
#[derive(Debug, Deserialize, Clone)]
struct ClientAuthConfig {
    #[serde(default)]
    required: bool,
    ca_path: Option<String>,
}

/// TLS configuration for listeners
#[derive(Debug, Deserialize, Clone)]
struct ListenerTlsConfig {
    cert_path: String,
    key_path: String,
    #[serde(default)]
    client_auth: Option<ClientAuthConfig>,
}

/// Listener configuration for incoming TAK connections
#[derive(Debug, Deserialize, Clone)]
struct ListenerConfig {
    id: String,
    #[serde(default = "default_listener_enabled")]
    enabled: bool,
    bind_addr: String,
    protocol: ListenerProtocol,
    #[serde(default = "default_max_connections")]
    max_connections: usize,
    #[serde(default)]
    tls: Option<ListenerTlsConfig>,
}

fn default_listener_enabled() -> bool {
    true
}

fn default_max_connections() -> usize {
    100
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            enable_tls: default_enable_tls(),
        }
    }
}

fn default_bind_addr() -> String {
    "0.0.0.0:8443".to_string()
}

fn default_enable_tls() -> bool {
    false
}

/// Validates listener configuration
fn validate_listeners(listeners: &[ListenerConfig]) -> Result<()> {
    // Check if no listeners are enabled
    let enabled_count = listeners.iter().filter(|l| l.enabled).count();
    if enabled_count == 0 && !listeners.is_empty() {
        warn!("No listeners are enabled in configuration");
    }

    // Check for unique listener IDs
    let mut seen_ids = HashSet::new();
    for listener in listeners {
        if !seen_ids.insert(&listener.id) {
            anyhow::bail!("Duplicate listener ID found: {}", listener.id);
        }
    }

    // Check for port conflicts between listeners
    let mut seen_ports = HashSet::new();
    for listener in listeners.iter().filter(|l| l.enabled) {
        // Parse bind address to extract port
        let addr: SocketAddr = listener.bind_addr.parse().with_context(|| {
            format!(
                "Invalid bind address for listener '{}': {}",
                listener.id, listener.bind_addr
            )
        })?;

        let port = addr.port();
        if !seen_ports.insert(port) {
            anyhow::bail!(
                "Port conflict detected: listener '{}' attempts to bind to port {} which is already in use by another listener",
                listener.id,
                port
            );
        }
    }

    // Validate TLS configuration
    for listener in listeners.iter().filter(|l| l.enabled) {
        if listener.protocol == ListenerProtocol::Tls {
            // Check that TLS config is provided
            let tls_config = listener.tls.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Listener '{}' specifies TLS protocol but missing 'tls' configuration section",
                    listener.id
                )
            })?;

            // Validate TLS certificate paths exist
            validate_file_exists(
                &tls_config.cert_path,
                &format!("TLS certificate for listener '{}'", listener.id),
            )?;
            validate_file_exists(
                &tls_config.key_path,
                &format!("TLS key for listener '{}'", listener.id),
            )?;

            // Validate client auth CA if required
            if let Some(ref client_auth) = tls_config.client_auth {
                if client_auth.required {
                    if let Some(ref ca_path) = client_auth.ca_path {
                        validate_file_exists(
                            ca_path,
                            &format!("Client CA certificate for listener '{}'", listener.id),
                        )?;
                    } else {
                        anyhow::bail!(
                            "Listener '{}' requires client authentication but 'ca_path' is not specified",
                            listener.id
                        );
                    }
                }
            }
        } else if listener.tls.is_some() {
            warn!(
                "Listener '{}' has TLS configuration but protocol is not 'tls' - TLS config will be ignored",
                listener.id
            );
        }

        // Validate max_connections is reasonable
        if listener.max_connections == 0 {
            anyhow::bail!(
                "Listener '{}' has max_connections set to 0 - must be at least 1",
                listener.id
            );
        }
        if listener.max_connections > 10000 {
            warn!(
                "Listener '{}' has very high max_connections ({}). This may impact system resources.",
                listener.id, listener.max_connections
            );
        }
    }

    Ok(())
}

/// Helper function to validate that a file exists
fn validate_file_exists(path: &str, description: &str) -> Result<()> {
    let path_buf = PathBuf::from(path);
    if !Path::new(&path_buf).exists() {
        anyhow::bail!("{} file not found: {}", description, path);
    }
    Ok(())
}

/// Convert config listener to server_listener format
fn convert_listener_config(config: &ListenerConfig) -> ServerListenerConfig {
    ServerListenerConfig {
        id: config.id.clone(),
        enabled: config.enabled,
        bind_addr: config.bind_addr.clone(),
        protocol: match config.protocol {
            ListenerProtocol::Tcp => ServerListenerProtocol::Tcp,
            ListenerProtocol::Tls => ServerListenerProtocol::Tls,
        },
        max_connections: config.max_connections,
        tls: config.tls.as_ref().map(|tls| ServerTlsListenerConfig {
            cert_path: tls.cert_path.clone(),
            key_path: tls.key_path.clone(),
            client_auth: tls.client_auth.as_ref().map(|ca| ServerClientAuthConfig {
                required: ca.required,
                ca_path: ca.ca_path.clone().unwrap_or_default(),
            }),
        }),
    }
}

/// Connection metrics for monitoring
#[derive(Debug, Clone)]
struct ConnectionMetrics {
    messages_received: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl ConnectionMetrics {
    fn new() -> Self {
        Self {
            messages_received: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
        }
    }

    fn record_message(&self, bytes: usize) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.messages_received.load(Ordering::Relaxed),
            self.bytes_received.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
        )
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (ignore if already initialized)
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    // Parse command line arguments
    let args = Args::parse();

    // Load configuration file
    let config_content = fs::read_to_string(&args.config)
        .with_context(|| format!("Failed to read config file: {:?}", args.config))?;

    let config: Config =
        serde_yaml::from_str(&config_content).context("Failed to parse config file")?;

    // Validate listener configuration
    validate_listeners(&config.listeners)?;

    // Log listener configuration
    let enabled_listeners: Vec<_> = config.listeners.iter().filter(|l| l.enabled).collect();
    if !enabled_listeners.is_empty() {
        info!("Configured {} listener(s):", enabled_listeners.len());
        for listener in &enabled_listeners {
            info!(
                "  - {} [{}] on {} (max: {} connections)",
                listener.id,
                match listener.protocol {
                    ListenerProtocol::Tcp => "TCP",
                    ListenerProtocol::Tls => "TLS",
                },
                listener.bind_addr,
                listener.max_connections
            );
        }
    } else {
        info!("No listeners configured - running in client-only mode");
    }

    // Build server configuration
    let bind_addr: SocketAddr = args.bind.unwrap_or_else(|| {
        config
            .api
            .bind_addr
            .parse()
            .expect("Invalid bind address in config")
    });

    let server_config = ServerConfig {
        bind_addr,
        enable_tls: config.api.enable_tls,
        tls_cert_path: None,
        tls_key_path: None,
        auth_config: Default::default(),
        rate_limit_rps: 100,
        enable_swagger: true,
        enable_static_files: true,
        enable_enrollment: true,
        enrollment_ca_cert_path: None,
        enrollment_ca_key_path: None,
        enrollment_server_config: None,
    };

    // ═══════════════════════════════════════════════════════════════════════════
    // PHASE 1: Initialize ConnectionPool Infrastructure
    // ═══════════════════════════════════════════════════════════════════════════
    info!("Initializing connection pool infrastructure...");

    // Create connection pool
    let pool_config = PoolConfig {
        max_connections: 1000,
        channel_capacity: 1000,
        health_check_interval: Duration::from_secs(30),
        inactive_timeout: Duration::from_secs(300),
        auto_reconnect: true,
    };
    let pool = Arc::new(ConnectionPool::new(pool_config));
    info!("Connection pool initialized (max: 1000 connections)");

    // Create message distributor with 16 worker threads
    let distributor_config = DistributorConfig {
        channel_capacity: 10_000,
        strategy: omnitak_pool::DistributionStrategy::DropOnFull,
        max_workers: 16,
        batch_size: 100,
        flush_interval: Duration::from_millis(10),
    };
    let distributor = Arc::new(MessageDistributor::new(
        Arc::clone(&pool),
        distributor_config,
    ));
    distributor.start().await;
    info!("Message distributor started (16 workers)");

    // Create message aggregator with deduplication
    let aggregator_config = AggregatorConfig {
        dedup_window: Duration::from_secs(60),
        max_cache_entries: 100_000,
        cleanup_interval: Duration::from_secs(10),
        channel_capacity: 10_000,
        worker_count: 4,
    };
    let aggregator = Arc::new(MessageAggregator::new(
        Arc::clone(&distributor),
        aggregator_config,
    ));
    aggregator.start().await;
    info!("Message aggregator started (60s dedup window, 4 workers)");

    // Create health monitor
    let health_monitor = Arc::new(HealthMonitor::new());
    health_monitor.start(Arc::clone(&pool));
    info!("Health monitor started");

    // Set default filter rule: broadcast all messages to all connections
    // (The distributor already has source filtering to prevent loops)
    info!("Message distribution configured with loop prevention");

    // ═══════════════════════════════════════════════════════════════════════════
    // PHASE 2: Start Server Listeners for ATAK Client Connections
    // ═══════════════════════════════════════════════════════════════════════════
    let mut tcp_listeners: Vec<ServerTcpListener> = Vec::new();
    let mut tls_listeners: Vec<ServerTlsListener> = Vec::new();

    for listener_config in &config.listeners {
        if !listener_config.enabled {
            info!("Listener '{}' is disabled, skipping", listener_config.id);
            continue;
        }

        let server_config = convert_listener_config(listener_config);

        match listener_config.protocol {
            ListenerProtocol::Tcp => {
                info!(
                    "Starting TCP listener '{}' on {}",
                    listener_config.id, listener_config.bind_addr
                );

                let mut tcp_listener = ServerTcpListener::new(
                    server_config,
                    Arc::clone(&pool),
                    Arc::clone(&aggregator),
                );

                match tcp_listener.start().await {
                    Ok(_) => {
                        info!("TCP listener '{}' started successfully", listener_config.id);
                        tcp_listeners.push(tcp_listener);
                    }
                    Err(e) => {
                        error!(
                            "Failed to start TCP listener '{}': {}",
                            listener_config.id, e
                        );
                        // Continue with other listeners instead of failing completely
                    }
                }
            }
            ListenerProtocol::Tls => {
                info!(
                    "Starting TLS listener '{}' on {}",
                    listener_config.id, listener_config.bind_addr
                );

                match ServerTlsListener::new(
                    server_config,
                    Arc::clone(&pool),
                    Arc::clone(&aggregator),
                ) {
                    Ok(mut tls_listener) => match tls_listener.start().await {
                        Ok(_) => {
                            info!("TLS listener '{}' started successfully", listener_config.id);
                            tls_listeners.push(tls_listener);
                        }
                        Err(e) => {
                            error!(
                                "Failed to start TLS listener '{}': {}",
                                listener_config.id, e
                            );
                        }
                    },
                    Err(e) => {
                        error!(
                            "Failed to create TLS listener '{}': {}",
                            listener_config.id, e
                        );
                    }
                }
            }
        }
    }

    info!(
        "Started {} listener(s) ({} TCP, {} TLS)",
        tcp_listeners.len() + tls_listeners.len(),
        tcp_listeners.len(),
        tls_listeners.len()
    );

    // ═══════════════════════════════════════════════════════════════════════════
    // PHASE 3: Start TAK Server Connections with Bidirectional Flow
    // ═══════════════════════════════════════════════════════════════════════════
    info!("Starting TAK server connections...");
    let global_metrics = Arc::new(ConnectionMetrics::new());

    // Start periodic stats reporter (with listener stats)
    let metrics_clone = global_metrics.clone();
    let pool_clone = Arc::clone(&pool);
    let aggregator_clone = Arc::clone(&aggregator);

    // Create vectors to hold listener references for stats
    let tcp_listener_count = tcp_listeners.len();
    let tls_listener_count = tls_listeners.len();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        let mut last_messages = 0u64;
        let mut last_bytes = 0u64;

        loop {
            interval.tick().await;
            let (messages, bytes, errors) = metrics_clone.snapshot();
            let msg_delta = messages.saturating_sub(last_messages);
            let bytes_delta = bytes.saturating_sub(last_bytes);

            // Get pool statistics
            let pool_stats = pool_clone.stats();
            let (dedup_entries, _) = aggregator_clone.cache_stats();

            if msg_delta > 0 || errors > 0 {
                info!(
                    "Stats: {} msgs (+{}, {:.1} msg/s), {:.2} KB (+{:.2} KB), {} errors | Pool: {} conns ({} active) | Listeners: {} active ({} TCP, {} TLS) | Dedup: {} cached",
                    messages,
                    msg_delta,
                    msg_delta as f64 / 30.0,
                    bytes as f64 / 1024.0,
                    bytes_delta as f64 / 1024.0,
                    errors,
                    pool_stats.total_connections,
                    pool_stats.active_connections,
                    tcp_listener_count + tls_listener_count,
                    tcp_listener_count,
                    tls_listener_count,
                    dedup_entries
                );
            } else {
                info!(
                    "Stats: {} msgs total, {:.2} KB total, {} errors | No activity in last 30s | Pool: {} conns | Listeners: {} | Dedup: {} cached",
                    messages,
                    bytes as f64 / 1024.0,
                    errors,
                    pool_stats.total_connections,
                    tcp_listener_count + tls_listener_count,
                    dedup_entries
                );
            }

            last_messages = messages;
            last_bytes = bytes;
        }
    });

    for server_def in &config.servers {
        info!(
            "Connecting to TAK server: {} at {}",
            server_def.id, server_def.address
        );

        if server_def.protocol.to_lowercase() == "tcp" {
            // Create TCP client
            let mut client_config = TcpClientConfig::default();
            client_config.base.server_addr = server_def.address.clone();

            // Clone for the async task
            let address = server_def.address.clone();
            let server_id = server_def.id.clone();
            let metrics = global_metrics.clone();
            let pool_clone = Arc::clone(&pool);
            let aggregator_clone = Arc::clone(&aggregator);
            let distributor_clone = Arc::clone(&distributor);

            let mut client = TcpClient::new(client_config);
            tokio::spawn(async move {
                info!("Connecting TCP client to {} ({})", address, server_id);
                if let Err(e) = client.connect().await {
                    error!("Failed to connect to TAK server {}: {}", server_id, e);
                } else {
                    info!("Successfully connected to TAK server: {}", server_id);

                    // ═══════════════════════════════════════════════════════════════
                    // CONNECTION POOL INTEGRATION POINT
                    // ═══════════════════════════════════════════════════════════════

                    // Register this connection with the pool
                    let connection_id = format!("tak-server-{}", server_id);
                    match pool_clone
                        .add_connection(
                            connection_id.clone(),
                            server_id.clone(),
                            address.clone(),
                            5, // Default priority
                        )
                        .await
                    {
                        Ok(_) => {
                            info!("[{}] Registered with connection pool", server_id);

                            // Set filter to broadcast to all connections (default behavior)
                            distributor_clone
                                .add_filter(connection_id.clone(), FilterRule::AlwaysSend);
                        }
                        Err(e) => {
                            error!("[{}] Failed to register with pool: {}", server_id, e);
                            return;
                        }
                    }

                    // Get the connection's rx channel for receiving distributed messages
                    let connection = match pool_clone.get_connection(&connection_id) {
                        Some(conn) => conn,
                        None => {
                            error!("[{}] Failed to get connection from pool", server_id);
                            return;
                        }
                    };

                    // ═══════════════════════════════════════════════════════════════
                    // BIDIRECTIONAL MESSAGE FLOW
                    // ═══════════════════════════════════════════════════════════════

                    // Task 1: Receive messages FROM TAK server → Aggregator
                    let mut rx_from_server = client.receive_cot();
                    let aggregator_sender = aggregator_clone.sender();
                    let metrics_clone = metrics.clone();
                    let server_id_clone = server_id.clone();
                    let connection_id_recv = connection_id.clone();

                    let recv_task = tokio::spawn(async move {
                        while let Some(result) = rx_from_server.next().await {
                            match result {
                                Ok(msg) => {
                                    metrics_clone.record_message(msg.data.len());
                                    debug!(
                                        "[{}] Received {} bytes from TAK server",
                                        server_id_clone,
                                        msg.data.len()
                                    );

                                    // Feed into aggregator for deduplication
                                    // Convert Bytes to Vec<u8>
                                    let inbound_msg = InboundMessage {
                                        data: msg.data.to_vec(),
                                        source: connection_id_recv.clone(),
                                        timestamp: Instant::now(),
                                    };

                                    if let Err(e) = aggregator_sender.send_async(inbound_msg).await
                                    {
                                        error!(
                                            "[{}] Failed to send to aggregator: {}",
                                            server_id_clone, e
                                        );
                                        break;
                                    }
                                }
                                Err(e) => {
                                    metrics_clone.record_error();
                                    warn!("[{}] Error receiving message: {}", server_id_clone, e);
                                    break;
                                }
                            }
                        }
                        warn!("[{}] Receive task ended", server_id_clone);
                    });

                    // Task 2: Receive messages FROM pool → TAK server
                    let rx_from_pool = connection.rx.clone();
                    let server_id_clone = server_id.clone();

                    let send_task = tokio::spawn(async move {
                        while let Ok(pool_msg) = rx_from_pool.recv_async().await {
                            match pool_msg {
                                PoolMessage::Cot(data) => {
                                    debug!(
                                        "[{}] Sending {} bytes to TAK server",
                                        server_id_clone,
                                        data.len()
                                    );

                                    // Send to TAK server - convert Vec<u8> back to CotMessage
                                    let cot_msg = CotMessage {
                                        data: Bytes::from(data),
                                        metadata: None,
                                    };
                                    if let Err(e) = client.send_cot(cot_msg).await {
                                        error!(
                                            "[{}] Failed to send to TAK server: {}",
                                            server_id_clone, e
                                        );
                                        break;
                                    }
                                }
                                PoolMessage::Ping => {
                                    debug!("[{}] Received ping", server_id_clone);
                                    // Health check - no action needed
                                }
                                PoolMessage::Shutdown => {
                                    info!("[{}] Received shutdown signal", server_id_clone);
                                    break;
                                }
                            }
                        }
                        warn!("[{}] Send task ended", server_id_clone);
                    });

                    // Wait for either task to complete
                    tokio::select! {
                        _ = recv_task => {
                            info!("[{}] Connection receive task completed", server_id);
                        }
                        _ = send_task => {
                            info!("[{}] Connection send task completed", server_id);
                        }
                    }

                    // Clean up: remove from pool
                    if let Err(e) = pool_clone.remove_connection(&connection_id).await {
                        warn!("[{}] Failed to remove from pool: {}", server_id, e);
                    }

                    warn!("Connection closed to TAK server: {}", server_id);
                }
            });
        } else if server_def.protocol.to_lowercase() == "tls" {
            if let Some(ref tls_config) = server_def.tls {
                // Create TLS client
                let cert_path = PathBuf::from(&tls_config.cert_path);
                let key_path = PathBuf::from(&tls_config.key_path);
                let ca_path = PathBuf::from(&tls_config.ca_path);

                let mut client_config =
                    TlsClientConfig::new(cert_path, key_path).with_ca_cert(ca_path);
                client_config.base.server_addr = server_def.address.clone();
                client_config.verify_server = tls_config.verify_server;

                // Clone for the async task
                let address = server_def.address.clone();
                let server_id = server_def.id.clone();
                let metrics = global_metrics.clone();
                let pool_clone = Arc::clone(&pool);
                let aggregator_clone = Arc::clone(&aggregator);
                let distributor_clone = Arc::clone(&distributor);

                match TlsClient::new(client_config) {
                    Ok(mut client) => {
                        tokio::spawn(async move {
                            info!("Connecting TLS client to {} ({})", address, server_id);
                            if let Err(e) = client.connect().await {
                                error!("Failed to connect to TAK server {}: {}", server_id, e);
                            } else {
                                info!("Successfully connected to TAK server: {}", server_id);

                                // ═══════════════════════════════════════════════════════════════
                                // CONNECTION POOL INTEGRATION POINT (TLS)
                                // ═══════════════════════════════════════════════════════════════

                                // Register this connection with the pool
                                let connection_id = format!("tak-server-{}", server_id);
                                match pool_clone
                                    .add_connection(
                                        connection_id.clone(),
                                        server_id.clone(),
                                        address.clone(),
                                        5, // Default priority
                                    )
                                    .await
                                {
                                    Ok(_) => {
                                        info!("[{}] Registered with connection pool", server_id);

                                        // Set filter to broadcast to all connections
                                        distributor_clone.add_filter(
                                            connection_id.clone(),
                                            FilterRule::AlwaysSend,
                                        );
                                    }
                                    Err(e) => {
                                        error!(
                                            "[{}] Failed to register with pool: {}",
                                            server_id, e
                                        );
                                        return;
                                    }
                                }

                                // Get the connection's rx channel
                                let connection = match pool_clone.get_connection(&connection_id) {
                                    Some(conn) => conn,
                                    None => {
                                        error!(
                                            "[{}] Failed to get connection from pool",
                                            server_id
                                        );
                                        return;
                                    }
                                };

                                // ═══════════════════════════════════════════════════════════════
                                // BIDIRECTIONAL MESSAGE FLOW (TLS)
                                // ═══════════════════════════════════════════════════════════════

                                // Task 1: Receive messages FROM TAK server → Aggregator
                                let mut rx_from_server = client.receive_cot();
                                let aggregator_sender = aggregator_clone.sender();
                                let metrics_clone = metrics.clone();
                                let server_id_clone = server_id.clone();
                                let connection_id_recv = connection_id.clone();

                                let recv_task = tokio::spawn(async move {
                                    while let Some(result) = rx_from_server.next().await {
                                        match result {
                                            Ok(msg) => {
                                                metrics_clone.record_message(msg.data.len());
                                                debug!(
                                                    "[{}] Received {} bytes from TAK server",
                                                    server_id_clone,
                                                    msg.data.len()
                                                );

                                                // Feed into aggregator for deduplication
                                                // Convert Bytes to Vec<u8>
                                                let inbound_msg = InboundMessage {
                                                    data: msg.data.to_vec(),
                                                    source: connection_id_recv.clone(),
                                                    timestamp: Instant::now(),
                                                };

                                                if let Err(e) =
                                                    aggregator_sender.send_async(inbound_msg).await
                                                {
                                                    error!(
                                                        "[{}] Failed to send to aggregator: {}",
                                                        server_id_clone, e
                                                    );
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                metrics_clone.record_error();
                                                warn!(
                                                    "[{}] Error receiving message: {}",
                                                    server_id_clone, e
                                                );
                                                break;
                                            }
                                        }
                                    }
                                    warn!("[{}] Receive task ended", server_id_clone);
                                });

                                // Task 2: Receive messages FROM pool → TAK server
                                let rx_from_pool = connection.rx.clone();
                                let server_id_clone = server_id.clone();

                                let send_task = tokio::spawn(async move {
                                    while let Ok(pool_msg) = rx_from_pool.recv_async().await {
                                        match pool_msg {
                                            PoolMessage::Cot(data) => {
                                                debug!(
                                                    "[{}] Sending {} bytes to TAK server",
                                                    server_id_clone,
                                                    data.len()
                                                );

                                                // Send to TAK server - convert Vec<u8> back to CotMessage
                                                let cot_msg = CotMessage {
                                                    data: Bytes::from(data),
                                                    metadata: None,
                                                };
                                                if let Err(e) = client.send_cot(cot_msg).await {
                                                    error!(
                                                        "[{}] Failed to send to TAK server: {}",
                                                        server_id_clone, e
                                                    );
                                                    break;
                                                }
                                            }
                                            PoolMessage::Ping => {
                                                debug!("[{}] Received ping", server_id_clone);
                                                // Health check - no action needed
                                            }
                                            PoolMessage::Shutdown => {
                                                info!(
                                                    "[{}] Received shutdown signal",
                                                    server_id_clone
                                                );
                                                break;
                                            }
                                        }
                                    }
                                    warn!("[{}] Send task ended", server_id_clone);
                                });

                                // Wait for either task to complete
                                tokio::select! {
                                    _ = recv_task => {
                                        info!("[{}] Connection receive task completed", server_id);
                                    }
                                    _ = send_task => {
                                        info!("[{}] Connection send task completed", server_id);
                                    }
                                }

                                // Clean up: remove from pool
                                if let Err(e) = pool_clone.remove_connection(&connection_id).await {
                                    warn!("[{}] Failed to remove from pool: {}", server_id, e);
                                }

                                warn!("Connection closed to TAK server: {}", server_id);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to create TLS client for {}: {}", server_id, e);
                    }
                }
            }
        }
    }

    // Build and run API server
    info!("Starting OmniTAK API server");
    info!("Configuration loaded from {:?}", args.config);
    info!("Bind address: {}", bind_addr);
    info!("TLS enabled: {}", server_config.enable_tls);

    let server = ServerBuilder::new(server_config)
        .with_default_user(&args.admin_user, &args.admin_password)
        .build()?;

    // Run server with graceful shutdown
    tokio::select! {
        result = server.run() => {
            if let Err(e) = result {
                error!("Server error: {}", e);

                // Graceful shutdown in proper order:
                // 1. Stop listeners (no new connections)
                // 2. Stop health monitor
                // 3. Stop aggregator
                // 4. Stop distributor
                // 5. Shutdown pool (closes all connections)
                info!("Shutting down infrastructure...");

                // Stop all listeners
                info!("Stopping {} TCP listener(s)...", tcp_listeners.len());
                for listener in &mut tcp_listeners {
                    if let Err(e) = listener.stop().await {
                        error!("Error stopping TCP listener: {}", e);
                    }
                }

                info!("Stopping {} TLS listener(s)...", tls_listeners.len());
                for listener in &mut tls_listeners {
                    if let Err(e) = listener.stop().await {
                        error!("Error stopping TLS listener: {}", e);
                    }
                }

                info!("Shutting down connection pool infrastructure...");
                health_monitor.stop().await;
                aggregator.stop().await;
                distributor.stop().await;
                if let Err(e) = pool.shutdown().await {
                    error!("Error during pool shutdown: {}", e);
                }

                return Err(e);
            }
        }
        _ = signal::ctrl_c() => {
            info!("Received shutdown signal, stopping server...");

            // Graceful shutdown in proper order
            info!("Shutting down infrastructure...");

            // Stop all listeners
            info!("Stopping {} TCP listener(s)...", tcp_listeners.len());
            for listener in &mut tcp_listeners {
                if let Err(e) = listener.stop().await {
                    error!("Error stopping TCP listener: {}", e);
                }
            }

            info!("Stopping {} TLS listener(s)...", tls_listeners.len());
            for listener in &mut tls_listeners {
                if let Err(e) = listener.stop().await {
                    error!("Error stopping TLS listener: {}", e);
                }
            }

            info!("Shutting down connection pool infrastructure...");
            health_monitor.stop().await;
            aggregator.stop().await;
            distributor.stop().await;
            if let Err(e) = pool.shutdown().await {
                error!("Error during pool shutdown: {}", e);
            }
        }
    }

    Ok(())
}
