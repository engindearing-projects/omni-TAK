//! API request and response types with OpenAPI schema generation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

// ============================================================================
// Common Types
// ============================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionType {
    TcpClient,
    TcpServer,
    TlsClient,
    TlsServer,
    Multicast,
    Udp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    Operator,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    Allow,
    Deny,
    Modify,
}

// ============================================================================
// System Status
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SystemStatus {
    /// System uptime in seconds
    pub uptime_seconds: u64,

    /// Total number of active connections
    pub active_connections: usize,

    /// Total messages received from TAK servers
    pub messages_processed: u64,

    /// Total messages sent to TAK servers
    #[serde(default)]
    pub messages_sent: u64,

    /// Total connection errors
    #[serde(default)]
    pub errors: u64,

    /// Messages per second (last minute)
    pub messages_per_second: f64,

    /// Memory usage in bytes
    pub memory_usage_bytes: u64,

    /// Active filter rules
    pub active_filters: usize,

    /// System version
    pub version: String,

    /// Current timestamp
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Connection Management
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConnectionInfo {
    /// Unique connection identifier
    pub id: Uuid,

    /// Connection name/label
    pub name: String,

    /// Connection type
    pub connection_type: ConnectionType,

    /// Current status
    pub status: ConnectionStatus,

    /// Remote address
    pub address: String,

    /// Remote port
    pub port: u16,

    /// Messages received
    pub messages_received: u64,

    /// Messages sent
    pub messages_sent: u64,

    /// Bytes received
    pub bytes_received: u64,

    /// Bytes sent
    pub bytes_sent: u64,

    /// Connection established time
    pub connected_at: Option<DateTime<Utc>>,

    /// Last activity time
    pub last_activity: Option<DateTime<Utc>>,

    /// Error message if status is Error
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ConnectionList {
    pub connections: Vec<ConnectionInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateConnectionRequest {
    /// Connection name/label
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    /// Connection type
    pub connection_type: ConnectionType,

    /// Remote address (hostname or IP)
    #[validate(length(min = 1, max = 255))]
    pub address: String,

    /// Remote port (1-65535)
    #[validate(range(min = 1, max = 65535))]
    pub port: u16,

    /// Auto-reconnect on disconnect
    #[serde(default = "default_auto_reconnect")]
    pub auto_reconnect: bool,

    /// TLS certificate path (for TLS connections)
    #[validate(length(max = 500))]
    pub tls_cert_path: Option<String>,

    /// TLS key path (for TLS connections)
    #[validate(length(max = 500))]
    pub tls_key_path: Option<String>,

    /// Inline PEM client certificate (base64 of the PEM file), uploaded from the browser.
    /// When present, persisted server-side and used instead of tls_cert_path.
    #[serde(default)]
    pub tls_client_cert_pem_b64: Option<String>,

    /// Inline PEM client private key (base64 of the PEM file).
    #[serde(default)]
    pub tls_client_key_pem_b64: Option<String>,

    /// Inline PEM CA certificate (base64 of the PEM file).
    #[serde(default)]
    pub tls_ca_cert_pem_b64: Option<String>,

    /// Password for an encrypted PKCS#12 client certificate upload.
    #[serde(default)]
    pub tls_cert_password: Option<String>,

    /// Validate TLS certificates
    #[serde(default = "default_validate_certs")]
    pub validate_certs: bool,
}

fn default_auto_reconnect() -> bool {
    true
}

fn default_validate_certs() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateConnectionResponse {
    pub id: Uuid,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeleteConnectionResponse {
    pub message: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TestConnectionRequest {
    /// Target address as "host:port"
    pub address: String,

    /// Optional protocol hint (tcp/tls); informational only for the reachability probe
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TestConnectionResponse {
    pub success: bool,
    /// Round-trip time of the TCP connect, in milliseconds
    pub latency: u64,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReconnectResponse {
    pub id: Uuid,
    pub status: ConnectionStatus,
    pub message: String,
}

// ============================================================================
// Filter Management
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FilterRule {
    /// Unique filter identifier
    pub id: Uuid,

    /// Filter name/description
    pub name: String,

    /// Filter priority (higher = evaluated first)
    pub priority: i32,

    /// Filter action
    pub action: FilterAction,

    /// Match on CoT event type (regex)
    pub event_type: Option<String>,

    /// Match on UID pattern (regex)
    pub uid_pattern: Option<String>,

    /// Match on callsign pattern (regex)
    pub callsign_pattern: Option<String>,

    /// Match on source address
    pub source_address: Option<String>,

    /// Match on destination address
    pub destination_address: Option<String>,

    /// Geographic bounding box filter
    pub geo_bounds: Option<GeoBounds>,

    /// Filter is enabled
    pub enabled: bool,

    /// Number of messages matched
    pub match_count: u64,

    /// Created timestamp
    pub created_at: DateTime<Utc>,

    /// Last modified timestamp
    pub modified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GeoBounds {
    /// Minimum latitude
    pub min_lat: f64,

    /// Maximum latitude
    pub max_lat: f64,

    /// Minimum longitude
    pub min_lon: f64,

    /// Maximum longitude
    pub max_lon: f64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FilterList {
    pub filters: Vec<FilterRule>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateFilterRequest {
    /// Filter name/description
    #[validate(length(min = 1, max = 200))]
    pub name: String,

    /// Filter priority (higher = evaluated first)
    pub priority: i32,

    /// Filter action
    pub action: FilterAction,

    /// Match on CoT event type (regex)
    #[validate(length(max = 500))]
    pub event_type: Option<String>,

    /// Match on UID pattern (regex)
    #[validate(length(max = 500))]
    pub uid_pattern: Option<String>,

    /// Match on callsign pattern (regex)
    #[validate(length(max = 500))]
    pub callsign_pattern: Option<String>,

    /// Match on source address
    #[validate(length(max = 100))]
    pub source_address: Option<String>,

    /// Match on destination address
    #[validate(length(max = 100))]
    pub destination_address: Option<String>,

    /// Geographic bounding box filter
    pub geo_bounds: Option<GeoBounds>,

    /// Filter is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateFilterResponse {
    pub id: Uuid,
    pub message: String,
}

// ============================================================================
// Metrics
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MetricsSnapshot {
    /// Prometheus-formatted metrics
    pub metrics: String,

    /// Timestamp of snapshot
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Authentication
// ============================================================================

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    /// Username
    #[validate(length(min = 1, max = 100))]
    pub username: String,

    /// Password
    #[validate(length(min = 8))]
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoginResponse {
    /// JWT access token
    pub access_token: String,

    /// Token expiration time
    pub expires_at: DateTime<Utc>,

    /// User role
    pub role: UserRole,
}

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct ApiKeyRequest {
    /// API key name/description
    #[validate(length(min = 1, max = 200))]
    pub name: String,

    /// API key role
    pub role: UserRole,

    /// Expiration time (optional)
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ApiKeyResponse {
    /// API key (only shown once)
    pub api_key: String,

    /// Key ID
    pub id: Uuid,

    /// Key name
    pub name: String,

    /// Created timestamp
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// WebSocket Messages
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    /// Subscribe to CoT message stream
    Subscribe {
        /// Filter by event types (regex patterns)
        event_types: Option<Vec<String>>,

        /// Filter by UIDs (regex patterns)
        uids: Option<Vec<String>>,

        /// Filter by geographic bounds
        geo_bounds: Option<GeoBounds>,

        /// Use binary encoding (protobuf/msgpack)
        binary: bool,
    },

    /// Unsubscribe from stream
    Unsubscribe,

    /// Subscribe to system events
    SubscribeEvents,

    /// Unsubscribe from system events
    UnsubscribeEvents,

    /// Ping to keep connection alive
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMessage {
    /// CoT message data
    CotMessage {
        /// Message ID
        id: Uuid,

        /// Source connection ID
        source_connection: Uuid,

        /// CoT XML data
        data: String,

        /// Event type
        event_type: String,

        /// UID
        uid: String,

        /// Timestamp
        timestamp: DateTime<Utc>,
    },

    /// System event notification
    SystemEvent {
        /// Event type
        event: String,

        /// Event details
        details: serde_json::Value,

        /// Timestamp
        timestamp: DateTime<Utc>,
    },

    /// Error message
    Error {
        /// Error code
        code: String,

        /// Error message
        message: String,
    },

    /// Acknowledgement
    Ack {
        /// Message being acknowledged
        message_type: String,
    },

    /// Pong response
    Pong,
}

// ============================================================================
// Error Responses
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error code
    pub error: String,

    /// Human-readable error message
    pub message: String,

    /// Additional error details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,

    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

impl ErrorResponse {
    pub fn new(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            details: None,
            timestamp: Utc::now(),
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

// ============================================================================
// Audit Log
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditLogEntry {
    /// Log entry ID
    pub id: Uuid,

    /// User who performed the action
    pub user: String,

    /// User role at time of action
    pub role: UserRole,

    /// Action performed
    pub action: String,

    /// Resource affected
    pub resource: String,

    /// Request details
    pub details: serde_json::Value,

    /// Source IP address
    pub source_ip: String,

    /// Timestamp
    pub timestamp: DateTime<Utc>,

    /// Whether action succeeded
    pub success: bool,
}

// ============================================================================
// CoT Message Injection
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct SendCotRequest {
    /// CoT message in XML format
    #[validate(length(min = 1, max = 100000))]
    pub message: String,

    /// Optional: Send to specific connection(s) by ID
    /// If None, broadcasts to all connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_connections: Option<Vec<Uuid>>,

    /// Optional: Apply filters before sending
    /// If true, message goes through normal filter rules
    /// If false, bypasses filters and sends directly
    #[serde(default = "default_apply_filters")]
    pub apply_filters: bool,

    /// Optional: Priority for message routing (0-10, higher = more important)
    #[serde(default)]
    pub priority: u8,
}

fn default_apply_filters() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SendCotResponse {
    /// Unique message ID assigned
    pub message_id: Uuid,

    /// Number of connections the message was sent to
    pub sent_to_count: usize,

    /// List of connection IDs the message was sent to
    pub sent_to_connections: Vec<Uuid>,

    /// Any warnings during processing (e.g., missing fields)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub warnings: Vec<String>,

    /// Timestamp when message was processed
    pub timestamp: DateTime<Utc>,
}

// ============================================================================
// Enrollment (Data Package Server)
// ============================================================================

/// Request to create an enrollment token
#[derive(Debug, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateEnrollmentTokenRequest {
    /// Username/device name for the enrollment
    #[validate(length(min = 1, max = 100))]
    pub username: String,

    /// Validity period in hours (default: 24)
    #[serde(default = "default_token_validity_hours")]
    pub validity_hours: u32,

    /// Maximum number of uses (None = unlimited)
    pub max_uses: Option<u32>,

    /// Certificate validity in days (default: 365)
    #[serde(default = "default_cert_validity_days")]
    pub cert_validity_days: u32,
}

fn default_token_validity_hours() -> u32 {
    24
}

fn default_cert_validity_days() -> u32 {
    365
}

/// Response after creating an enrollment token
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateEnrollmentTokenResponse {
    /// Token ID
    pub id: String,

    /// The enrollment token (for URL)
    pub token: String,

    /// Full enrollment URL
    pub enrollment_url: String,

    /// Username this token is for
    pub username: String,

    /// When the token expires
    pub expires_at: DateTime<Utc>,

    /// Maximum uses allowed
    pub max_uses: Option<u32>,
}

/// Enrollment token info (admin view)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EnrollmentTokenInfo {
    /// Token ID
    pub id: String,

    /// Username/device name
    pub username: String,

    /// When the token was created
    pub created_at: DateTime<Utc>,

    /// When the token expires
    pub expires_at: DateTime<Utc>,

    /// Whether the token has been used
    pub used: bool,

    /// Use count
    pub use_count: u32,

    /// Maximum uses allowed
    pub max_uses: Option<u32>,

    /// Whether the token is still valid
    pub is_valid: bool,
}

/// List of enrollment tokens
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EnrollmentTokenList {
    pub tokens: Vec<EnrollmentTokenInfo>,
    pub total: usize,
}

/// Server configuration for data package
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ServerConnectionConfig {
    /// Server hostname or IP
    pub host: String,

    /// Streaming port (default: 8089 for TLS, 8087 for TCP)
    pub streaming_port: u16,

    /// API port (default: 8443)
    pub api_port: u16,

    /// Server description/name
    pub description: Option<String>,

    /// Whether to use TLS for streaming
    pub use_tls: bool,
}

impl Default for ServerConnectionConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            streaming_port: 8089,
            api_port: 8443,
            description: Some("OmniTAK Server".to_string()),
            use_tls: true,
        }
    }
}

/// Enrollment status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EnrollmentStatus {
    /// Whether enrollment is enabled
    pub enabled: bool,

    /// Number of active tokens
    pub active_tokens: usize,

    /// Number of enrolled clients
    pub enrolled_clients: usize,

    /// CA certificate info (subject, expiry)
    pub ca_info: Option<CaInfo>,
}

/// CA certificate information
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CaInfo {
    /// CA subject common name
    pub subject_cn: String,

    /// CA expiration date
    pub expires_at: String,

    /// Days until expiration
    pub days_until_expiry: i64,
}
