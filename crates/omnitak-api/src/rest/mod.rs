//! REST API endpoints using Axum

pub mod enrollment;
pub mod plugins;

use crate::auth::{AuthService, AuthUser, RequireAdmin, RequireOperator};
use crate::middleware::AuditLogger;
use crate::types::*;
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use omnitak_client::{
    tcp::{FramingMode, TcpClient, TcpClientConfig},
    tls::{TlsClient, TlsClientConfig},
    BytesMut, ClientConfig, ReconnectConfig,
};
use omnitak_pool::{ConnectionPool, FilterRule as PoolFilterRule, MessageDistributor, PoolMessage};
use quick_xml;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;
use validator::Validate;

// ============================================================================
// Application State
// ============================================================================

#[derive(Clone)]
pub struct ApiState {
    pub auth_service: Arc<AuthService>,
    pub audit_logger: Arc<AuditLogger>,
    pub pool: Arc<ConnectionPool>,
    pub distributor: Arc<MessageDistributor>,
    pub connections: Arc<RwLock<Vec<ConnectionInfo>>>,
    pub start_time: std::time::Instant,
    pub discovery: Option<Arc<omnitak_discovery::DiscoveryService>>,
}

// ============================================================================
// Router Setup
// ============================================================================

pub fn create_rest_router(state: ApiState) -> Router {
    Router::new()
        // System endpoints
        .route("/api/v1/status", get(get_system_status))
        .route("/api/v1/health", get(health_check))
        // Connection management
        .route("/api/v1/connections", get(list_connections))
        .route("/api/v1/connections", post(create_connection))
        .route("/api/v1/connections/{id}", get(get_connection))
        .route("/api/v1/connections/{id}", delete(delete_connection))
        .route(
            "/api/v1/connections/{id}/reconnect",
            post(reconnect_connection),
        )
        .route("/api/v1/test-connection", post(test_connection))
        // Filter management
        .route("/api/v1/filters", get(list_filters))
        .route("/api/v1/filters", post(create_filter))
        .route("/api/v1/filters/{id}", get(get_filter))
        .route("/api/v1/filters/{id}", delete(delete_filter))
        // CoT message injection
        .route("/api/v1/cot/send", post(send_cot_message))
        // Metrics
        .route("/api/v1/metrics", get(get_metrics))
        // Authentication
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/api-keys", post(create_api_key))
        // Audit logs (admin only)
        .route("/api/v1/audit", get(get_audit_logs))
        // ADB integration (requires operator role)
        .route("/api/v1/adb/devices", get(crate::adb::list_devices))
        .route(
            "/api/v1/adb/pull-certs",
            post(crate::adb::pull_certificates),
        )
        // Discovery service routes (if enabled)
        .route(
            "/api/v1/discovery/status",
            get(crate::discovery::get_discovery_status),
        )
        .route(
            "/api/v1/discovery/services",
            get(crate::discovery::list_discovered_services),
        )
        .route(
            "/api/v1/discovery/services/{id}",
            get(crate::discovery::get_discovered_service),
        )
        .route(
            "/api/v1/discovery/refresh",
            post(crate::discovery::refresh_discovery),
        )
        .route(
            "/api/v1/discovery/tak-servers",
            get(crate::discovery::list_tak_servers),
        )
        .route(
            "/api/v1/discovery/atak-devices",
            get(crate::discovery::list_atak_devices),
        )
        .with_state(state)
}

// ============================================================================
// System Status Endpoints
// ============================================================================

/// GET /api/v1/status - Get overall system status
#[utoipa::path(
    get,
    path = "/api/v1/status",
    responses(
        (status = 200, description = "System status retrieved successfully", body = SystemStatus),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_system_status(
    State(state): State<ApiState>,
    _user: AuthUser,
) -> Result<Json<SystemStatus>, ApiError> {
    // Get real metrics from pool
    let pool_stats = state.pool.stats();
    let _pool_metrics = state.pool.metrics();

    // Calculate uptime
    let uptime_seconds = state.start_time.elapsed().as_secs();

    // Throughput is the combined message rate over uptime.
    let total_messages = pool_stats.total_messages_received + pool_stats.total_messages_sent;
    let messages_per_second = if uptime_seconds > 0 {
        total_messages as f64 / uptime_seconds as f64
    } else {
        0.0
    };

    // Get memory usage (current process)
    let memory_usage_bytes = get_memory_usage();

    let status = SystemStatus {
        uptime_seconds,
        active_connections: pool_stats.active_connections,
        messages_processed: pool_stats.total_messages_received,
        messages_sent: pool_stats.total_messages_sent,
        errors: pool_stats.total_errors,
        messages_per_second,
        memory_usage_bytes,
        active_filters: 0, // TODO: Get from filter engine when available
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
    };

    Ok(Json(status))
}

// Helper function to get memory usage
fn get_memory_usage() -> u64 {
    // Read from /proc/self/statm for Linux
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(pages) = statm.split_whitespace().nth(1) {
                if let Ok(rss_pages) = pages.parse::<u64>() {
                    // Convert pages to bytes (page size is typically 4096)
                    return rss_pages * 4096;
                }
            }
        }
    }

    // Fallback for non-Linux or if reading failed
    0
}

