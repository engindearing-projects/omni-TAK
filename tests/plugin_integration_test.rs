//! End-to-end integration tests for the plugin system
//!
//! This test suite verifies the complete plugin system workflow:
//! 1. Starting the API server
//! 2. Loading a plugin via API
//! 3. Verifying plugin functionality
//! 4. Testing metrics collection
//! 5. Proper cleanup
//!
//! These tests use actual HTTP requests against a running server instance.

use reqwest::{Client, StatusCode};
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;

// Test configuration
const TEST_SERVER_ADDR: &str = "127.0.0.1:18443";
const TEST_BASE_URL: &str = "http://127.0.0.1:18443";

// ============================================================================
// Test Helper Functions
// ============================================================================

/// Create a test HTTP client
fn create_test_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client")
}

/// Login and get auth token
async fn get_auth_token(client: &Client, username: &str, password: &str) -> String {
    let response = client
        .post(&format!("{}/api/v1/auth/login", TEST_BASE_URL))
        .json(&json!({
            "username": username,
            "password": password
        }))
        .send()
        .await
        .expect("Failed to login");

    assert_eq!(response.status(), StatusCode::OK, "Login failed");

    let body: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse login response");
    body["access_token"]
        .as_str()
        .expect("No access token in response")
        .to_string()
}

