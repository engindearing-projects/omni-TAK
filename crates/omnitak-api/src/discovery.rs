//! Discovery REST API endpoints for managing mDNS service discovery

use crate::auth::{AuthUser, RequireOperator};
use crate::rest::{ApiError, ApiState};
use crate::types::ErrorResponse;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use omnitak_discovery::{DiscoveredService, DiscoveryService, ServiceStatus, ServiceType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use validator::Validate;

// ============================================================================
// Response Types
// ============================================================================

/// Response containing list of discovered services
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(utoipa::ToSchema)]
pub struct DiscoveredServicesList {
    /// Total number of services
    pub total: usize,

    /// List of discovered services
    pub services: Vec<DiscoveredServiceResponse>,

    /// Response timestamp
    pub timestamp: DateTime<Utc>,
}

/// Discovered service information for API responses
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(utoipa::ToSchema)]
pub struct DiscoveredServiceResponse {
    /// Unique service ID
    pub id: String,

    /// Service type
    pub service_type: String,

    /// Instance name
    pub instance_name: String,

    /// Hostname
    pub hostname: String,

    /// IP addresses
    pub addresses: Vec<String>,

    /// Service port
    pub port: u16,

    /// Connection string (host:port)
    pub connection_string: String,

    /// TLS support
    pub supports_tls: bool,

    /// Service status
    pub status: ServiceStatus,

    /// Service properties/metadata
    pub properties: std::collections::HashMap<String, String>,

    /// First discovered timestamp
    pub discovered_at: DateTime<Utc>,

    /// Last seen timestamp
    pub last_seen_at: DateTime<Utc>,

    /// Number of times seen
    pub seen_count: u64,

    /// Age in seconds since last seen
    pub age_seconds: i64,
}

impl From<DiscoveredService> for DiscoveredServiceResponse {
    fn from(service: DiscoveredService) -> Self {
        Self {
            id: service.id.clone(),
            service_type: format!("{:?}", service.service_type),
            instance_name: service.instance_name.clone(),
            hostname: service.hostname.clone(),
            addresses: service.addresses.iter().map(|a| a.to_string()).collect(),
            port: service.port,
            connection_string: service.connection_string(),
            supports_tls: service.supports_tls(),
            status: service.status,
            properties: service.properties.clone(),
            discovered_at: service.discovered_at,
            last_seen_at: service.last_seen_at,
            seen_count: service.seen_count,
            age_seconds: service.age_seconds(),
        }
    }
}

/// Discovery status response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(utoipa::ToSchema)]
pub struct DiscoveryStatusResponse {
    /// Whether discovery is running
    pub running: bool,

    /// Total services discovered
    pub total_services: usize,

    /// Active services
    pub active_services: usize,

    /// Stale services
    pub stale_services: usize,

    /// Response timestamp
    pub timestamp: DateTime<Utc>,
}