/// GET /api/v1/health - Health check endpoint (no auth required)
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": Utc::now(),
    }))
}

// ============================================================================
// Connection Management Endpoints
// ============================================================================

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

/// GET /api/v1/connections - List all connections
#[utoipa::path(
    get,
    path = "/api/v1/connections",
    params(
        ("offset" = Option<usize>, Query, description = "Pagination offset"),
        ("limit" = Option<usize>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Connections list retrieved successfully", body = ConnectionList),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn list_connections(
    State(state): State<ApiState>,
    Query(query): Query<ListQuery>,
    _user: AuthUser,
) -> Result<Json<ConnectionList>, ApiError> {
    // Get connections from state
    let all_connections = state.connections.read().await;
    let total = all_connections.len();

    // Apply pagination, overlaying live message counts from the pool.
    let connections: Vec<ConnectionInfo> = all_connections
        .iter()
        .skip(query.offset)
        .take(query.limit)
        .cloned()
        .map(|mut c| {
            overlay_live_counts(&state.pool, &mut c);
            c
        })
        .collect();

    Ok(Json(ConnectionList { total, connections }))
}

/// GET /api/v1/connections/:id - Get specific connection details
#[utoipa::path(
    get,
    path = "/api/v1/connections/{id}",
    params(
        ("id" = Uuid, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Connection details retrieved successfully", body = ConnectionInfo),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_connection(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    _user: AuthUser,
) -> Result<Json<ConnectionInfo>, ApiError> {
    // Get connection from state by ID
    let connections = state.connections.read().await;
    let connection = connections
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Connection {} not found", id)))?;

    let mut connection = connection;
    overlay_live_counts(&state.pool, &mut connection);

    Ok(Json(connection))
}

/// Overlay live message counters from the pool onto a ConnectionInfo so the UI
/// shows real RX/TX instead of the static zeros captured at creation time.
fn overlay_live_counts(pool: &ConnectionPool, conn: &mut ConnectionInfo) {
    use std::sync::atomic::Ordering;
    if let Some(c) = pool.get_connection(&conn.id.to_string()) {
        conn.messages_received = c.state.messages_received.load(Ordering::Relaxed);
        conn.messages_sent = c.state.messages_sent.load(Ordering::Relaxed);
        conn.bytes_received = c.state.bytes_received.load(Ordering::Relaxed);
        conn.bytes_sent = c.state.bytes_sent.load(Ordering::Relaxed);
    }
}

/// Update the tracked status of a connection in the shared connections list.
/// Called from the client task so the UI reflects the live socket state.
async fn set_connection_status(
    connections: &Arc<RwLock<Vec<ConnectionInfo>>>,
    id: Uuid,
    status: ConnectionStatus,
    error: Option<String>,
) {
    let mut conns = connections.write().await;
    if let Some(conn) = conns.iter_mut().find(|c| c.id == id) {
        conn.status = status;
        match status {
            ConnectionStatus::Connected => {
                conn.connected_at = Some(Utc::now());
                conn.error = None;
            }
            ConnectionStatus::Error => {
                conn.error = error;
            }
            _ => {}
        }
    }
}

/// POST /api/v1/connections - Create new connection
#[utoipa::path(
    post,
    path = "/api/v1/connections",
    request_body = CreateConnectionRequest,
    responses(
        (status = 201, description = "Connection created successfully", body = CreateConnectionResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires operator role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn create_connection(
    State(state): State<ApiState>,
    RequireOperator(user): RequireOperator,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<CreateConnectionResponse>), ApiError> {
    // Validate request
    request.validate()?;

    let connection_id = Uuid::new_v4();
    let id_str = connection_id.to_string();
    let address_with_port = format!("{}:{}", request.address, request.port);

    info!(
        id = %connection_id,
        name = request.name,
        connection_type = ?request.connection_type,
        address = %address_with_port,
        "Creating connection"
    );

    // Add connection to pool
    state
        .pool
        .add_connection(
            id_str.clone(),
            request.name.clone(),
            address_with_port.clone(),
            5, // Default priority
        )
        .await
        .map_err(|e| {
            error!(id = %connection_id, error = %e, "Failed to add connection to pool");
            ApiError::InternalError(format!("Failed to add connection: {}", e))
        })?;

    // Get the connection's channels from pool
    let connection = state.pool.get_connection(&id_str).ok_or_else(|| {
        ApiError::InternalError("Connection not found in pool after creation".to_string())
    })?;

    let pool_tx = connection.tx.clone();
    let pool_rx = connection.rx.clone();

    // Add filter for this connection
    state
        .distributor
        .add_filter(id_str.clone(), PoolFilterRule::AlwaysSend);

    // Spawn client task based on connection type
    let address_clone = address_with_port.clone();
    let id_clone = id_str.clone();
    let auto_reconnect = request.auto_reconnect;

    match request.connection_type {
        ConnectionType::TcpClient => {
            info!(id = %connection_id, "Creating TCP client");

            let config = TcpClientConfig {
                base: ClientConfig {
                    server_addr: address_with_port.clone(),
                    connect_timeout: Duration::from_secs(10),
                    read_timeout: Duration::from_secs(30),
                    write_timeout: Duration::from_secs(10),
                    recv_buffer_size: 65536,
                    reconnect: ReconnectConfig {
                        enabled: auto_reconnect,
                        max_attempts: Some(5),
                        initial_backoff: Duration::from_secs(1),
                        max_backoff: Duration::from_secs(60),
                        backoff_multiplier: 2.0,
                    },
                },
                framing: FramingMode::Xml,
                keepalive: true,
                keepalive_interval: Some(Duration::from_secs(30)),
                nagle: false,
            };

            // Capture handles for status sync + reconnect supervision.
            let status_conns = Arc::clone(&state.connections);
            let status_id = connection_id;
            let task_pool_tx = pool_tx;
            let task_pool_rx = pool_rx;

            // Spawn supervising client task with a reconnect loop, so a dropped
            // connection is re-established when auto_reconnect is enabled.
            tokio::spawn(async move {
                let reconnect_delay = Duration::from_secs(5);
                loop {
                    set_connection_status(
                        &status_conns,
                        status_id,
                        ConnectionStatus::Connecting,
                        None,
                    )
                    .await;

                    let mut client = TcpClient::new(config.clone());
                    info!(id = %id_clone, "Connecting TCP client");

                    if let Err(e) = client.connect_only().await {
                        error!(id = %id_clone, error = %e, "Failed to connect TCP client");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Error,
                            Some(e.to_string()),
                        )
                        .await;
                    } else {
                        info!(id = %id_clone, address = %address_clone, "TCP client connected");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Connected,
                            None,
                        )
                        .await;

                        let client_arc = Arc::new(tokio::sync::Mutex::new(client));
                        let client_read = Arc::clone(&client_arc);
                        let client_write = Arc::clone(&client_arc);

                        let id_read = id_clone.clone();
                        let id_write = id_clone.clone();
                        let pool_tx = task_pool_tx.clone();
                        let pool_rx = task_pool_rx.clone();

                        // Read task (TAK server → Pool)
                        let read_task = tokio::spawn(async move {
                            let mut buffer = BytesMut::with_capacity(8192);

                            loop {
                                let result = {
                                    let mut client = client_read.lock().await;

                                    // Clone immutable data first (ConnectionStatus is cheap to clone)
                                    let status = client.status().clone();
                                    let framing = client.framing();

                                    // Then get mutable reference to stream
                                    let stream = match client.stream_mut() {
                                        Some(s) => s,
                                        None => {
                                            error!(id = %id_read, "Stream not available");
                                            break;
                                        }
                                    };

                                    TcpClient::read_frame_static(
                                        stream,
                                        &mut buffer,
                                        &status,
                                        framing,
                                    )
                                    .await
                                };

                                match result {
                                    Ok(Some(frame)) => {
                                        info!(id = %id_read, bytes = frame.len(), "Received CoT message");
                                        if let Err(e) = pool_tx
                                            .send_async(PoolMessage::Cot(frame.to_vec()))
                                            .await
                                        {
                                            error!(id = %id_read, error = %e, "Failed to send to pool");
                                            break;
                                        }
                                    }
                                    Ok(None) => {
                                        info!(id = %id_read, "Connection closed by remote");
                                        break;
                                    }
                                    Err(e) => {
                                        error!(id = %id_read, error = %e, "Error reading from TAK server");
                                        break;
                                    }
                                }
                            }
                            info!(id = %id_read, "TCP read task terminated");
                        });

                        // Write task (Pool → TAK server)
                        let write_task = tokio::spawn(async move {
                            loop {
                                match pool_rx.recv_async().await {
                                    Ok(PoolMessage::Cot(data)) => {
                                        let mut client = client_write.lock().await;
                                        if let Err(e) = client.write_frame_direct(&data).await {
                                            error!(id = %id_write, error = %e, "Failed to send to TAK server");
                                            break;
                                        }
                                    }
                                    Ok(PoolMessage::Shutdown) => {
                                        info!(id = %id_write, "Shutdown signal received");
                                        break;
                                    }
                                    Ok(PoolMessage::Ping) => continue,
                                    Err(e) => {
                                        error!(id = %id_write, error = %e, "Pool channel error");
                                        break;
                                    }
                                }
                            }
                            info!(id = %id_write, "TCP write task terminated");
                        });

                        tokio::select! {
                            _ = read_task => {}
                            _ = write_task => {}
                        }

                        // The socket closed (remote hang-up, idle timeout, or error).
                        info!(id = %id_clone, "TCP client disconnected");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Disconnected,
                            None,
                        )
                        .await;
                    }

                    if !auto_reconnect {
                        break;
                    }
                    tokio::time::sleep(reconnect_delay).await;
                }
                info!(id = %id_clone, "TCP client supervisor task ended");
            });
        }
        ConnectionType::TlsClient => {
            info!(id = %connection_id, "Creating TLS client");

            // Build the TLS config: prefer inline cert material uploaded from the
            // browser (base64 PEM or PKCS#12), else fall back to server-side paths.
            let mut client_config = if let Some(cert_b64) = request.tls_client_cert_pem_b64.clone()
            {
                let mk = |data: String, name: &str| omnitak_cert::CertificateData {
                    name: name.to_string(),
                    size: data.len() as u64,
                    data,
                };
                let cert_data = mk(cert_b64, "client-cert");
                let key_data = request
                    .tls_client_key_pem_b64
                    .clone()
                    .map(|k| mk(k, "client-key"));
                let ca_data = request
                    .tls_ca_cert_pem_b64
                    .clone()
                    .map(|c| mk(c, "ca-cert"));
                TlsClientConfig::from_memory(
                    cert_data,
                    key_data,
                    ca_data,
                    request.tls_cert_password.clone(),
                )
            } else {
                let cert_path = request.tls_cert_path.clone().ok_or_else(|| {
                    ApiError::BadRequest(
                        "TLS requires an uploaded client certificate or tls_cert_path".to_string(),
                    )
                })?;
                let key_path = request.tls_key_path.clone().ok_or_else(|| {
                    ApiError::BadRequest(
                        "TLS requires an uploaded client key or tls_key_path".to_string(),
                    )
                })?;
                TlsClientConfig::new(
                    std::path::PathBuf::from(cert_path),
                    std::path::PathBuf::from(key_path),
                )
            };
            client_config.base.server_addr = address_with_port.clone();
            client_config.base.connect_timeout = Duration::from_secs(10);
            client_config.base.read_timeout = Duration::from_secs(30);
            client_config.base.write_timeout = Duration::from_secs(10);
            client_config.base.recv_buffer_size = 65536;
            client_config.base.reconnect = ReconnectConfig {
                enabled: auto_reconnect,
                max_attempts: Some(5),
                initial_backoff: Duration::from_secs(1),
                max_backoff: Duration::from_secs(60),
                backoff_multiplier: 2.0,
            };
            client_config.verify_server = request.validate_certs;

            // Validate cert material up front so a bad upload returns an error now.
            TlsClient::new(client_config.clone()).map_err(|e| {
                ApiError::BadRequest(format!("Invalid TLS certificate material: {}", e))
            })?;

            // Capture handles for status sync + reconnect supervision.
            let status_conns = Arc::clone(&state.connections);
            let status_id = connection_id;
            let task_pool_tx = pool_tx;
            let task_pool_rx = pool_rx;

            tokio::spawn(async move {
                let reconnect_delay = Duration::from_secs(5);
                loop {
                    set_connection_status(
                        &status_conns,
                        status_id,
                        ConnectionStatus::Connecting,
                        None,
                    )
                    .await;

                    let mut client = match TlsClient::new(client_config.clone()) {
                        Ok(c) => c,
                        Err(e) => {
                            error!(id = %id_clone, error = %e, "Failed to create TLS client");
                            set_connection_status(
                                &status_conns,
                                status_id,
                                ConnectionStatus::Error,
                                Some(e.to_string()),
                            )
                            .await;
                            break;
                        }
                    };
                    info!(id = %id_clone, "Connecting TLS client");

                    if let Err(e) = client.connect_only().await {
                        error!(id = %id_clone, error = %e, "Failed to connect TLS client");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Error,
                            Some(e.to_string()),
                        )
                        .await;
                    } else {
                        info!(id = %id_clone, address = %address_clone, "TLS client connected");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Connected,
                            None,
                        )
                        .await;

                        let client_arc = Arc::new(tokio::sync::Mutex::new(client));
                        let client_read = Arc::clone(&client_arc);
                        let client_write = Arc::clone(&client_arc);

                        let id_read = id_clone.clone();
                        let id_write = id_clone.clone();
                        let pool_tx = task_pool_tx.clone();
                        let pool_rx = task_pool_rx.clone();

                        // Read task (TAK server → Pool)
                        let read_task = tokio::spawn(async move {
                            use omnitak_client::tls::TlsClient;

                            let mut buffer = BytesMut::with_capacity(8192);

                            loop {
                                let result = {
                                    let mut client = client_read.lock().await;

                                    // Clone immutable data first (ConnectionStatus is cheap to clone)
                                    let status = client.status().clone();
                                    let framing = client.framing();

                                    // Then get mutable reference to stream
                                    let stream = match client.stream_mut() {
                                        Some(s) => s,
                                        None => {
                                            error!(id = %id_read, "Stream not available");
                                            break;
                                        }
                                    };

                                    TlsClient::read_frame_static(
                                        stream,
                                        &mut buffer,
                                        &status,
                                        framing,
                                    )
                                    .await
                                };

                                match result {
                                    Ok(Some(frame)) => {
                                        info!(id = %id_read, bytes = frame.len(), "Received CoT message (TLS)");
                                        if let Err(e) = pool_tx
                                            .send_async(PoolMessage::Cot(frame.to_vec()))
                                            .await
                                        {
                                            error!(id = %id_read, error = %e, "Failed to send to pool");
                                            break;
                                        }
                                    }
                                    Ok(None) => {
                                        info!(id = %id_read, "Connection closed by remote");
                                        break;
                                    }
                                    Err(e) => {
                                        error!(id = %id_read, error = %e, "Error reading from TLS TAK server");
                                        break;
                                    }
                                }
                            }
                            info!(id = %id_read, "TLS read task terminated");
                        });

                        // Write task (Pool → TAK server)
                        let write_task = tokio::spawn(async move {
                            loop {
                                match pool_rx.recv_async().await {
                                    Ok(PoolMessage::Cot(data)) => {
                                        let mut client = client_write.lock().await;
                                        if let Err(e) = client.write_frame_direct(&data).await {
                                            error!(id = %id_write, error = %e, "Failed to send to TLS TAK server");
                                            break;
                                        }
                                    }
                                    Ok(PoolMessage::Shutdown) => {
                                        info!(id = %id_write, "Shutdown signal received");
                                        break;
                                    }
                                    Ok(PoolMessage::Ping) => continue,
                                    Err(e) => {
                                        error!(id = %id_write, error = %e, "Pool channel error");
                                        break;
                                    }
                                }
                            }
                            info!(id = %id_write, "TLS write task terminated");
                        });

                        tokio::select! {
                            _ = read_task => {}
                            _ = write_task => {}
                        }

                        info!(id = %id_clone, "TLS client disconnected");
                        set_connection_status(
                            &status_conns,
                            status_id,
                            ConnectionStatus::Disconnected,
                            None,
                        )
                        .await;
                    }

                    if !auto_reconnect {
                        break;
                    }
                    tokio::time::sleep(reconnect_delay).await;
                }
                info!(id = %id_clone, "TLS client supervisor task ended");
            });
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Connection type {:?} not yet supported",
                request.connection_type
            )));
        }
    }

    // Store connection info
    let conn_info = ConnectionInfo {
        id: connection_id,
        name: request.name.clone(),
        connection_type: request.connection_type,
        status: ConnectionStatus::Connecting,
        address: request.address.clone(),
        port: request.port,
        messages_received: 0,
        messages_sent: 0,
        bytes_received: 0,
        bytes_sent: 0,
        connected_at: None,
        last_activity: None,
        error: None,
    };

    let mut connections = state.connections.write().await;
    connections.push(conn_info);
    drop(connections);

    // Audit log with actual client IP
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "create_connection".to_string(),
        format!("/api/v1/connections/{}", connection_id),
        serde_json::to_value(&request).unwrap(),
        client_addr.ip().to_string(),
        true,
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateConnectionResponse {
            id: connection_id,
            message: "Connection created successfully".to_string(),
        }),
    ))
}

