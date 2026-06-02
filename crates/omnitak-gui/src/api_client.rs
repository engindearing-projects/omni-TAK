//! REST API client for OmniTAK GUI
//!
//! Handles all communication with the main OmniTAK server via REST API

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    client: Client,
    auth_token: Option<String>,
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Serialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_at: String,
    #[allow(dead_code)]
    role: String,
}

#[derive(Debug, Deserialize)]
pub struct SystemStatus {
    pub uptime_seconds: u64,
    pub active_connections: usize,
    pub messages_processed: u64,
    pub messages_per_second: f64,
    pub memory_usage_bytes: u64,
    pub active_filters: usize,
    pub version: String,
}

#[derive(Debug, Deserialize)]
struct ConnectionListResponse {
    connections: Vec<ConnectionInfo>,
    #[allow(dead_code)]
    total: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionType {
    TcpClient,
    TcpServer,
    TlsClient,
    TlsServer,
    Multicast,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub connection_type: ConnectionType,
    pub status: String,
    pub address: String,
    pub port: u16,
    pub messages_received: u64,
    pub messages_sent: u64,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionRequest {
    pub name: String,
    pub connection_type: ConnectionType,
    pub address: String,
    pub port: u16,
    pub priority: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct CreateConnectionResponse {
    id: String,
    #[allow(dead_code)]
    message: String,
}

// ============================================================================
// Plugin API Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct PluginListResponse {
    pub plugins: Vec<omnitak_plugin_api::PluginInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginRequest {
    pub id: String,
    pub path: String,
    pub enabled: bool,
    pub plugin_type: PluginApiType,
    pub config: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginApiType {
    Filter,
    Transformer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDetailsResponse {
    pub info: omnitak_plugin_api::PluginInfo,
    pub enabled: bool,
    pub loaded_at: Option<String>,
    pub execution_count: u64,
    pub error_count: u64,
    pub avg_execution_time_ms: f64,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthResponse {
    pub plugin_id: String,
    pub status: String,
    pub health_check_time: String,
    pub uptime_seconds: u64,
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePluginConfigRequest {
    pub config: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TogglePluginRequest {
    pub enabled: bool,
}

// ============================================================================
// API Client Implementation
// ============================================================================

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            base_url: base_url.into(),
            client,
            auth_token: None,
        })
    }

    /// Login and get authentication token
    pub async fn login(&mut self, username: &str, password: &str) -> Result<()> {
        let url = format!("{}/api/v1/auth/login", self.base_url);

        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send login request")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Login failed ({}): {}", status, error_text);
        }

        let login_response: LoginResponse = response
            .json()
            .await
            .context("Failed to parse login response")?;

        self.auth_token = Some(login_response.access_token);

        Ok(())
    }

    /// Check if authenticated
    pub fn is_authenticated(&self) -> bool {
        self.auth_token.is_some()
    }

    /// Get system status
    pub async fn get_status(&self) -> Result<SystemStatus> {
        let url = format!("{}/api/v1/status", self.base_url);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to get system status")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Status request failed ({}): {}", status, error_text);
        }

        let status = response
            .json()
            .await
            .context("Failed to parse status response")?;

        Ok(status)
    }

    /// List all connections
    pub async fn list_connections(&self) -> Result<Vec<ConnectionInfo>> {
        let url = format!("{}/api/v1/connections", self.base_url);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Failed to list connections")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("List connections failed ({}): {}", status, error_text);
        }

        let list_response: ConnectionListResponse = response
            .json()
            .await
            .context("Failed to parse connections response")?;

