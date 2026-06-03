//! Host implementation for OmniTAK plugins.
//!
//! This module provides the host-side implementation of the plugin interface,
//! exposing OmniTAK functionality to WASM plugins.

use super::bindings::omnitak::plugins::host;
use crate::types::ServerConfig;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};

/// Plugin host context that implements the host interface.
///
/// This struct holds the state needed by plugins to interact with the OmniTAK host,
/// including configuration, server information, and WASI context.
pub struct PluginHost {
    /// WASI context for standard WASI operations
    wasi: WasiCtx,
    /// Resource table for managing WASM resources
    resources: ResourceTable,
    /// Configuration key-value pairs accessible to plugins
    config: Arc<RwLock<HashMap<String, String>>>,
    /// Server configurations indexed by ID
    servers: Arc<RwLock<HashMap<String, ServerConfig>>>,
}

impl PluginHost {
    /// Create a new plugin host with the given configuration and server list.
    pub fn new(config: HashMap<String, String>, servers: Vec<ServerConfig>) -> Self {
        // Build WASI context with inherited stdout/stderr for plugin logging
        let wasi = WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();

        // Index servers by their name for quick lookup
        let servers_map: HashMap<String, ServerConfig> = servers
            .into_iter()
            .map(|server| (server.name.clone(), server))
            .collect();

        Self {
            wasi,
            resources: ResourceTable::new(),
            config: Arc::new(RwLock::new(config)),
            servers: Arc::new(RwLock::new(servers_map)),
        }
    }

    /// Update the plugin configuration.
    pub fn update_config(&self, config: HashMap<String, String>) {
        if let Ok(mut cfg) = self.config.write() {
            *cfg = config;
        }
    }

    /// Update the server list.
    pub fn update_servers(&self, servers: Vec<ServerConfig>) {
        if let Ok(mut srv) = self.servers.write() {
            srv.clear();
            for server in servers {
                srv.insert(server.name.clone(), server);
            }
        }
    }
}

// Implement WasiView for wasmtime_wasi integration
impl WasiView for PluginHost {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

// Implement the host interface for plugins
impl host::Host for PluginHost {
    /// Log a message from a plugin.
    fn log(&mut self, level: host::LogLevel, message: String) -> wasmtime::Result<()> {
        match level {
            host::LogLevel::Trace => tracing::trace!(target: "plugin", "{}", message),
            host::LogLevel::Debug => tracing::debug!(target: "plugin", "{}", message),
            host::LogLevel::Info => tracing::info!(target: "plugin", "{}", message),
            host::LogLevel::Warn => tracing::warn!(target: "plugin", "{}", message),
            host::LogLevel::Error => tracing::error!(target: "plugin", "{}", message),
        }
        Ok(())
    }

    /// Get a configuration value by key.
    fn get_config(&mut self, key: String) -> wasmtime::Result<Option<String>> {
        let config = self
            .config
            .read()
            .map_err(|e| wasmtime::Error::msg(format!("Failed to read config: {}", e)))?;
        Ok(config.get(&key).cloned())
    }

    /// Get server information by ID.
    fn get_server_info(&mut self, server_id: String) -> wasmtime::Result<Option<host::ServerInfo>> {
        let servers = self
            .servers
            .read()
            .map_err(|e| wasmtime::Error::msg(format!("Failed to read servers: {}", e)))?;

        Ok(servers.get(&server_id).map(|server| host::ServerInfo {
            id: server.name.clone(),
            name: server.name.clone(),
            host: server.host.clone(),
            port: server.port,
            protocol: format!("{:?}", server.protocol),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Protocol;

    #[test]
    fn test_host_creation() {
        let config = HashMap::new();
        let servers = vec![ServerConfig::builder()
            .name("test-server")
            .host("localhost")
            .port(8089)
            .protocol(Protocol::Tcp)
            .build()];

        let host = PluginHost::new(config, servers);
        assert!(host
            .servers
            .read()
            .unwrap()
            .contains_key(&host.servers.read().unwrap().keys().next().unwrap().clone()));
    }

    #[test]
    fn test_config_update() {
        let config = HashMap::new();
        let host = PluginHost::new(config, vec![]);

        let mut new_config = HashMap::new();
        new_config.insert("key1".to_string(), "value1".to_string());
        host.update_config(new_config);

        let config = host.config.read().unwrap();
        assert_eq!(config.get("key1"), Some(&"value1".to_string()));
    }
}