/// DELETE /api/v1/connections/:id - Remove connection
#[utoipa::path(
    delete,
    path = "/api/v1/connections/{id}",
    params(
        ("id" = Uuid, Path, description = "Connection ID")
    ),
    responses(
        (status = 200, description = "Connection deleted successfully", body = DeleteConnectionResponse),
        (status = 404, description = "Connection not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires operator role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn delete_connection(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    RequireOperator(user): RequireOperator,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
) -> Result<Json<DeleteConnectionResponse>, ApiError> {
    let id_str = id.to_string();
    info!(connection_id = %id, "Deleting connection");

    // Remove from connection pool
    state.pool.remove_connection(&id_str).await.map_err(|e| {
        error!(connection_id = %id, error = %e, "Failed to remove connection from pool");
        ApiError::InternalError(format!("Failed to remove connection: {}", e))
    })?;

    // Remove from state tracking
    let mut connections = state.connections.write().await;
    let initial_len = connections.len();
    connections.retain(|c| c.id != id);

    if connections.len() == initial_len {
        return Err(ApiError::NotFound(format!("Connection {} not found", id)));
    }
    drop(connections);

    // Remove filters
    state.distributor.remove_filters(&id_str);

    info!(connection_id = %id, "Connection deleted successfully");

    // Audit log with actual client IP
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "delete_connection".to_string(),
        format!("/api/v1/connections/{}", id),
        serde_json::json!({"connection_id": id}),
        client_addr.ip().to_string(),
        true,
    );

    Ok(Json(DeleteConnectionResponse {
        message: "Connection deleted successfully".to_string(),
    }))
}