        Ok(list_response.connections)
    }

    /// Create a new connection
    pub async fn create_connection(&self, request: CreateConnectionRequest) -> Result<String> {
        let url = format!("{}/api/v1/connections", self.base_url);

        let mut req = self.client.post(&url).json(&request);

        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("Failed to create connection")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Create connection failed ({}): {}", status, error_text);
        }

        let create_response: CreateConnectionResponse = response
            .json()
            .await
            .context("Failed to parse create connection response")?;

        Ok(create_response.id)
    }

    /// Delete a connection
    pub async fn delete_connection(&self, id: &str) -> Result<()> {
        let url = format!("{}/api/v1/connections/{}", self.base_url, id);

        let mut request = self.client.delete(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to delete connection")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Delete connection failed ({}): {}", status, error_text);
        }

        Ok(())
    }

    /// Health check (no auth required)
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/v1/health", self.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to check health")?;

        Ok(response.status().is_success())
    }

    // ============================================================================
    // Plugin API Methods
    // ============================================================================

    /// List all plugins
    pub async fn list_plugins(&self) -> Result<Vec<omnitak_plugin_api::PluginInfo>> {
        let url = format!("{}/api/v1/plugins", self.base_url);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Failed to list plugins")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("List plugins failed ({}): {}", status, error_text);
        }

        let list_response: PluginListResponse = response
            .json()
            .await
            .context("Failed to parse plugins response")?;

        Ok(list_response.plugins)
    }

    /// Load a new plugin
    pub async fn load_plugin(
        &self,
        request: LoadPluginRequest,
    ) -> Result<omnitak_plugin_api::PluginInfo> {
        let url = format!("{}/api/v1/plugins", self.base_url);

        let mut req = self.client.post(&url).json(&request);

        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("Failed to load plugin")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Load plugin failed ({}): {}", status, error_text);
        }

        let plugin_info = response
            .json()
            .await
            .context("Failed to parse load plugin response")?;

        Ok(plugin_info)
    }

    /// Get plugin details
    pub async fn get_plugin_details(&self, id: &str) -> Result<PluginDetailsResponse> {
        let url = format!("{}/api/v1/plugins/{}", self.base_url, id);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to get plugin details")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Get plugin details failed ({}): {}", status, error_text);
        }

        let details = response
            .json()
            .await
            .context("Failed to parse plugin details response")?;

        Ok(details)
    }

    /// Unload a plugin
    pub async fn unload_plugin(&self, id: &str) -> Result<()> {
        let url = format!("{}/api/v1/plugins/{}", self.base_url, id);

        let mut request = self.client.delete(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Failed to unload plugin")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Unload plugin failed ({}): {}", status, error_text);
        }

        Ok(())
    }

    /// Update plugin configuration
    pub async fn update_plugin_config(&self, id: &str, config: serde_json::Value) -> Result<()> {
        let url = format!("{}/api/v1/plugins/{}/config", self.base_url, id);

        let request_body = UpdatePluginConfigRequest { config };

        let mut req = self.client.put(&url).json(&request_body);

        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("Failed to update plugin config")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Update plugin config failed ({}): {}", status, error_text);
        }

        Ok(())
    }

    /// Toggle plugin enabled/disabled
    pub async fn toggle_plugin(&self, id: &str, enabled: bool) -> Result<()> {
        let url = format!("{}/api/v1/plugins/{}/toggle", self.base_url, id);

        let request_body = TogglePluginRequest { enabled };

        let mut req = self.client.post(&url).json(&request_body);

        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("Failed to toggle plugin")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Toggle plugin failed ({}): {}", status, error_text);
        }

        Ok(())
    }

    /// Get plugin metrics
    pub async fn get_plugin_metrics(&self, id: &str) -> Result<PluginMetricsResponse> {
        let url = format!("{}/api/v1/plugins/{}/metrics", self.base_url, id);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to get plugin metrics")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Get plugin metrics failed ({}): {}", status, error_text);
        }

        let metrics = response
            .json()
            .await
            .context("Failed to parse plugin metrics response")?;

        Ok(metrics)
    }

    /// Get plugin health
    pub async fn get_plugin_health(&self, id: &str) -> Result<PluginHealthResponse> {
        let url = format!("{}/api/v1/plugins/{}/health", self.base_url, id);

        let mut request = self.client.get(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to get plugin health")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Get plugin health failed ({}): {}", status, error_text);
        }

        let health = response
            .json()
            .await
            .context("Failed to parse plugin health response")?;

        Ok(health)
    }

    /// Reload a plugin
    pub async fn reload_plugin(&self, id: &str) -> Result<()> {
        let url = format!("{}/api/v1/plugins/{}/reload", self.base_url, id);

        let mut request = self.client.post(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Failed to reload plugin")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Reload plugin failed ({}): {}", status, error_text);
        }

        Ok(())
    }

    /// Reload all plugins
    pub async fn reload_all_plugins(&self) -> Result<()> {
        let url = format!("{}/api/v1/plugins/reload-all", self.base_url);

        let mut request = self.client.post(&url);

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to reload all plugins")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Reload all plugins failed ({}): {}", status, error_text);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let client = ApiClient::new("http://localhost:9443");
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_health_check() {
        let client = ApiClient::new("http://localhost:9443").unwrap();
        // This will fail if server is not running, which is fine for unit tests
        let _ = client.health_check().await;
    }
}
