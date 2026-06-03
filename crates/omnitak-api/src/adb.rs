//! ADB integration endpoints for pulling certificates from connected Android devices

use crate::auth::AuthUser;
use axum::{extract::State, Json};
use omnitak_adb::{AdbClient, AtakPackage, TakCertificateBundle};
use omnitak_client::tls::{FramingMode, TlsCertConfig, TlsCertSource, TlsClient, TlsClientConfig};
use omnitak_client::ClientConfig;
use omnitak_core::ConnectionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{error, info, warn};
use validator::Validate;

use super::rest::{ApiError, ApiState};
use super::types::ConnectionInfo;

// ============================================================================
// Types
// ============================================================================

/// Request to pull certificates from device
#[derive(Debug, Deserialize, Validate)]
pub struct PullCertsRequest {
    /// Device serial number (optional, auto-detect if single device)
    pub device_serial: Option<String>,
    /// ATAK package name (defaults to civilian ATAK)
    #[serde(default = "default_package")]
    pub package: String,
    /// Output directory for certificates (defaults to "certs/from-device")
    #[serde(default = "default_cert_dir")]
    pub cert_dir: String,
    /// Automatically create TAK connection after pulling certs
    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,
}

fn default_package() -> String {
    "com.atakmap.app.civ".to_string()
}

fn default_cert_dir() -> String {
    "certs/from-device".to_string()
}

fn default_auto_connect() -> bool {
    true
}

/// Response after pulling certificates
#[derive(Debug, Serialize)]
pub struct PullCertsResponse {
    /// Success flag
    pub success: bool,
    /// Message
    pub message: String,
    /// Certificate bundle information
    pub bundle: Option<CertificateBundleInfo>,
    /// Connection ID if auto-connect was enabled
    pub connection_id: Option<String>,
}

/// Certificate bundle information
#[derive(Debug, Serialize)]
pub struct CertificateBundleInfo {
    /// Server address
    pub server_address: Option<String>,
    /// Server name
    pub server_name: String,
    /// Number of certificate files pulled
    pub cert_count: usize,
    /// Certificate file paths
    pub cert_files: Vec<String>,
}

/// List of connected ADB devices
#[derive(Debug, Serialize)]
pub struct DeviceListResponse {
    /// Connected devices
    pub devices: Vec<DeviceInfoResponse>,
}

/// Device information
#[derive(Debug, Serialize)]
pub struct DeviceInfoResponse {
    /// Device serial number
    pub serial: String,
    /// Device state
    pub state: String,
    /// Device model
    pub model: Option<String>,
    /// Device product
    pub product: Option<String>,
}

// ============================================================================
// Endpoints
// ============================================================================

/// GET /api/v1/adb/devices - List connected Android devices
pub async fn list_devices(_user: AuthUser) -> Result<Json<DeviceListResponse>, ApiError> {
    let adb = AdbClient::new();

    // Check if ADB is available
    if !adb.is_available() {
        return Err(ApiError::BadRequest(
            "ADB is not available. Make sure Android SDK Platform-Tools is installed.".to_string(),
        ));
    }

    let devices = adb
        .list_devices()
        .map_err(|e| ApiError::InternalError(format!("Failed to list devices: {}", e)))?;

    let devices = devices
        .into_iter()
        .map(|d| DeviceInfoResponse {
            serial: d.serial,
            state: d.state,
            model: d.model,
            product: d.product,
        })
        .collect();

    Ok(Json(DeviceListResponse { devices }))
}