/// POST /api/v1/connections/:id/reconnect - Request a reconnect for a connection
async fn reconnect_connection(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    RequireOperator(user): RequireOperator,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
) -> Result<Json<ReconnectResponse>, ApiError> {
    info!(connection_id = %id, "Reconnect requested");

    // Flip the tracked status to Connecting; the pool's health monitor drives the
    // actual reconnect on its next cycle for connections with auto-reconnect enabled.
    let mut connections = state.connections.write().await;
    let conn = connections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("Connection {} not found", id)))?;
    conn.status = ConnectionStatus::Connecting;
    conn.error = None;
    drop(connections);

    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "reconnect_connection".to_string(),
        format!("/api/v1/connections/{}/reconnect", id),
        serde_json::json!({ "connection_id": id }),
        client_addr.ip().to_string(),
        true,
    );

    Ok(Json(ReconnectResponse {
        id,
        status: ConnectionStatus::Connecting,
        message: "Reconnect requested".to_string(),
    }))
}

/// POST /api/v1/test-connection - Probe TCP reachability of a host:port
async fn test_connection(
    _user: AuthUser,
    Json(request): Json<TestConnectionRequest>,
) -> Result<Json<TestConnectionResponse>, ApiError> {
    let address = request.address.trim().to_string();
    if address.is_empty() {
        return Err(ApiError::BadRequest("address is required".to_string()));
    }

    info!(address = %address, "Testing connection reachability");

    let start = std::time::Instant::now();
    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(&address),
    )
    .await;

    match probe {
        Ok(Ok(_stream)) => {
            let latency = start.elapsed().as_millis() as u64;
            Ok(Json(TestConnectionResponse {
                success: true,
                latency,
                message: format!("Reachable at {}", address),
            }))
        }
        Ok(Err(e)) => Err(ApiError::BadRequest(format!(
            "Could not connect to {}: {}",
            address, e
        ))),
        Err(_) => Err(ApiError::BadRequest(format!(
            "Connection to {} timed out after 5s",
            address
        ))),
    }
}

