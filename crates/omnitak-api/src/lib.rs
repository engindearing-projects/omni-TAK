//! OmniTAK API - REST API and WebSocket interface for TAK aggregator
//!
//! This crate provides a comprehensive web API for managing and monitoring
//! the TAK (Team Awareness Kit) aggregator with military-grade security features.
//!
//! # Features
//!
//! - REST API for system management
//! - WebSocket streaming for real-time CoT messages
//! - JWT and API key authentication
//! - Role-based access control (RBAC)
//! - Rate limiting and DoS protection
//! - Comprehensive audit logging
//! - Prometheus metrics
//! - TLS-only communication
//! - OpenAPI/Swagger documentation
//!
//! # Example
//!
//! ```no_run
//! use omnitak_api::{ServerBuilder, ServerConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = ServerConfig::default();
//!     let server = ServerBuilder::new(config)
//!         .with_default_user("admin", "secure_password")
//!         .build()?;
//!
//!     server.run().await?;
//!     Ok(())
//! }
//! ```

pub mod adb;
pub mod auth;
pub mod discovery;
pub mod middleware;
pub mod rest;
pub mod static_files;
pub mod types;
pub mod websocket;

use auth::{AuthConfig, AuthService};
use middleware::{
    cors_layer, logging_middleware, rate_limit_middleware, request_id_middleware,
    security_headers_middleware, timeout_middleware, RateLimitState, ReadinessState,
};
use omnitak_cert::generator::CaConfig;
use omnitak_plugin_api::PluginManager;
use omnitak_pool::{
    AggregatorConfig, ConnectionPool, DistributionStrategy, DistributorConfig, MessageAggregator,
    MessageDistributor, PoolConfig,
};
use rest::enrollment::EnrollmentState;
use rest::ApiState;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::RwLock;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};
use tracing::{error, info, warn};
use types::{ServerConnectionConfig, UserRole};
use utoipa::OpenApi;

// ============================================================================
// Server Configuration
// ============================================================================

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Server bind address
    pub bind_addr: SocketAddr,

    /// Enable TLS
    pub enable_tls: bool,

    /// TLS certificate path
    pub tls_cert_path: Option<String>,

    /// TLS key path
    pub tls_key_path: Option<String>,

    /// Authentication configuration
    pub auth_config: AuthConfig,

    /// Rate limit (requests per second)
    pub rate_limit_rps: u32,

    /// Enable Swagger UI
    pub enable_swagger: bool,

    /// Enable static file serving
    pub enable_static_files: bool,

    /// Enable enrollment service
    pub enable_enrollment: bool,

    /// Enrollment CA certificate path (for persistence)
    pub enrollment_ca_cert_path: Option<PathBuf>,

    /// Enrollment CA key path (for persistence)
    pub enrollment_ca_key_path: Option<PathBuf>,

    /// Server connection config for enrollment data packages
    pub enrollment_server_config: Option<ServerConnectionConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8443".parse().unwrap(),
            enable_tls: true,
            tls_cert_path: None,
            tls_key_path: None,
            auth_config: AuthConfig::default(),
            rate_limit_rps: 100,
            enable_swagger: true,
            enable_static_files: true,
            enable_enrollment: true,
            enrollment_ca_cert_path: None,
            enrollment_ca_key_path: None,
            enrollment_server_config: None,
        }
    }
}

// ============================================================================
// OpenAPI Documentation
// ============================================================================

#[derive(OpenApi)]
#[openapi(
    info(
        title = "OmniTAK API",
        version = "0.1.0",
        description = "REST API and WebSocket interface for TAK aggregator",
        contact(
            name = "OmniTAK Team",
            email = "support@omnitak.dev"
        ),
        license(
            name = "MIT OR Apache-2.0"
        )
    ),
    paths(
        rest::get_system_status,
        rest::list_connections,
        rest::get_connection,
        rest::create_connection,
        rest::delete_connection,
        rest::list_filters,
        rest::create_filter,
        rest::get_metrics,
        rest::login,
        rest::create_api_key,
        rest::get_audit_logs,
        rest::plugins::list_plugins,
        rest::plugins::load_plugin,
        rest::plugins::get_plugin_details,
        rest::plugins::unload_plugin,
        rest::plugins::update_plugin_config,
        rest::plugins::toggle_plugin,
        rest::plugins::get_plugin_metrics,
        rest::plugins::get_plugin_health,
        rest::plugins::reload_plugin,
        rest::plugins::reload_all_plugins,
    ),
    components(
        schemas(
            types::SystemStatus,
            types::ConnectionInfo,
            types::ConnectionList,
            types::ConnectionStatus,
            types::ConnectionType,
            types::CreateConnectionRequest,
            types::CreateConnectionResponse,
            types::DeleteConnectionResponse,
            types::FilterRule,
            types::FilterList,
            types::FilterAction,
            types::CreateFilterRequest,
            types::CreateFilterResponse,
            types::GeoBounds,
            types::MetricsSnapshot,
            types::LoginRequest,
            types::LoginResponse,
            types::ApiKeyRequest,
            types::ApiKeyResponse,
            types::UserRole,
            types::ErrorResponse,
            types::AuditLogEntry,
            types::WsClientMessage,
            types::WsServerMessage,
        )
    ),
    tags(
        (name = "system", description = "System status and health"),
        (name = "connections", description = "Connection management"),
        (name = "filters", description = "Filter management"),
        (name = "metrics", description = "Prometheus metrics"),
        (name = "auth", description = "Authentication"),
        (name = "audit", description = "Audit logs"),
        (name = "plugins", description = "Plugin management"),
    ),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::Http::new(
                        utoipa::openapi::security::HttpAuthScheme::Bearer,
                    ),
                ),
            );
            components.add_security_scheme(
                "api_key",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Header(
                        utoipa::openapi::security::ApiKeyValue::new("X-API-Key"),
                    ),
                ),
            );
        }
    }
}