/// Request to manually trigger discovery refresh
#[derive(Debug, Serialize, Deserialize, Validate, utoipa::ToSchema)]
pub struct RefreshRequest {
    /// Optional service type to refresh (or all if not specified)
    pub service_type: Option<String>,
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DiscoveryQuery {
    /// Filter by service type
    pub service_type: Option<String>,

    /// Filter by status
    pub status: Option<ServiceStatus>,

    /// Only show services with TLS support
    #[serde(default)]
    pub tls_only: bool,
}

// ============================================================================
// API Endpoints
// ============================================================================

/// GET /api/v1/discovery/status - Get discovery service status
#[utoipa::path(
    get,
    path = "/api/v1/discovery/status",
    responses(
        (status = 200, description = "Discovery status retrieved successfully", body = DiscoveryStatusResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn get_discovery_status(
    State(state): State<ApiState>,
    _user: AuthUser,
) -> Result<Json<DiscoveryStatusResponse>, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    let services = discovery.get_discovered_services().await;

    let active_count = services
        .iter()
        .filter(|s| s.status == ServiceStatus::Active)
        .count();

    let stale_count = services
        .iter()
        .filter(|s| s.status == ServiceStatus::Stale)
        .count();

    let status = DiscoveryStatusResponse {
        running: discovery.is_running(),
        total_services: services.len(),
        active_services: active_count,
        stale_services: stale_count,
        timestamp: Utc::now(),
    };

    Ok(Json(status))
}

/// GET /api/v1/discovery/services - List all discovered services
#[utoipa::path(
    get,
    path = "/api/v1/discovery/services",
    params(
        ("service_type" = Option<String>, Query, description = "Filter by service type"),
        ("status" = Option<ServiceStatus>, Query, description = "Filter by status"),
        ("tls_only" = Option<bool>, Query, description = "Only show TLS-enabled services")
    ),
    responses(
        (status = 200, description = "Discovered services list", body = DiscoveredServicesList),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn list_discovered_services(
    State(state): State<ApiState>,
    Query(query): Query<DiscoveryQuery>,
    _user: AuthUser,
) -> Result<Json<DiscoveredServicesList>, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    let mut services = discovery.get_discovered_services().await;

    // Apply filters
    if let Some(ref service_type_str) = query.service_type {
        let service_type = ServiceType::from_service_string(service_type_str);
        services.retain(|s| s.service_type == service_type);
    }

    if let Some(status) = query.status {
        services.retain(|s| s.status == status);
    }

    if query.tls_only {
        services.retain(|s| s.supports_tls());
    }

    let response_services: Vec<DiscoveredServiceResponse> =
        services.into_iter().map(|s| s.into()).collect();

    let list = DiscoveredServicesList {
        total: response_services.len(),
        services: response_services,
        timestamp: Utc::now(),
    };

    Ok(Json(list))
}

/// GET /api/v1/discovery/services/{id} - Get a specific discovered service
#[utoipa::path(
    get,
    path = "/api/v1/discovery/services/{id}",
    params(
        ("id" = String, Path, description = "Service ID")
    ),
    responses(
        (status = 200, description = "Service details", body = DiscoveredServiceResponse),
        (status = 404, description = "Service not found", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn get_discovered_service(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    _user: AuthUser,
) -> Result<Json<DiscoveredServiceResponse>, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    let service = discovery
        .get_service(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Service not found: {}", id)))?;

    Ok(Json(service.into()))
}

/// POST /api/v1/discovery/refresh - Manually trigger discovery refresh
#[utoipa::path(
    post,
    path = "/api/v1/discovery/refresh",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Refresh triggered successfully"),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - requires operator role", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn refresh_discovery(
    State(state): State<ApiState>,
    _operator: RequireOperator,
    Json(_request): Json<RefreshRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    info!("Manually triggering discovery refresh");

    discovery
        .refresh()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to refresh discovery: {}", e)))?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "Discovery refresh triggered",
            "timestamp": Utc::now(),
        })),
    ))
}

/// GET /api/v1/discovery/tak-servers - Get discovered TAK servers only
#[utoipa::path(
    get,
    path = "/api/v1/discovery/tak-servers",
    responses(
        (status = 200, description = "Discovered TAK servers", body = DiscoveredServicesList),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn list_tak_servers(
    State(state): State<ApiState>,
    _user: AuthUser,
) -> Result<Json<DiscoveredServicesList>, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    let services = discovery
        .get_services_by_type(&ServiceType::TakServer)
        .await;

    let response_services: Vec<DiscoveredServiceResponse> =
        services.into_iter().map(|s| s.into()).collect();

    let list = DiscoveredServicesList {
        total: response_services.len(),
        services: response_services,
        timestamp: Utc::now(),
    };

    Ok(Json(list))
}

/// GET /api/v1/discovery/atak-devices - Get discovered ATAK devices only
#[utoipa::path(
    get,
    path = "/api/v1/discovery/atak-devices",
    responses(
        (status = 200, description = "Discovered ATAK devices", body = DiscoveredServicesList),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
pub async fn list_atak_devices(
    State(state): State<ApiState>,
    _user: AuthUser,
) -> Result<Json<DiscoveredServicesList>, ApiError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Discovery service not enabled".to_string()))?;

    let services = discovery
        .get_services_by_type(&ServiceType::AtakDevice)
        .await;

    let response_services: Vec<DiscoveredServiceResponse> =
        services.into_iter().map(|s| s.into()).collect();

    let list = DiscoveredServicesList {
        total: response_services.len(),
        services: response_services,
        timestamp: Utc::now(),
    };

    Ok(Json(list))
}