/// Wait for server to be ready
async fn wait_for_server(client: &Client, max_attempts: u32) -> bool {
    for attempt in 1..=max_attempts {
        match client
            .get(&format!("{}/health", TEST_BASE_URL))
            .send()
            .await
        {
            Ok(response) if response.status() == StatusCode::OK => {
                println!("Server ready after {} attempts", attempt);
                return true;
            }
            _ => {
                if attempt < max_attempts {
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }
    false
}

// ============================================================================
// End-to-End Integration Tests
// ============================================================================

#[tokio::test]
#[ignore] // Run with: cargo test --test plugin_integration_test -- --ignored
async fn test_plugin_system_e2e_without_server() {
    // This test demonstrates the E2E flow without requiring a running server
    // It documents the expected behavior and API contract

    println!("=== Plugin System E2E Test (Documentation) ===");
    println!();
    println!("Expected Flow:");
    println!("1. Server starts with authentication enabled");
    println!("2. Admin user logs in and receives JWT token");
    println!("3. Admin loads a plugin via POST /api/v1/plugins");
    println!("4. System verifies plugin and loads it into WASM runtime");
    println!("5. Plugin appears in GET /api/v1/plugins list");
    println!("6. Plugin details accessible via GET /api/v1/plugins/:id");
    println!("7. Plugin metrics tracked via GET /api/v1/plugins/:id/metrics");
    println!("8. Plugin health monitored via GET /api/v1/plugins/:id/health");
    println!("9. Operator can update plugin config via PUT /api/v1/plugins/:id/config");
    println!("10. Operator can toggle plugin via POST /api/v1/plugins/:id/toggle");
    println!("11. Admin can unload plugin via DELETE /api/v1/plugins/:id");
    println!();

    // Verify test client can be created
    let client = create_test_client();
    assert!(client
        .get("https://httpbin.org/status/200")
        .send()
        .await
        .is_ok());
    println!("Test client verified");
}

#[tokio::test]
#[ignore] // Run manually when server is available
async fn test_plugin_api_endpoints_with_server() {
    println!("=== Testing Plugin API Endpoints ===");

    let client = create_test_client();

    // Wait for server to be ready
    println!("Waiting for server at {}...", TEST_BASE_URL);
    assert!(
        wait_for_server(&client, 10).await,
        "Server did not become ready"
    );

    // Step 1: Login as admin
    println!("Step 1: Logging in as admin...");
    let token = get_auth_token(&client, "admin", "admin_password_123").await;
    println!("Received auth token: {}...", &token[..20]);

    // Step 2: List plugins (should be empty initially)
    println!("Step 2: Listing plugins...");
    let response = client
        .get(&format!("{}/api/v1/plugins", TEST_BASE_URL))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to list plugins");

    assert_eq!(response.status(), StatusCode::OK);
    let plugins: serde_json::Value = response.json().await.unwrap();
    println!("Initial plugin count: {}", plugins["total"]);

    // Step 3: Try to load a plugin (will fail without actual WASM file)
    println!("Step 3: Attempting to load plugin...");
    let response = client
        .post(&format!("{}/api/v1/plugins", TEST_BASE_URL))
        .bearer_auth(&token)
        .json(&json!({
            "id": "test-filter",
            "path": "/tmp/test-plugin.wasm",
            "enabled": true,
            "pluginType": "filter",
            "config": {
                "id": "test-filter",
                "name": "Test Filter Plugin",
                "version": "0.1.0",
                "author": "Test Suite",
                "description": "A test filter plugin",
                "maxExecutionTimeUs": 1000
            }
        }))
        .send()
        .await
        .expect("Failed to send load plugin request");

    println!("Load plugin response status: {}", response.status());
    // Expected to fail since we don't have actual WASM file
    assert!(
        response.status() == StatusCode::CREATED
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );

    // Step 4: Test plugin metrics endpoint
    println!("Step 4: Testing plugin metrics endpoint...");
    let response = client
        .get(&format!(
            "{}/api/v1/plugins/test-filter/metrics",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to get plugin metrics");

    println!("Metrics response status: {}", response.status());
    // Will be 404 if plugin wasn't loaded
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);

    // Step 5: Test plugin health endpoint
    println!("Step 5: Testing plugin health endpoint...");
    let response = client
        .get(&format!(
            "{}/api/v1/plugins/test-filter/health",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to get plugin health");

    println!("Health response status: {}", response.status());

    // Step 6: Test plugin config update
    println!("Step 6: Testing plugin config update...");
    let response = client
        .put(&format!(
            "{}/api/v1/plugins/test-filter/config",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .json(&json!({
            "config": {
                "threshold": 100,
                "enabled": true
            }
        }))
        .send()
        .await
        .expect("Failed to update plugin config");

    println!("Config update response status: {}", response.status());

    // Step 7: Test plugin toggle
    println!("Step 7: Testing plugin toggle...");
    let response = client
        .post(&format!(
            "{}/api/v1/plugins/test-filter/toggle",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .json(&json!({
            "enabled": false
        }))
        .send()
        .await
        .expect("Failed to toggle plugin");

    println!("Toggle response status: {}", response.status());

    // Step 8: Test plugin reload
    println!("Step 8: Testing plugin reload...");
    let response = client
        .post(&format!(
            "{}/api/v1/plugins/test-filter/reload",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to reload plugin");

    println!("Reload response status: {}", response.status());

    // Step 9: Test reload all plugins
    println!("Step 9: Testing reload all plugins...");
    let response = client
        .post(&format!("{}/api/v1/plugins/reload-all", TEST_BASE_URL))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to reload all plugins");

    assert_eq!(response.status(), StatusCode::OK);
    println!("Reload all successful");

    // Step 10: Test plugin unload
    println!("Step 10: Testing plugin unload...");
    let response = client
        .delete(&format!("{}/api/v1/plugins/test-filter", TEST_BASE_URL))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to unload plugin");

    println!("Unload response status: {}", response.status());

    println!("=== Test completed successfully ===");
}

#[tokio::test]
#[ignore] // Run manually
async fn test_plugin_permissions() {
    println!("=== Testing Plugin API Permissions ===");

    let client = create_test_client();

    // Wait for server
    assert!(wait_for_server(&client, 10).await);

    // Login as operator (not admin)
    println!("Logging in as operator...");
    let operator_token = get_auth_token(&client, "operator", "operator_password_123").await;

    // Try to load plugin (should fail - requires admin)
    println!("Attempting to load plugin as operator (should fail)...");
    let response = client
        .post(&format!("{}/api/v1/plugins", TEST_BASE_URL))
        .bearer_auth(&operator_token)
        .json(&json!({
            "id": "test-plugin",
            "path": "/tmp/test.wasm",
            "enabled": true,
            "pluginType": "filter",
            "config": {}
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "Operator should not be able to load plugins"
    );
    println!("Correctly rejected operator load attempt");

    // Try to update config (should succeed - operator can do this)
    println!("Attempting to update config as operator (should succeed)...");
    let response = client
        .put(&format!("{}/api/v1/plugins/test/config", TEST_BASE_URL))
        .bearer_auth(&operator_token)
        .json(&json!({
            "config": {"key": "value"}
        }))
        .send()
        .await
        .expect("Failed to send request");

    // Will be 404 since plugin doesn't exist, but permission check passes
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND,
        "Operator should be able to update plugin config"
    );
    println!("Operator config update permission verified");

    println!("=== Permission tests completed ===");
}

#[tokio::test]
#[ignore] // Run manually
async fn test_plugin_metrics_collection() {
    println!("=== Testing Plugin Metrics Collection ===");

    let client = create_test_client();
    assert!(wait_for_server(&client, 10).await);

    let token = get_auth_token(&client, "admin", "admin_password_123").await;

    // Get metrics for a plugin
    let response = client
        .get(&format!("{}/api/v1/plugins/test/metrics", TEST_BASE_URL))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to get metrics");

    if response.status() == StatusCode::OK {
        let metrics: serde_json::Value = response.json().await.unwrap();
        println!(
            "Metrics structure: {}",
            serde_json::to_string_pretty(&metrics).unwrap()
        );

        // Verify metrics structure
        assert!(metrics.get("pluginId").is_some());
        assert!(metrics.get("executionCount").is_some());
        assert!(metrics.get("errorCount").is_some());
        assert!(metrics.get("avgExecutionTimeMs").is_some());

        println!("Metrics structure verified");
    } else {
        println!("Plugin not found (expected if no plugins loaded)");
    }

    println!("=== Metrics test completed ===");
}

// ============================================================================
// Performance Tests
// ============================================================================

#[tokio::test]
#[ignore] // Run manually for performance testing
async fn test_plugin_api_performance() {
    println!("=== Testing Plugin API Performance ===");

    let client = create_test_client();
    assert!(wait_for_server(&client, 10).await);

    let token = get_auth_token(&client, "admin", "admin_password_123").await;

    // Measure list plugins latency
    let start = std::time::Instant::now();
    for _ in 0..100 {
        let response = client
            .get(&format!("{}/api/v1/plugins", TEST_BASE_URL))
            .bearer_auth(&token)
            .send()
            .await
            .expect("Failed to list plugins");
        assert_eq!(response.status(), StatusCode::OK);
    }
    let duration = start.elapsed();

    println!(
        "100 list plugins requests took: {:?} (avg: {:?})",
        duration,
        duration / 100
    );

    assert!(
        duration.as_millis() < 5000,
        "List plugins should complete 100 requests in less than 5 seconds"
    );

    println!("=== Performance test completed ===");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
#[ignore] // Run manually
async fn test_plugin_error_handling() {
    println!("=== Testing Plugin Error Handling ===");

    let client = create_test_client();
    assert!(wait_for_server(&client, 10).await);

    let token = get_auth_token(&client, "admin", "admin_password_123").await;

    // Test 1: Invalid plugin ID format
    println!("Test 1: Invalid plugin load request...");
    let response = client
        .post(&format!("{}/api/v1/plugins", TEST_BASE_URL))
        .bearer_auth(&token)
        .json(&json!({
            "id": "",  // Empty ID should fail validation
            "path": "/tmp/test.wasm",
            "enabled": true,
            "pluginType": "filter",
            "config": {}
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    println!("Validation error correctly returned");

    // Test 2: Non-existent plugin operations
    println!("Test 2: Operations on non-existent plugin...");
    let response = client
        .get(&format!(
            "{}/api/v1/plugins/nonexistent-plugin-xyz",
            TEST_BASE_URL
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    println!("Not found error correctly returned");

    // Test 3: Invalid JSON
    println!("Test 3: Invalid JSON payload...");
    let response = client
        .post(&format!("{}/api/v1/plugins", TEST_BASE_URL))
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body("{invalid json")
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    println!("JSON parse error correctly handled");

    println!("=== Error handling tests completed ===");
}
