//! Enrollment endpoints for OmniTAK data package distribution
//!
//! Provides a simple enrollment system for ATAK clients:
//! 1. Admin creates enrollment tokens
//! 2. Users download data packages using tokens
//! 3. Data packages contain certificates + server configuration

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use dashmap::DashMap;
use omnitak_cert::generator::{
    CaConfig, ClientCertConfig, EnrollmentToken, GeneratedCa, GeneratedClientCert,
};
use omnitak_datapackage::{ContentType, DataPackageBuilder};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use validator::Validate;

use crate::auth::RequireAdmin;
use crate::middleware::AuditLogger;
use crate::rest::ApiError;
use crate::types::*;

// ============================================================================
// Enrollment State
// ============================================================================

/// Shared state for the enrollment service
#[derive(Clone)]
pub struct EnrollmentState {
    /// Certificate Authority
    pub ca: Arc<RwLock<Option<GeneratedCa>>>,
    /// Active enrollment tokens
    pub tokens: Arc<DashMap<String, EnrollmentToken>>,
    /// Server connection configuration
    pub server_config: Arc<RwLock<ServerConnectionConfig>>,
    /// Enrolled clients count
    pub enrolled_count: Arc<std::sync::atomic::AtomicUsize>,
    /// Audit logger
    pub audit_logger: Arc<AuditLogger>,
    /// CA storage path (for persistence)
    pub ca_cert_path: Option<std::path::PathBuf>,
    pub ca_key_path: Option<std::path::PathBuf>,
}

impl EnrollmentState {
    /// Create a new enrollment state
    pub fn new(audit_logger: Arc<AuditLogger>) -> Self {
        Self {
            ca: Arc::new(RwLock::new(None)),
            tokens: Arc::new(DashMap::new()),
            server_config: Arc::new(RwLock::new(ServerConnectionConfig::default())),
            enrolled_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            audit_logger,
            ca_cert_path: None,
            ca_key_path: None,
        }
    }

    /// Initialize or load the CA
    pub async fn initialize_ca(&self, config: Option<CaConfig>) -> Result<(), anyhow::Error> {
        let mut ca_lock = self.ca.write().await;

        // Try to load existing CA from disk
        if let (Some(cert_path), Some(key_path)) = (&self.ca_cert_path, &self.ca_key_path) {
            if cert_path.exists() && key_path.exists() {
                match GeneratedCa::from_files(cert_path, key_path) {
                    Ok(ca) => {
                        info!("Loaded existing CA from disk");
                        *ca_lock = Some(ca);
                        return Ok(());
                    }
                    Err(e) => {
                        warn!("Failed to load existing CA: {}, generating new one", e);
                    }
                }
            }
        }

        // Generate new CA
        let ca_config = config.unwrap_or_default();
        let ca = GeneratedCa::generate(&ca_config)?;

        // Save to disk if paths configured
        if let (Some(cert_path), Some(key_path)) = (&self.ca_cert_path, &self.ca_key_path) {
            ca.save_to_files(cert_path, key_path)?;
        }

        info!("Generated new CA: {}", ca_config.common_name);
        *ca_lock = Some(ca);
        Ok(())
    }

    /// Set server configuration
    pub async fn set_server_config(&self, config: ServerConnectionConfig) {
        let mut server_config = self.server_config.write().await;
        *server_config = config;
    }
}

// ============================================================================
// Router Setup
// ============================================================================