// ============================================================================
// CoT Message Injection Endpoints
// ============================================================================

/// POST /api/v1/cot/send - Inject a CoT message into the system
#[utoipa::path(
    post,
    path = "/api/v1/cot/send",
    request_body = SendCotRequest,
    responses(
        (status = 200, description = "Message sent successfully", body = SendCotResponse),
        (status = 400, description = "Invalid message format", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 500, description = "Internal error", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn send_cot_message(
    State(state): State<ApiState>,
    RequireOperator(user): RequireOperator,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    Json(request): Json<SendCotRequest>,
) -> Result<Json<SendCotResponse>, ApiError> {
    use omnitak_pool::DistributionMessage;

    // Validate the request
    request
        .validate()
        .map_err(|e| ApiError::BadRequest(format!("Validation failed: {}", e)))?;

    let message_id = Uuid::new_v4();
    let mut warnings = Vec::new();

    info!(
        message_id = %message_id,
        user = %user.user_id.as_ref().unwrap_or(&"api_key".to_string()),
        target_count = request.target_connections.as_ref().map(|t| t.len()).unwrap_or(0),
        "Injecting CoT message"
    );

    // Basic validation: check if it looks like XML
    let message_str = request.message.trim();
    if !message_str.starts_with('<') || !message_str.ends_with('>') {
        return Err(ApiError::BadRequest(
            "Message must be valid XML starting with '<' and ending with '>'".to_string(),
        ));
    }

    // Try to parse as XML to validate structure (forgiving parse)
    // Note: We accept the message even if parsing isn't perfect - prioritize showing data
    match quick_xml::Reader::from_str(message_str).read_event() {
        Ok(_) => {
            // XML structure looks valid
        }
        Err(e) => {
            // Log warning but accept anyway
            let warning = format!("XML parse warning: {}. Message will be sent as-is.", e);
            warn!(message_id = %message_id, warning = %warning);
            warnings.push(warning);
        }
    }

    // Create distribution message
    let dist_message = DistributionMessage {
        data: message_str.as_bytes().to_vec(),
        source: None, // Injected messages have no source connection
        timestamp: std::time::Instant::now(),
    };

    // Send to distributor
    let sender = state.distributor.sender();
    sender.send_async(dist_message).await.map_err(|e| {
        error!(message_id = %message_id, error = %e, "Failed to send message to distributor");
        ApiError::InternalError(format!("Failed to queue message: {}", e))
    })?;

    // Determine which connections received it
    // For now, we report based on request (actual delivery is async)
    let (sent_to_connections, sent_to_count) = if let Some(targets) = &request.target_connections {
        (targets.clone(), targets.len())
    } else {
        // Broadcasting to all connections
        let connections = state.connections.read().await;
        let all_ids: Vec<Uuid> = connections
            .iter()
            .filter(|c| c.status == ConnectionStatus::Connected)
            .map(|c| c.id)
            .collect();
        let count = all_ids.len();
        (all_ids, count)
    };

    info!(
        message_id = %message_id,
        sent_to_count = sent_to_count,
        "CoT message queued for distribution"
    );

    // Audit log
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "send_cot_message".to_string(),
        "/api/v1/cot/send".to_string(),
        serde_json::json!({
            "message_id": message_id,
            "sent_to_count": sent_to_count,
            "message_length": message_str.len(),
        }),
        client_addr.ip().to_string(),
        true,
    );

    Ok(Json(SendCotResponse {
        message_id,
        sent_to_count,
        sent_to_connections,
        warnings,
        timestamp: Utc::now(),
    }))
}

// ============================================================================
// Filter Management Endpoints
// ============================================================================

/// GET /api/v1/filters - List all filters
#[utoipa::path(
    get,
    path = "/api/v1/filters",
    responses(
        (status = 200, description = "Filters list retrieved successfully", body = FilterList),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn list_filters(
    State(_state): State<ApiState>,
    _user: AuthUser,
) -> Result<Json<FilterList>, ApiError> {
    // TODO: Get actual filters from filter engine
    let filters = vec![];

    Ok(Json(FilterList {
        total: filters.len(),
        filters,
    }))
}

/// GET /api/v1/filters/:id - Get specific filter
async fn get_filter(
    State(_state): State<ApiState>,
    Path(id): Path<Uuid>,
    _user: AuthUser,
) -> Result<Json<crate::types::FilterRule>, ApiError> {
    // TODO: Get actual filter from filter engine
    Err(ApiError::NotFound(format!("Filter {} not found", id)))
}

/// POST /api/v1/filters - Create new filter
#[utoipa::path(
    post,
    path = "/api/v1/filters",
    request_body = CreateFilterRequest,
    responses(
        (status = 201, description = "Filter created successfully", body = CreateFilterResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires operator role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn create_filter(
    State(state): State<ApiState>,
    RequireOperator(user): RequireOperator,
    Json(request): Json<CreateFilterRequest>,
) -> Result<(StatusCode, Json<CreateFilterResponse>), ApiError> {
    // Validate request
    request.validate()?;

    info!(
        name = request.name,
        action = ?request.action,
        priority = request.priority,
        "Creating filter"
    );

    // TODO: Actually create filter in filter engine
    let filter_id = Uuid::new_v4();

    // Audit log
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "create_filter".to_string(),
        format!("/api/v1/filters/{}", filter_id),
        serde_json::to_value(&request).unwrap(),
        "0.0.0.0".to_string(), // TODO: Get actual IP
        true,
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateFilterResponse {
            id: filter_id,
            message: "Filter created successfully".to_string(),
        }),
    ))
}

/// DELETE /api/v1/filters/:id - Remove filter
async fn delete_filter(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    RequireOperator(user): RequireOperator,
) -> Result<Json<DeleteConnectionResponse>, ApiError> {
    info!(filter_id = %id, "Deleting filter");

    // TODO: Actually delete filter from filter engine

    // Audit log
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "delete_filter".to_string(),
        format!("/api/v1/filters/{}", id),
        serde_json::json!({"filter_id": id}),
        "0.0.0.0".to_string(), // TODO: Get actual IP
        true,
    );

    Ok(Json(DeleteConnectionResponse {
        message: "Filter deleted successfully".to_string(),
    }))
}