/// POST /api/v1/adb/pull-certs - Pull certificates from connected device
pub async fn pull_certificates(
    State(state): State<ApiState>,
    _user: AuthUser,
    Json(req): Json<PullCertsRequest>,
) -> Result<Json<PullCertsResponse>, ApiError> {
    req.validate()
        .map_err(|e| ApiError::BadRequest(format!("Invalid request: {}", e)))?;

    let adb = AdbClient::new();

    // Check if ADB is available
    if !adb.is_available() {
        return Err(ApiError::BadRequest(
            "ADB is not available. Make sure Android SDK Platform-Tools is installed.".to_string(),
        ));
    }

    // Get device
    let device = if let Some(serial) = req.device_serial {
        adb.get_device(&serial)
            .map_err(|e| ApiError::NotFound(format!("Device not found: {}", e)))?
    } else {
        adb.auto_detect_device()
            .map_err(|e| ApiError::BadRequest(format!("Failed to detect device: {}", e)))?
    };

    info!("Pulling certificates from device: {}", device.serial);

    // Save device serial for later use
    let device_serial = device.serial.clone();

    // Determine ATAK package
    let package = match req.package.as_str() {
        "com.atakmap.app.civ" => AtakPackage::Civilian,
        "com.atakmap.app.mil" => AtakPackage::Military,
        _ => AtakPackage::Custom(Box::leak(req.package.clone().into_boxed_str())),
    };

    // Create output directory
    let cert_dir = PathBuf::from(&req.cert_dir);
    tokio::fs::create_dir_all(&cert_dir).await.map_err(|e| {
        ApiError::InternalError(format!("Failed to create certificate directory: {}", e))
    })?;

    // Pull certificates
    let bundle = device
        .pull_tak_certificates(&cert_dir, package)
        .map_err(|e| ApiError::InternalError(format!("Failed to pull certificates: {}", e)))?;

    info!(
        "Successfully pulled {} certificate files",
        bundle.certificates.len()
    );

    // Build response
    let bundle_info = CertificateBundleInfo {
        server_address: bundle.server_address.clone(),
        server_name: bundle.server_name.clone(),
        cert_count: bundle.certificates.len(),
        cert_files: bundle
            .certificates
            .iter()
            .map(|c| c.local_path.display().to_string())
            .collect(),
    };

    // Auto-connect if requested
    let connection_id = if req.auto_connect {
        match create_connection_from_bundle(state, bundle, device_serial).await {
            Ok(id) => {
                info!("Successfully created connection: {}", id);
                Some(id)
            }
            Err(e) => {
                warn!("Failed to auto-create connection: {}", e);
                None
            }
        }
    } else {
        None
    };

    Ok(Json(PullCertsResponse {
        success: true,
        message: format!(
            "Successfully pulled {} certificate files from device",
            bundle_info.cert_count
        ),
        bundle: Some(bundle_info),
        connection_id,
    }))
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a TAK connection from certificate bundle
async fn create_connection_from_bundle(
    state: ApiState,
    bundle: TakCertificateBundle,
    device_serial: String,
) -> anyhow::Result<String> {
    info!("Creating TAK connection from certificate bundle...");

    // Find P12 file or PEM files
    let p12_file = bundle
        .certificates
        .iter()
        .find(|c| c.cert_type == omnitak_adb::CertificateType::P12);

    let client_cert = bundle
        .certificates
        .iter()
        .find(|c| c.cert_type == omnitak_adb::CertificateType::Client);

    let client_key = bundle
        .certificates
        .iter()
        .find(|c| c.cert_type == omnitak_adb::CertificateType::Key);

    let ca_cert = bundle
        .certificates
        .iter()
        .find(|c| c.cert_type == omnitak_adb::CertificateType::CA);

    // Determine certificate source
    let cert_source = if let Some(p12) = p12_file {
        info!("Using PKCS#12 certificate: {}", p12.local_path.display());

        // Note: PKCS#12 requires password, which we don't have
        // User will need to convert to PEM or provide password
        warn!("PKCS#12 files require password. Please convert to PEM format:");
        warn!(
            "  openssl pkcs12 -in {} -out client.pem -clcerts -nokeys",
            p12.local_path.display()
        );
        warn!(
            "  openssl pkcs12 -in {} -out client.key -nocerts -nodes",
            p12.local_path.display()
        );
        warn!(
            "  openssl pkcs12 -in {} -out ca.pem -cacerts -nokeys",
            p12.local_path.display()
        );

        return Err(anyhow::anyhow!(
            "PKCS#12 certificates require password. Please convert to PEM format first."
        ));
    } else if let (Some(cert), Some(key), Some(ca)) = (client_cert, client_key, ca_cert) {
        info!("Using PEM certificates");
        TlsCertSource::Files {
            cert_path: cert.local_path.clone(),
            key_path: key.local_path.clone(),
            ca_cert_path: Some(ca.local_path.clone()),
        }
    } else {
        return Err(anyhow::anyhow!("No valid certificate files found"));
    };

    // Create connection ID
    let connection_id = ConnectionId::new();

    // Determine server address
    let server_address = bundle
        .server_address
        .or_else(|| {
            bundle
                .server_host
                .map(|h| format!("{}:{}", h, bundle.server_port))
        })
        .unwrap_or_else(|| format!("takserver.example.com:{}", bundle.server_port));

    info!("Connecting to TAK server: {}", server_address);

    // Parse host and port from server address
    let (host, port) = server_address
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Invalid server address format"))?;

    // Create TLS client configuration
    let tls_config = TlsClientConfig {
        base: ClientConfig {
            server_addr: server_address.clone(),
            reconnect: omnitak_client::ReconnectConfig {
                enabled: true,
                initial_backoff: std::time::Duration::from_secs(1),
                max_backoff: std::time::Duration::from_secs(60),
                backoff_multiplier: 2.0,
                max_attempts: None,
            },
            ..Default::default()
        },
        cert_config: TlsCertConfig {
            source: cert_source,
        },
        server_name: Some(host.to_string()),
        tls13_only: false,
        verify_server: true,
        framing: FramingMode::Xml,
    };

    // Create TLS client
    let mut client = TlsClient::new(tls_config)
        .map_err(|e| anyhow::anyhow!("Failed to create TLS client: {}", e))?;

    // Spawn connection task
    let conn_id = connection_id;
    tokio::spawn(async move {
        info!(id = %conn_id, "Connecting TLS client from ADB");
        if let Err(e) = client.connect_only().await {
            error!(id = %conn_id, error = %e, "Failed to connect TLS client");
        } else {
            info!(id = %conn_id, "TLS client connected from ADB");
        }
    });

    // Add to connections list
    let conn_info = ConnectionInfo {
        id: *connection_id.as_uuid(),
        name: format!("ADB-{}", device_serial),
        connection_type: crate::types::ConnectionType::TlsClient,
        status: crate::types::ConnectionStatus::Connecting,
        address: host.to_string(),
        port: port.parse().unwrap_or(8089),
        messages_sent: 0,
        messages_received: 0,
        bytes_sent: 0,
        bytes_received: 0,
        connected_at: Some(chrono::Utc::now()),
        last_activity: Some(chrono::Utc::now()),
        error: None,
    };

    state.connections.write().await.push(conn_info);

    Ok(connection_id.to_string())
}