pub fn create_enrollment_router(state: EnrollmentState) -> Router {
    Router::new()
        // Public endpoint - download data package with token
        .route("/api/v1/enrollment/datapackage", get(download_datapackage))
        // Admin endpoints
        .route("/api/v1/enrollment/status", get(get_enrollment_status))
        .route("/api/v1/enrollment/tokens", get(list_tokens))
        .route("/api/v1/enrollment/tokens", post(create_token))
        .route("/api/v1/enrollment/tokens/{id}", delete(delete_token))
        .route("/api/v1/enrollment/config", get(get_server_config))
        .route("/api/v1/enrollment/config", post(update_server_config))
        .with_state(state)
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DatapackageQuery {
    /// Enrollment token
    pub token: String,
}

// ============================================================================
// Endpoints
// ============================================================================

/// GET /api/v1/enrollment/datapackage?token=xxx
/// Download a data package using an enrollment token
async fn download_datapackage(
    State(state): State<EnrollmentState>,
    Query(query): Query<DatapackageQuery>,
) -> Result<Response, ApiError> {
    info!("Data package download request with token");

    // Find and validate token
    let mut token_entry = state.tokens.get_mut(&query.token).ok_or_else(|| {
        warn!("Invalid enrollment token attempted");
        ApiError::Unauthorized("Invalid or expired enrollment token".to_string())
    })?;

    if !token_entry.is_valid() {
        warn!("Expired or exhausted token: {}", token_entry.id);
        return Err(ApiError::Unauthorized(
            "Token has expired or reached maximum uses".to_string(),
        ));
    }

    // Get CA
    let ca_lock = state.ca.read().await;
    let ca = ca_lock
        .as_ref()
        .ok_or_else(|| ApiError::InternalError("Enrollment CA not initialized".to_string()))?;

    // Get server config
    let server_config = state.server_config.read().await.clone();

    // Generate client certificate
    let client_config = ClientCertConfig::new(&token_entry.username).with_validity(365); // TODO: make configurable per token

    let client_cert = ca.issue_client_cert(&client_config).map_err(|e| {
        error!("Failed to issue client certificate: {}", e);
        ApiError::InternalError("Failed to generate certificate".to_string())
    })?;

    // Build data package
    let data_package = build_data_package(&client_cert, &server_config, &token_entry.username)
        .map_err(|e| {
            error!("Failed to build data package: {}", e);
            ApiError::InternalError("Failed to build data package".to_string())
        })?;

    // Mark token as used
    token_entry.mark_used();
    let username = token_entry.username.clone();
    drop(token_entry);

    // Increment enrolled count
    state
        .enrolled_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    info!("Data package generated for: {}", username);

    // Return as downloadable ZIP
    let filename = format!("omnitak-{}.zip", username);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from(data_package))
        .map_err(|e| ApiError::InternalError(format!("Response build error: {}", e)))?;

    Ok(response)
}

/// GET /api/v1/enrollment/status
/// Get enrollment service status (admin only)
async fn get_enrollment_status(
    State(state): State<EnrollmentState>,
    RequireAdmin(_user): RequireAdmin,
) -> Result<Json<EnrollmentStatus>, ApiError> {
    let ca_lock = state.ca.read().await;
    let ca_info = if let Some(ca) = ca_lock.as_ref() {
        // Parse CA cert to get info
        let ca_info_result = parse_ca_info(&ca.cert_pem);
        ca_info_result.ok()
    } else {
        None
    };

    // Count active tokens
    let active_tokens = state.tokens.iter().filter(|t| t.is_valid()).count();

    let enrolled_clients = state
        .enrolled_count
        .load(std::sync::atomic::Ordering::Relaxed);

    Ok(Json(EnrollmentStatus {
        enabled: ca_lock.is_some(),
        active_tokens,
        enrolled_clients,
        ca_info,
    }))
}

/// Parse CA certificate PEM to extract info
fn parse_ca_info(cert_pem: &str) -> Result<CaInfo, anyhow::Error> {
    let mut reader = std::io::BufReader::new(cert_pem.as_bytes());
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("Failed to parse certificate: {}", e))?;

    let cert_der = certs
        .first()
        .ok_or_else(|| anyhow::anyhow!("No certificate found"))?;

    let cert_info = omnitak_cert::CertificateInfo::from_der(cert_der.as_ref())?;

    Ok(CaInfo {
        subject_cn: cert_info.subject_cn,
        expires_at: cert_info.not_after,
        days_until_expiry: cert_info.days_until_expiry,
    })
}

/// GET /api/v1/enrollment/tokens
/// List all enrollment tokens (admin only)
async fn list_tokens(
    State(state): State<EnrollmentState>,
    RequireAdmin(_user): RequireAdmin,
) -> Result<Json<EnrollmentTokenList>, ApiError> {
    let tokens: Vec<EnrollmentTokenInfo> = state
        .tokens
        .iter()
        .map(|entry| {
            let token = entry.value();
            EnrollmentTokenInfo {
                id: token.id.clone(),
                username: token.username.clone(),
                created_at: token.created_at,
                expires_at: token.expires_at,
                used: token.used,
                use_count: token.use_count,
                max_uses: token.max_uses,
                is_valid: token.is_valid(),
            }
        })
        .collect();

    let total = tokens.len();

    Ok(Json(EnrollmentTokenList { tokens, total }))
}