// ============================================================================
// Metrics Endpoint
// ============================================================================

/// GET /api/v1/metrics - Prometheus metrics
#[utoipa::path(
    get,
    path = "/api/v1/metrics",
    responses(
        (status = 200, description = "Metrics retrieved successfully", content_type = "text/plain"),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_metrics(State(state): State<ApiState>, _user: AuthUser) -> Result<String, ApiError> {
    // Get real metrics from pool
    let pool_stats = state.pool.stats();
    let _pool_metrics = state.pool.metrics();

    // Format as Prometheus metrics
    Ok(format!(
        "# HELP omnitak_connections_total Total number of connections\n\
         # TYPE omnitak_connections_total gauge\n\
         omnitak_connections_total {}\n\
         \n\
         # HELP omnitak_connections_active Active connections\n\
         # TYPE omnitak_connections_active gauge\n\
         omnitak_connections_active {}\n\
         \n\
         # HELP omnitak_messages_received_total Total messages received\n\
         # TYPE omnitak_messages_received_total counter\n\
         omnitak_messages_received_total {}\n\
         \n\
         # HELP omnitak_messages_sent_total Total messages sent\n\
         # TYPE omnitak_messages_sent_total counter\n\
         omnitak_messages_sent_total {}\n\
         \n\
         # HELP omnitak_messages_processed_total Total messages processed\n\
         # TYPE omnitak_messages_processed_total counter\n\
         omnitak_messages_processed_total {}\n\
         \n\
         # HELP omnitak_uptime_seconds System uptime in seconds\n\
         # TYPE omnitak_uptime_seconds gauge\n\
         omnitak_uptime_seconds {}\n",
        pool_stats.total_connections,
        pool_stats.active_connections,
        pool_stats.total_messages_received,
        pool_stats.total_messages_sent,
        pool_stats.total_messages_received + pool_stats.total_messages_sent,
        state.start_time.elapsed().as_secs(),
    ))
}

// ============================================================================
// Authentication Endpoints
// ============================================================================

/// POST /api/v1/auth/login - User login
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ErrorResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse)
    )
)]
async fn login(
    State(state): State<ApiState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Validate request
    request.validate()?;

    // Attempt login
    let (access_token, expires_at) = state
        .auth_service
        .login(&request.username, &request.password)
        .map_err(|_| ApiError::Unauthorized("Invalid credentials".to_string()))?;

    // Get user role
    let user = state
        .auth_service
        .users
        .get(&request.username)
        .ok_or_else(|| ApiError::Unauthorized("Invalid credentials".to_string()))?;

    let role = user.role;

    info!(username = request.username, "User logged in successfully");

    Ok(Json(LoginResponse {
        access_token,
        expires_at,
        role,
    }))
}

