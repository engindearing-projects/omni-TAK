//! Plugin management REST API endpoints

use super::ApiError;
use crate::auth::{AuthUser, RequireAdmin, RequireOperator};
use crate::middleware::AuditLogger;
use crate::types::ErrorResponse;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use omnitak_plugin_api::{
    FilterMetadata, PluginCapability, PluginInfo, PluginManager, TransformerMetadata,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use utoipa::ToSchema;
use validator::Validate;

// ============================================================================
// State
// ============================================================================

#[derive(Clone)]
pub struct PluginApiState {
    pub plugin_manager: Arc<RwLock<PluginManager>>,
    pub audit_logger: Arc<AuditLogger>,
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginListResponse {
    pub plugins: Vec<PluginInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginDetailsResponse {
    pub info: PluginInfo,
    pub enabled: bool,
    pub loaded_at: Option<String>,
    pub execution_count: u64,
    pub error_count: u64,
    pub avg_execution_time_ms: f64,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginRequest {
    #[validate(length(min = 1, max = 100))]
    pub id: String,

    #[validate(length(min = 1, max = 500))]
    pub path: String,

    #[serde(default)]
    pub enabled: bool,

    pub plugin_type: PluginType,

    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    Filter,
    Transformer,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePluginConfigRequest {
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TogglePluginRequest {
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginMetricsResponse {
    pub plugin_id: String,
    pub execution_count: u64,
    pub error_count: u64,
    pub timeout_count: u64,
    pub avg_execution_time_ms: f64,
    pub p50_execution_time_ms: f64,
    pub p95_execution_time_ms: f64,
    pub p99_execution_time_ms: f64,
    pub last_execution: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthResponse {
    pub plugin_id: String,
    pub status: PluginStatus,
    pub health_check_time: String,
    pub uptime_seconds: u64,
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Disabled,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginQueryParams {
    #[serde(default)]
    pub enabled_only: bool,

    #[serde(default)]
    pub plugin_type: Option<String>,
}

// ============================================================================
// Router Setup
// ============================================================================

pub fn create_plugin_router(state: PluginApiState) -> Router {
    Router::new()
        // List and load plugins
        .route("/api/v1/plugins", get(list_plugins))
        .route("/api/v1/plugins", post(load_plugin))
        // Plugin details and management
        .route("/api/v1/plugins/{id}", get(get_plugin_details))
        .route("/api/v1/plugins/{id}", delete(unload_plugin))
        // Plugin configuration
        .route("/api/v1/plugins/{id}/config", put(update_plugin_config))
        .route("/api/v1/plugins/{id}/toggle", post(toggle_plugin))
        // Plugin metrics and health
        .route("/api/v1/plugins/{id}/metrics", get(get_plugin_metrics))
        .route("/api/v1/plugins/{id}/health", get(get_plugin_health))
        // Plugin reload
        .route("/api/v1/plugins/{id}/reload", post(reload_plugin))
        // Bulk operations
        .route("/api/v1/plugins/reload-all", post(reload_all_plugins))
        .with_state(state)
}

// ============================================================================
// Endpoints
// ============================================================================

/// GET /api/v1/plugins - List all plugins
#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    params(PluginQueryParams),
    responses(
        (status = 200, description = "Plugins retrieved successfully", body = PluginListResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn list_plugins(
    State(state): State<PluginApiState>,
    Query(params): Query<PluginQueryParams>,
    _user: AuthUser,
) -> Result<Json<PluginListResponse>, ApiError> {
    let manager = state.plugin_manager.read().await;
    let mut plugins = manager.list_plugins();

    // Apply filters
    if params.enabled_only {
        // TODO: Track enabled state in plugin info
    }

    if let Some(plugin_type) = params.plugin_type {
        plugins.retain(|p| {
            p.capabilities.iter().any(|cap| match plugin_type.as_str() {
                "filter" => matches!(cap, PluginCapability::Filter),
                "transformer" => matches!(cap, PluginCapability::Transform),
                _ => false,
            })
        });
    }

    let total = plugins.len();

    Ok(Json(PluginListResponse { plugins, total }))
}

/// POST /api/v1/plugins - Load a new plugin
#[utoipa::path(
    post,
    path = "/api/v1/plugins",
    request_body = LoadPluginRequest,
    responses(
        (status = 201, description = "Plugin loaded successfully", body = PluginInfo),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden - Admin required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["admin"]),
        ("api_key" = ["admin"])
    )
)]
async fn load_plugin(
    State(state): State<PluginApiState>,
    _admin: RequireAdmin,
    Json(req): Json<LoadPluginRequest>,
) -> Result<(StatusCode, Json<PluginInfo>), ApiError> {
    req.validate()?;

    info!("Loading plugin: {} from {}", req.id, req.path);

    let manager = state.plugin_manager.write().await;

    // Load based on type
    let plugin_info = match req.plugin_type {
        PluginType::Filter => {
            // Parse metadata from config
            let metadata: FilterMetadata = serde_json::from_value(req.config.clone())
                .map_err(|e| ApiError::BadRequest(format!("Invalid filter metadata: {}", e)))?;

            let _plugin = manager
                .load_filter_plugin(&req.path, metadata)
                .map_err(|e| ApiError::InternalError(format!("Failed to load plugin: {}", e)))?;

            // Get plugin info from manager
            manager
                .list_plugins()
                .into_iter()
                .find(|p| p.id == req.id)
                .ok_or_else(|| {
                    ApiError::InternalError("Plugin loaded but not found in list".to_string())
                })?
        }
        PluginType::Transformer => {
            let metadata: TransformerMetadata = serde_json::from_value(req.config.clone())
                .map_err(|e| {
                    ApiError::BadRequest(format!("Invalid transformer metadata: {}", e))
                })?;

            let _plugin = manager
                .load_transformer_plugin(&req.path, metadata)
                .map_err(|e| ApiError::InternalError(format!("Failed to load plugin: {}", e)))?;

            manager
                .list_plugins()
                .into_iter()
                .find(|p| p.id == req.id)
                .ok_or_else(|| {
                    ApiError::InternalError("Plugin loaded but not found in list".to_string())
                })?
        }
    };

    info!("Plugin loaded successfully: {}", req.id);

    // Audit log
    state.audit_logger.log(
        "admin".to_string(),
        crate::types::UserRole::Admin,
        "load_plugin".to_string(),
        "/api/v1/plugins".to_string(),
        serde_json::json!({"plugin_id": req.id, "path": req.path}),
        "0.0.0.0".to_string(), // TODO: Get actual IP from request
        true,
    );

    Ok((StatusCode::CREATED, Json(plugin_info)))
}

/// GET /api/v1/plugins/{id} - Get plugin details
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{id}",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    responses(
        (status = 200, description = "Plugin details retrieved", body = PluginDetailsResponse),
        (status = 404, description = "Plugin not found", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_plugin_details(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _user: AuthUser,
) -> Result<Json<PluginDetailsResponse>, ApiError> {
    let manager = state.plugin_manager.read().await;

    let plugin_info = manager
        .list_plugins()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(ApiError::NotFound(format!("Plugin not found: {}", id)))?;

    // TODO: Get actual metrics from plugin manager
    let details = PluginDetailsResponse {
        info: plugin_info,
        enabled: true,
        loaded_at: Some(chrono::Utc::now().to_rfc3339()),
        execution_count: 0,
        error_count: 0,
        avg_execution_time_ms: 0.0,
    };

    Ok(Json(details))
}

/// DELETE /api/v1/plugins/{id} - Unload a plugin
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{id}",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    responses(
        (status = 204, description = "Plugin unloaded successfully"),
        (status = 404, description = "Plugin not found", body = ErrorResponse),
        (status = 403, description = "Forbidden - Admin required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["admin"]),
        ("api_key" = ["admin"])
    )
)]
async fn unload_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _admin: RequireAdmin,
) -> Result<StatusCode, ApiError> {
    info!("Unloading plugin: {}", id);

    let manager = state.plugin_manager.write().await;

    manager
        .unload_plugin(&id)
        .map_err(|e| ApiError::InternalError(format!("Failed to unload plugin: {}", e)))?;

    info!("Plugin unloaded successfully: {}", id);

    state.audit_logger.log(
        "admin".to_string(),
        crate::types::UserRole::Admin,
        "unload_plugin".to_string(),
        format!("/api/v1/plugins/{}", id),
        serde_json::json!({"plugin_id": id}),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(StatusCode::NO_CONTENT)
}

/// PUT /api/v1/plugins/{id}/config - Update plugin configuration
#[utoipa::path(
    put,
    path = "/api/v1/plugins/{id}/config",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    request_body = UpdatePluginConfigRequest,
    responses(
        (status = 200, description = "Configuration updated successfully"),
        (status = 404, description = "Plugin not found", body = ErrorResponse),
        (status = 403, description = "Forbidden - Operator required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["operator"]),
        ("api_key" = ["operator"])
    )
)]
async fn update_plugin_config(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _operator: RequireOperator,
    Json(req): Json<UpdatePluginConfigRequest>,
) -> Result<StatusCode, ApiError> {
    info!("Updating config for plugin: {}", id);

    // TODO: Implement config update in plugin manager
    // For now, just validate the plugin exists
    let manager = state.plugin_manager.read().await;
    let _ = manager
        .list_plugins()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(ApiError::NotFound(format!("Plugin not found: {}", id)))?;

    state.audit_logger.log(
        "operator".to_string(),
        crate::types::UserRole::Operator,
        "update_plugin_config".to_string(),
        format!("/api/v1/plugins/{}/config", id),
        serde_json::json!({"plugin_id": id, "config": req.config}),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(StatusCode::OK)
}

/// POST /api/v1/plugins/{id}/toggle - Enable/disable a plugin
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/toggle",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    request_body = TogglePluginRequest,
    responses(
        (status = 200, description = "Plugin toggled successfully"),
        (status = 404, description = "Plugin not found", body = ErrorResponse),
        (status = 403, description = "Forbidden - Operator required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["operator"]),
        ("api_key" = ["operator"])
    )
)]
async fn toggle_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _operator: RequireOperator,
    Json(req): Json<TogglePluginRequest>,
) -> Result<StatusCode, ApiError> {
    info!("Toggling plugin {} to enabled={}", id, req.enabled);

    // TODO: Implement toggle in plugin manager

    state.audit_logger.log(
        "operator".to_string(),
        crate::types::UserRole::Operator,
        "toggle_plugin".to_string(),
        format!("/api/v1/plugins/{}/toggle", id),
        serde_json::json!({"plugin_id": id, "enabled": req.enabled}),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(StatusCode::OK)
}

/// GET /api/v1/plugins/{id}/metrics - Get plugin metrics
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{id}/metrics",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    responses(
        (status = 200, description = "Plugin metrics retrieved", body = PluginMetricsResponse),
        (status = 404, description = "Plugin not found", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_plugin_metrics(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _user: AuthUser,
) -> Result<Json<PluginMetricsResponse>, ApiError> {
    let manager = state.plugin_manager.read().await;

    let _ = manager
        .list_plugins()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(ApiError::NotFound(format!("Plugin not found: {}", id)))?;

    // TODO: Get actual metrics from plugin manager
    let metrics = PluginMetricsResponse {
        plugin_id: id,
        execution_count: 0,
        error_count: 0,
        timeout_count: 0,
        avg_execution_time_ms: 0.0,
        p50_execution_time_ms: 0.0,
        p95_execution_time_ms: 0.0,
        p99_execution_time_ms: 0.0,
        last_execution: None,
        last_error: None,
    };

    Ok(Json(metrics))
}

/// GET /api/v1/plugins/{id}/health - Get plugin health status
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{id}/health",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    responses(
        (status = 200, description = "Plugin health status", body = PluginHealthResponse),
        (status = 404, description = "Plugin not found", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = []),
        ("api_key" = [])
    )
)]
async fn get_plugin_health(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _user: AuthUser,
) -> Result<Json<PluginHealthResponse>, ApiError> {
    let manager = state.plugin_manager.read().await;

    let _ = manager
        .list_plugins()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(ApiError::NotFound(format!("Plugin not found: {}", id)))?;

    // TODO: Implement health check in plugin manager
    let health = PluginHealthResponse {
        plugin_id: id,
        status: PluginStatus::Healthy,
        health_check_time: chrono::Utc::now().to_rfc3339(),
        uptime_seconds: 0,
        issues: vec![],
    };

    Ok(Json(health))
}

/// POST /api/v1/plugins/{id}/reload - Reload a plugin
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/reload",
    params(
        ("id" = String, Path, description = "Plugin ID")
    ),
    responses(
        (status = 200, description = "Plugin reloaded successfully"),
        (status = 404, description = "Plugin not found", body = ErrorResponse),
        (status = 403, description = "Forbidden - Admin required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["admin"]),
        ("api_key" = ["admin"])
    )
)]
async fn reload_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<String>,
    _admin: RequireAdmin,
) -> Result<StatusCode, ApiError> {
    info!("Reloading plugin: {}", id);

    // TODO: Implement reload (unload + load)

    state.audit_logger.log(
        "admin".to_string(),
        crate::types::UserRole::Admin,
        "reload_plugin".to_string(),
        format!("/api/v1/plugins/{}/reload", id),
        serde_json::json!({"plugin_id": id}),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(StatusCode::OK)
}

/// POST /api/v1/plugins/reload-all - Reload all plugins
#[utoipa::path(
    post,
    path = "/api/v1/plugins/reload-all",
    responses(
        (status = 200, description = "All plugins reloaded successfully"),
        (status = 403, description = "Forbidden - Admin required", body = ErrorResponse)
    ),
    security(
        ("bearer_auth" = ["admin"]),
        ("api_key" = ["admin"])
    )
)]
async fn reload_all_plugins(
    State(state): State<PluginApiState>,
    _admin: RequireAdmin,
) -> Result<StatusCode, ApiError> {
    info!("Reloading all plugins");

    let manager = state.plugin_manager.read().await;
    let result = manager
        .load_all_plugins()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to reload plugins: {}", e)))?;

    info!("Reloaded {} plugins", result);

    state.audit_logger.log(
        "admin".to_string(),
        crate::types::UserRole::Admin,
        "reload_all_plugins".to_string(),
        "/api/v1/plugins/reload-all".to_string(),
        serde_json::json!({"count": result}),
        "0.0.0.0".to_string(),
        true,
    );

    Ok(StatusCode::OK)
}
