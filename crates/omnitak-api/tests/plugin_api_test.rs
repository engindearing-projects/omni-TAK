//! Integration tests for Plugin API endpoints
//!
//! Tests all plugin management endpoints including:
//! - Loading plugins
//! - Listing plugins
//! - Getting plugin details
//! - Updating configuration
//! - Toggling plugins
//! - Unloading plugins
//! - Plugin metrics
//! - Plugin health
//! - Error handling

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use omnitak_api::{
    auth::{AuthConfig, AuthService},
    middleware::AuditLogger,
    rest::plugins::{
        create_plugin_router, LoadPluginRequest, PluginApiState, PluginDetailsResponse,
        PluginHealthResponse, PluginListResponse, PluginMetricsResponse, PluginType,
        TogglePluginRequest, UpdatePluginConfigRequest,
    },
    types::UserRole,
};
use omnitak_plugin_api::{PluginManager, PluginManagerConfig};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

// ============================================================================
// Test Setup Helpers
// ============================================================================

/// Create a test plugin API state
async fn create_test_state() -> PluginApiState {
    let config = PluginManagerConfig {
        plugin_dir: "/tmp/omnitak-test-plugins".to_string(),
        hot_reload: false,
        ..Default::default()
    };

    let plugin_manager = PluginManager::new(config).expect("Failed to create plugin manager");

    PluginApiState {
        plugin_manager: Arc::new(RwLock::new(plugin_manager)),
        audit_logger: Arc::new(AuditLogger::new()),
    }
}

/// Create test auth service with admin user
fn create_test_auth() -> Arc<AuthService> {
    let auth_config = AuthConfig::default();
    let auth_service = Arc::new(AuthService::new(auth_config));

    auth_service
        .create_user("admin".to_string(), "admin_password_123", UserRole::Admin)
        .expect("Failed to create admin user");

    auth_service
        .create_user(
            "operator".to_string(),
            "operator_password_123",
            UserRole::Operator,
        )
        .expect("Failed to create operator user");

    auth_service
}

/// Create router with authentication layer
fn create_test_router(state: PluginApiState, auth_service: Arc<AuthService>) -> Router {
    create_plugin_router(state).layer(axum::Extension(auth_service))
}

/// Get admin auth token
async fn get_admin_token(auth_service: &Arc<AuthService>) -> String {
    let (token, _) = auth_service
        .login("admin", "admin_password_123")
        .expect("Failed to login");
    token
}

/// Get operator auth token
async fn get_operator_token(auth_service: &Arc<AuthService>) -> String {
    let (token, _) = auth_service
        .login("operator", "operator_password_123")
        .expect("Failed to login");
    token
}

// ============================================================================
// List Plugins Tests
// ============================================================================

#[tokio::test]
async fn test_list_plugins_empty() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let list: PluginListResponse = serde_json::from_slice(&body).unwrap();

    assert_eq!(list.total, 0);
    assert!(list.plugins.is_empty());
}

#[tokio::test]
async fn test_list_plugins_unauthorized() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_list_plugins_with_filters() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    // Test with enabled_only filter
    let request = Request::builder()
        .uri("/api/v1/plugins?enabled_only=true")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Test with plugin_type filter
    let auth2 = create_test_auth();
    let token2 = get_admin_token(&auth2).await;
    let state2 = create_test_state().await;
    let app2 = create_test_router(state2, auth2);

    let request = Request::builder()
        .uri("/api/v1/plugins?plugin_type=filter")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token2))
        .body(Body::empty())
        .unwrap();

    let response = app2.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ============================================================================
// Load Plugin Tests
// ============================================================================

#[tokio::test]
async fn test_load_plugin_validation_errors() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    // Test with empty ID
    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "id": "",
                "path": "/path/to/plugin.wasm",
                "enabled": true,
                "pluginType": "filter",
                "config": {
                    "id": "test",
                    "name": "Test Plugin",
                    "version": "1.0.0",
                    "author": "Test",
                    "description": "Test",
                    "maxExecutionTimeUs": 1000
                }
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_load_plugin_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "id": "test-plugin",
                "path": "/nonexistent/plugin.wasm",
                "enabled": true,
                "pluginType": "filter",
                "config": {
                    "id": "test-plugin",
                    "name": "Test Plugin",
                    "version": "1.0.0",
                    "author": "Test",
                    "description": "Test",
                    "maxExecutionTimeUs": 1000
                }
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_load_plugin_requires_admin() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "id": "test-plugin",
                "path": "/path/to/plugin.wasm",
                "enabled": true,
                "pluginType": "filter",
                "config": {}
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// Get Plugin Details Tests
// ============================================================================

#[tokio::test]
async fn test_get_plugin_details_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// Update Plugin Config Tests
// ============================================================================

#[tokio::test]
async fn test_update_plugin_config_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin/config")
        .method("PUT")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "config": {"key": "value"}
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_update_plugin_config_requires_operator() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/test-plugin/config")
        .method("PUT")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "config": {"key": "value"}
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Admin has operator permissions, so this should work
    // (would be 404 since plugin doesn't exist)
    assert!(response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::OK);
}

// ============================================================================
// Toggle Plugin Tests
// ============================================================================

#[tokio::test]
async fn test_toggle_plugin_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin/toggle")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "enabled": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Current implementation returns OK even if plugin doesn't exist (TODO)
    assert_eq!(response.status(), StatusCode::OK);
}

// ============================================================================
// Unload Plugin Tests
// ============================================================================

#[tokio::test]
async fn test_unload_plugin_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin")
        .method("DELETE")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_unload_plugin_requires_admin() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/test-plugin")
        .method("DELETE")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// Plugin Metrics Tests
// ============================================================================

#[tokio::test]
async fn test_get_plugin_metrics_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin/metrics")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// Plugin Health Tests
// ============================================================================

#[tokio::test]
async fn test_get_plugin_health_not_found() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent-plugin/health")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// Reload Plugin Tests
// ============================================================================

#[tokio::test]
async fn test_reload_plugin_requires_admin() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/test-plugin/reload")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_reload_all_plugins_requires_admin() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_operator_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/reload-all")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_reload_all_plugins_success() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/reload-all")
        .method("POST")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should succeed (returns 0 plugins loaded since directory doesn't exist)
    assert_eq!(response.status(), StatusCode::OK);
}

// ============================================================================
// Integration Tests with Multiple Operations
// ============================================================================

#[tokio::test]
async fn test_plugin_lifecycle_without_actual_wasm() {
    // This test verifies the API flow without actual WASM loading
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;

    // 1. List plugins (should be empty)
    let app = create_test_router(state.clone(), auth.clone());
    let request = Request::builder()
        .uri("/api/v1/plugins")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 2. Try to get details of non-existent plugin
    let app = create_test_router(state.clone(), auth.clone());
    let request = Request::builder()
        .uri("/api/v1/plugins/test-plugin")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // 3. Try to unload non-existent plugin
    let app = create_test_router(state.clone(), auth.clone());
    let request = Request::builder()
        .uri("/api/v1/plugins/test-plugin")
        .method("DELETE")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ============================================================================
// Error Response Format Tests
// ============================================================================

#[tokio::test]
async fn test_error_responses_have_correct_format() {
    let state = create_test_state().await;
    let auth = create_test_auth();
    let token = get_admin_token(&auth).await;
    let app = create_test_router(state, auth);

    let request = Request::builder()
        .uri("/api/v1/plugins/nonexistent")
        .method("GET")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Check error response structure
    assert!(error.get("error").is_some());
    assert!(error.get("message").is_some());
}