/// POST /api/v1/enrollment/tokens
/// Create a new enrollment token (admin only)
async fn create_token(
    State(state): State<EnrollmentState>,
    RequireAdmin(user): RequireAdmin,
    Json(request): Json<CreateEnrollmentTokenRequest>,
) -> Result<(StatusCode, Json<CreateEnrollmentTokenResponse>), ApiError> {
    request.validate()?;

    // Check if CA is initialized
    let ca_lock = state.ca.read().await;
    if ca_lock.is_none() {
        return Err(ApiError::BadRequest(
            "Enrollment CA not initialized".to_string(),
        ));
    }
    drop(ca_lock);

    // Create token
    let token = EnrollmentToken::new(&request.username, request.validity_hours, request.max_uses);

    let token_string = token.token.clone();
    let token_id = token.id.clone();
    let expires_at = token.expires_at;

    // Store token
    state.tokens.insert(token_string.clone(), token);

    // Get server config for URL
    let server_config = state.server_config.read().await;
    let enrollment_url = format!(
        "https://{}:{}/api/v1/enrollment/datapackage?token={}",
        server_config.host, server_config.api_port, token_string
    );
    drop(server_config);

    info!(
        "Created enrollment token for: {} (id: {})",
        request.username, token_id
    );

    // Audit log
    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "admin".to_string()),
        user.role,
        "create_enrollment_token".to_string(),
        format!("/api/v1/enrollment/tokens/{}", token_id),
        serde_json::json!({
            "username": request.username,
            "validity_hours": request.validity_hours,
            "max_uses": request.max_uses,
        }),
        "0.0.0.0".to_string(),
        true,
    );

    Ok((
        StatusCode::CREATED,
        Json(CreateEnrollmentTokenResponse {
            id: token_id,
            token: token_string,
            enrollment_url,
            username: request.username,
            expires_at,
            max_uses: request.max_uses,
        }),
    ))
}

/// DELETE /api/v1/enrollment/tokens/{id}
/// Delete an enrollment token (admin only)
async fn delete_token(
    State(state): State<EnrollmentState>,
    Path(id): Path<String>,
    RequireAdmin(user): RequireAdmin,
) -> Result<Json<DeleteConnectionResponse>, ApiError> {
    // Find token by ID
    let token_key = state
        .tokens
        .iter()
        .find(|entry| entry.value().id == id)
        .map(|entry| entry.key().clone());

    match token_key {
        Some(key) => {
            state.tokens.remove(&key);
            info!("Deleted enrollment token: {}", id);

            state.audit_logger.log(
                user.user_id.unwrap_or_else(|| "admin".to_string()),
                user.role,
                "delete_enrollment_token".to_string(),
                format!("/api/v1/enrollment/tokens/{}", id),
                serde_json::json!({"token_id": id}),
                "0.0.0.0".to_string(),
                true,
            );

            Ok(Json(DeleteConnectionResponse {
                message: "Enrollment token deleted successfully".to_string(),
            }))
        }
        None => Err(ApiError::NotFound(format!("Token {} not found", id))),
    }
}

/// GET /api/v1/enrollment/config
/// Get server connection configuration (admin only)
async fn get_server_config(
    State(state): State<EnrollmentState>,
    RequireAdmin(_user): RequireAdmin,
) -> Result<Json<ServerConnectionConfig>, ApiError> {
    let config = state.server_config.read().await.clone();
    Ok(Json(config))
}