// ============================================================================
// Server Builder
// ============================================================================

pub struct ServerBuilder {
    config: ServerConfig,
    auth_service: Option<Arc<AuthService>>,
}

impl ServerBuilder {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            auth_service: None,
        }
    }

    /// Add a default admin user
    pub fn with_default_user(mut self, username: &str, password: &str) -> Self {
        let auth_service = Arc::new(AuthService::new(self.config.auth_config.clone()));

        if let Err(e) = auth_service.create_user(username.to_string(), password, UserRole::Admin) {
            error!(error = %e, "Failed to create default user");
        } else {
            info!(username = username, "Created default admin user");
        }

        self.auth_service = Some(auth_service);
        self
    }

    /// Build the server
    pub fn build(self) -> anyhow::Result<Server> {
        let auth_service = self
            .auth_service
            .unwrap_or_else(|| Arc::new(AuthService::new(self.config.auth_config.clone())));

        Ok(Server {
            config: self.config,
            auth_service,
        })
    }
}

// ============================================================================
// Server
// ============================================================================

pub struct Server {
    config: ServerConfig,
    auth_service: Arc<AuthService>,
}

impl Server {
    /// Run the server
    pub async fn run(self) -> anyhow::Result<()> {
        // Initialize tracing (ignore if already initialized)
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "omnitak_api=debug,tower_http=debug".into()),
            )
            .try_init();

        info!("Starting OmniTAK API server");

        // Print embedded files in debug mode
        static_files::print_embedded_files();

        // Initialize connection pool
        info!("Initializing connection pool");
        let pool_config = PoolConfig {
            max_connections: 1000,
            channel_capacity: 1024,
            health_check_interval: Duration::from_secs(30),
            inactive_timeout: Duration::from_secs(300),
            auto_reconnect: true,
        };
        let pool = Arc::new(ConnectionPool::new(pool_config));

        // Initialize message distributor
        info!("Initializing message distributor");
        let distributor_config = DistributorConfig {
            strategy: DistributionStrategy::DropOnFull,
            channel_capacity: 1024,
            max_workers: 4,
            batch_size: 10,
            flush_interval: Duration::from_millis(100),
        };
        let distributor = Arc::new(MessageDistributor::new(pool.clone(), distributor_config));

        // Initialize message aggregator (for future use)
        let aggregator_config = AggregatorConfig {
            dedup_window: Duration::from_secs(60),
            max_cache_entries: 10000,
            cleanup_interval: Duration::from_secs(30),
            channel_capacity: 1024,
            worker_count: 4,
        };
        let _aggregator = Arc::new(MessageAggregator::new(
            distributor.clone(),
            aggregator_config,
        ));

        // Create application state
        let audit_logger = Arc::new(middleware::AuditLogger::new());
        let api_state = ApiState {
            auth_service: self.auth_service.clone(),
            audit_logger: audit_logger.clone(),
            pool: pool.clone(),
            distributor: distributor.clone(),
            connections: Arc::new(RwLock::new(Vec::new())),
            start_time: std::time::Instant::now(),
            discovery: None, // TODO: Initialize discovery service if enabled in config
        };

        // Initialize plugin manager in a blocking task to avoid blocking the async runtime
        // (wasmtime Engine creation with Cranelift JIT compilation is CPU-intensive)
        info!("Initializing plugin manager");
        let plugin_manager = tokio::task::spawn_blocking(|| {
            match PluginManager::new(omnitak_plugin_api::PluginManagerConfig::default()) {
                Ok(manager) => Arc::new(RwLock::new(manager)),
                Err(e) => {
                    warn!(
                        "Failed to initialize plugin manager: {}, plugins will be disabled",
                        e
                    );
                    // Create a minimal manager as a fallback
                    Arc::new(RwLock::new(
                        PluginManager::new(omnitak_plugin_api::PluginManagerConfig {
                            plugin_dir: "./plugins".to_string(),
                            hot_reload: false,
                            ..Default::default()
                        })
                        .unwrap_or_else(|_| {
                            panic!("Critical: Could not initialize plugin manager")
                        }),
                    ))
                }
            }
        })
        .await
        .expect("Plugin manager spawn_blocking failed");
        let plugin_state = rest::plugins::PluginApiState {
            plugin_manager,
            audit_logger: audit_logger.clone(),
        };

        let ws_state = websocket::WsState::new(self.auth_service.clone());
        let rate_limit_state = Arc::new(RateLimitState::new(self.config.rate_limit_rps));
        let readiness_state = Arc::new(ReadinessState::new());

        // Build the router
        let mut app = axum::Router::new();

        // Add REST API routes
        app = app.merge(rest::create_rest_router(api_state.clone()));

        // Add plugin management routes
        app = app.merge(rest::plugins::create_plugin_router(plugin_state));

        // Add WebSocket routes
        app = app.merge(websocket::create_ws_router(ws_state.clone()));

        // Add enrollment routes if enabled
        if self.config.enable_enrollment {
            info!("Initializing enrollment service");
            let mut enrollment_state = EnrollmentState::new(audit_logger.clone());

            // Set CA paths if configured
            enrollment_state.ca_cert_path = self.config.enrollment_ca_cert_path.clone();
            enrollment_state.ca_key_path = self.config.enrollment_ca_key_path.clone();

            // Initialize CA
            if let Err(e) = enrollment_state.initialize_ca(None).await {
                error!("Failed to initialize enrollment CA: {}", e);
            } else {
                // Set server config if provided
                if let Some(server_config) = &self.config.enrollment_server_config {
                    enrollment_state
                        .set_server_config(server_config.clone())
                        .await;
                } else {
                    // Auto-detect from bind address
                    let host = if self.config.bind_addr.ip().is_unspecified() {
                        "localhost".to_string()
                    } else {
                        self.config.bind_addr.ip().to_string()
                    };
                    let server_config = ServerConnectionConfig {
                        host,
                        streaming_port: 8089,
                        api_port: self.config.bind_addr.port(),
                        description: Some("OmniTAK Server".to_string()),
                        use_tls: self.config.enable_tls,
                    };
                    enrollment_state.set_server_config(server_config).await;
                }

                app = app.merge(rest::enrollment::create_enrollment_router(enrollment_state));
                info!("Enrollment service enabled at /api/v1/enrollment/*");
            }
        }

        // Add OpenAPI JSON endpoint if enabled
        if self.config.enable_swagger {
            let openapi = ApiDoc::openapi();
            app = app.route(
                "/api-docs/openapi.json",
                axum::routing::get(|| async move { axum::Json(openapi) }),
            );
            info!("OpenAPI spec available at /api-docs/openapi.json");
            info!("Custom API docs available at /api-docs.html, /rapidoc.html, /redoc.html");
        }

        // Add static file serving if enabled
        if self.config.enable_static_files {
            app = app.merge(static_files::create_static_router());
            info!("Static file serving enabled");
        }

        // Add health check endpoints
        app = app
            .route("/health", axum::routing::get(middleware::health_check))
            .route(
                "/ready",
                axum::routing::get(middleware::readiness_check).with_state(readiness_state.clone()),
            );

        // Add middleware layers
        app = app.layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(axum::middleware::from_fn(request_id_middleware))
                .layer(axum::middleware::from_fn(security_headers_middleware))
                .layer(axum::middleware::from_fn(timeout_middleware))
                .layer(axum::middleware::from_fn_with_state(
                    rate_limit_state.clone(),
                    rate_limit_middleware,
                ))
                .layer(axum::middleware::from_fn(logging_middleware))
                .layer(CompressionLayer::new())
                .layer(cors_layer()),
        );

        // Add auth service to extensions
        app = app.layer(axum::Extension(self.auth_service.clone()));

        // Mark server as ready
        readiness_state.set_ready(true);

        // Start server
        let listener = tokio::net::TcpListener::bind(self.config.bind_addr).await?;
        let local_addr = listener.local_addr()?;

        info!(
            address = %local_addr,
            tls = self.config.enable_tls,
            "Server listening"
        );

        if self.config.enable_tls {
            info!("TLS enabled - ensure valid certificates are configured");
            // TODO: Implement TLS with rustls
        } else {
            info!("WARNING: TLS disabled - not suitable for production use!");
        }

        // Run server (graceful shutdown handled by parent caller via tokio::select!)
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;

        info!("Server shutdown complete");
        Ok(())
    }
}

// ============================================================================
// Graceful Shutdown
// ============================================================================

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl-C signal");
        },
        _ = terminate => {
            info!("Received terminate signal");
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr.port(), 8443);
        assert!(config.enable_tls);
        assert!(config.enable_swagger);
    }

    #[test]
    fn test_server_builder() {
        let config = ServerConfig::default();
        let builder = ServerBuilder::new(config);
        let server = builder
            .with_default_user("test", "test_password_123")
            .build();
        assert!(server.is_ok());
    }
}