/// POST /api/v1/auth/api-keys - Create API key (admin only)
#[utoipa::path(
    post,
    path = "/api/v1/auth/api-keys",
    request_body = ApiKeyRequest,
    responses(
        (status = 201, description = "API key created successfully", body = ApiKeyResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires admin role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn create_api_key(
    State(state): State<ApiState>,
    RequireAdmin(user): RequireAdmin,
    Json(request): Json<ApiKeyRequest>,
) -> Result<(StatusCode, Json<ApiKeyResponse>), ApiError> {
    // Validate request
    request.validate()?;

    // Create API key
    let (api_key, key_id) = state
        .auth_service
        .create_api_key(request.name.clone(), request.role, request.expires_at)
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    info!(
        key_name = request.name,
        key_role = ?request.role,
        "API key created"
    );

    // Audit log
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "api_key".to_string()),
        user.role,
        "create_api_key".to_string(),
        format!("/api/v1/auth/api-keys/{}", key_id),
        serde_json::to_value(&request).unwrap(),
        "0.0.0.0".to_string(), // TODO: Get actual IP
        true,
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiKeyResponse {
            api_key,
            id: key_id,
            name: request.name,
            created_at: Utc::now(),
        }),
    ))
}

// ============================================================================
// Audit Log Endpoints
// ============================================================================

/// GET /api/v1/audit - Get audit logs (admin only)
#[utoipa::path(
    get,
    path = "/api/v1/audit",
    responses(
        (status = 200, description = "Audit logs retrieved successfully"),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires admin role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_audit_logs(
    State(state): State<ApiState>,
    RequireAdmin(_user): RequireAdmin,
) -> Result<Json<Vec<AuditLogEntry>>, ApiError> {
    let logs = state.audit_logger.get_logs();
    Ok(Json(logs))
}

// ============================================================================
// Error Handling
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Validation error: {0}")]
    ValidationError(#[from] validator::ValidationErrors),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_code, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "unauthorized", msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            ApiError::InternalError(msg) => {
                error!(error = %msg, "Internal API error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "An internal error occurred".to_string(),
                )
            }
            ApiError::ValidationError(errors) => {
                let message = format!("Validation failed: {}", errors);
                (StatusCode::BAD_REQUEST, "validation_error", message)
            }
        };

        let body = Json(ErrorResponse::new(error_code, message));
        (status, body).into_response()
    }
}