/// POST /api/v1/enrollment/config
/// Update server connection configuration (admin only)
async fn update_server_config(
    State(state): State<EnrollmentState>,
    RequireAdmin(user): RequireAdmin,
    Json(config): Json<ServerConnectionConfig>,
) -> Result<Json<ServerConnectionConfig>, ApiError> {
    state.set_server_config(config.clone()).await;

    info!("Updated enrollment server config: host={}", config.host);

    state.audit_logger.log(
        user.user_id.unwrap_or_else(|| "admin".to_string()),
        user.role,
        "update_enrollment_config".to_string(),
        "/api/v1/enrollment/config".to_string(),
        serde_json::to_value(&config).unwrap(),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(Json(config))
}

// ============================================================================
// Data Package Building
// ============================================================================

/// Build a TAK data package with client certificate and server configuration
fn build_data_package(
    client_cert: &GeneratedClientCert,
    server_config: &ServerConnectionConfig,
    username: &str,
) -> Result<Vec<u8>, anyhow::Error> {
    // Generate PKCS#12 with password
    let p12_password = "atakatak"; // Standard ATAK default password
    let p12_data = client_cert.to_pkcs12(p12_password)?;

    // Generate CA truststore (also as P12)
    let ca_truststore = create_ca_truststore(&client_cert.ca_cert_pem, p12_password)?;

    // Generate server preferences XML
    let prefs_xml = generate_atak_preferences(server_config, username);

    // Build the data package
    let package = DataPackageBuilder::new(&format!("omnitak-{}", username))
        .add_bytes(
            &format!("{}.p12", username),
            p12_data,
            ContentType::Certificate,
        )?
        .add_bytes(
            "truststore-omnitak-ca.p12",
            ca_truststore,
            ContentType::Certificate,
        )?
        .add_bytes(
            "omnitak-server.pref",
            prefs_xml.into_bytes(),
            ContentType::Configuration,
        )?
        .on_receive_delete(false)
        .build_to_memory()?;

    Ok(package)
}

/// Create a CA truststore in PKCS#12 format
fn create_ca_truststore(ca_cert_pem: &str, password: &str) -> Result<Vec<u8>, anyhow::Error> {
    use p12::PFX;

    // Parse CA certificate
    let ca_cert_der = {
        let mut reader = std::io::BufReader::new(ca_cert_pem.as_bytes());
        let certs: Vec<_> = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
        certs
            .first()
            .ok_or_else(|| anyhow::anyhow!("No CA certificate found"))?
            .to_vec()
    };

    // Create truststore (certificate only, no private key)
    // Note: For truststores, we create a special P12 with just the CA cert
    let pfx = PFX::new(&ca_cert_der, &[], None, password, "OmniTAK-CA")
        .ok_or_else(|| anyhow::anyhow!("Failed to create truststore PKCS#12"))?;

    Ok(pfx.to_der())
}

/// Generate ATAK preferences XML for server connection
fn generate_atak_preferences(server_config: &ServerConnectionConfig, username: &str) -> String {
    let protocol = if server_config.use_tls { "ssl" } else { "tcp" };
    let connect_string = format!(
        "{}:{}:{}",
        server_config.host, server_config.streaming_port, protocol
    );

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<preferences>
    <preference version="1" name="cot_streams">
        <entry key="count" class="class java.lang.Integer">1</entry>
        <entry key="description0" class="class java.lang.String">{description}</entry>
        <entry key="enabled0" class="class java.lang.Boolean">true</entry>
        <entry key="connectString0" class="class java.lang.String">{connect_string}</entry>
        <entry key="caLocation0" class="class java.lang.String">cert/truststore-omnitak-ca.p12</entry>
        <entry key="caPassword0" class="class java.lang.String">atakatak</entry>
        <entry key="certificateLocation0" class="class java.lang.String">cert/{username}.p12</entry>
        <entry key="clientPassword0" class="class java.lang.String">atakatak</entry>
        <entry key="useAuth0" class="class java.lang.Boolean">true</entry>
        <entry key="enrollForCertificateWithTrust0" class="class java.lang.Boolean">false</entry>
    </preference>
    <preference version="1" name="com.atakmap.app_preferences">
        <entry key="locationCallsign" class="class java.lang.String">{username}</entry>
    </preference>
</preferences>
"#,
        description = server_config
            .description
            .as_deref()
            .unwrap_or("OmniTAK Server"),
        connect_string = connect_string,
        username = username,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_preferences() {
        let config = ServerConnectionConfig {
            host: "tak.example.com".to_string(),
            streaming_port: 8089,
            api_port: 8443,
            description: Some("Test Server".to_string()),
            use_tls: true,
        };

        let prefs = generate_atak_preferences(&config, "testuser");
        assert!(prefs.contains("tak.example.com:8089:ssl"));
        assert!(prefs.contains("testuser"));
        assert!(prefs.contains("Test Server"));
    }
}
