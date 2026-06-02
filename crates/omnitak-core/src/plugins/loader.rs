//! Plugin loader and manager for OmniTAK.
//!
//! This module provides the infrastructure for loading, managing, and executing
//! WASM-based message filter plugins.

use super::{bindings, host::PluginHost};
use crate::{error::OmniTAKError, types::ServerConfig, Result};
use anyhow::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use wasmtime::component::{Component, HasSelf, Linker};

/// A loaded message filter plugin instance.
pub struct MessageFilterPlugin {
    /// Wasmtime store containing the plugin's state
    store: wasmtime::Store<PluginHost>,
    /// The plugin instance
    plugin: bindings::MessageFilterPlugin,
    /// Plugin metadata
    info: PluginInfo,
    /// Path to the plugin WASM file
    path: PathBuf,
}

/// Plugin metadata.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
}

impl MessageFilterPlugin {
    /// Load a plugin from a WASM file.
    ///
    /// # Arguments
    /// * `path` - Path to the WASM plugin file
    /// * `config` - Configuration key-value pairs for the plugin
    /// * `servers` - List of server configurations
    ///
    /// # Returns
    /// A loaded plugin instance ready to filter messages
    pub fn load<P: AsRef<Path>>(
        path: P,
        config: HashMap<String, String>,
        servers: Vec<ServerConfig>,
    ) -> Result<Self> {
        let path = path.as_ref();

        // Create Wasmtime engine with default configuration
        let engine = wasmtime::Engine::default();

        // Create host context
        let host = PluginHost::new(config.clone(), servers);
        let mut store = wasmtime::Store::new(&engine, host);

        // Load the component from the WASM file
        let component = Component::from_file(&engine, path)
            .context("Failed to load WASM component")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        // Create and configure linker
        let mut linker = Linker::new(&engine);

        // Add WASI support (using WASI preview 2)
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .context("Failed to add WASI bindings")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        // Add our custom bindings
        bindings::MessageFilterPlugin::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state: &mut PluginHost| state,
        )
        .context("Failed to add plugin bindings")
        .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        // Instantiate the plugin
        let plugin = bindings::MessageFilterPlugin::instantiate(&mut store, &component, &linker)
            .context("Failed to instantiate plugin")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        // Get plugin info
        let info_result = plugin
            .omnitak_plugins_message_filter()
            .call_get_info(&mut store)
            .context("Failed to get plugin info")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        let info = PluginInfo {
            name: info_result.name,
            version: info_result.version,
            description: info_result.description,
            author: info_result.author,
        };

        // Initialize the plugin
        let init_config: Vec<(String, String)> = config.into_iter().collect();
        plugin
            .omnitak_plugins_message_filter()
            .call_initialize(&mut store, &init_config)
            .context("Failed to call plugin initialize")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?
            .map_err(|e| {
                OmniTAKError::PluginError(format!("Plugin initialization failed: {}", e))
            })?;

        tracing::info!(
            "Loaded plugin: {} v{} from {}",
            info.name,
            info.version,
            path.display()
        );

        Ok(Self {
            store,
            plugin,
            info,
            path: path.to_path_buf(),
        })
    }

    /// Filter a message through this plugin.
    ///
    /// # Arguments
    /// * `xml` - CoT message XML
    /// * `metadata` - Message metadata
    ///
    /// # Returns
    /// `FilterResult` indicating what action to take
    pub fn filter_message(&mut self, xml: &str, metadata: MessageMetadata) -> Result<FilterResult> {
        use bindings::exports::omnitak::plugins::message_filter;

        let wasm_metadata = message_filter::MessageMetadata {
            server_id: metadata.server_id,
            server_name: metadata.server_name,
            received_at: metadata.received_at,
            source_addr: metadata.source_addr,
            protocol: metadata.protocol,
        };

        let result = self
            .plugin
            .omnitak_plugins_message_filter()
            .call_filter_message(&mut self.store, xml, &wasm_metadata)
            .context("Failed to call filter_message")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?
            .map_err(|e| OmniTAKError::PluginError(format!("Plugin filter error: {}", e)))?;

        Ok(FilterResult {
            action: match result.action {
                message_filter::FilterAction::Pass => FilterAction::Pass,
                message_filter::FilterAction::Modify => FilterAction::Modify,
                message_filter::FilterAction::Drop => FilterAction::Drop,
            },
            modified_xml: result.modified_xml,
            reason: result.reason,
        })
    }

    /// Reload this plugin from disk.
    pub fn reload(
        &mut self,
        config: HashMap<String, String>,
        servers: Vec<ServerConfig>,
    ) -> Result<()> {
        // Shutdown the current instance
        let _ = self
            .plugin
            .omnitak_plugins_message_filter()
            .call_shutdown(&mut self.store);

        // Load a new instance
        let new_plugin = Self::load(&self.path, config, servers)?;

        // Replace our state with the new plugin
        self.store = new_plugin.store;
        self.plugin = new_plugin.plugin;
        self.info = new_plugin.info;

        tracing::info!("Reloaded plugin: {}", self.info.name);
        Ok(())
    }

    /// Get plugin information.
    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// Shutdown the plugin.
    pub fn shutdown(&mut self) -> Result<()> {
        self.plugin
            .omnitak_plugins_message_filter()
            .call_shutdown(&mut self.store)
            .context("Failed to call shutdown")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?
            .map_err(|e| OmniTAKError::PluginError(format!("Plugin shutdown failed: {}", e)))?;

        tracing::info!("Shutdown plugin: {}", self.info.name);
        Ok(())
    }
}

/// Metadata passed to plugins when filtering messages.
#[derive(Debug, Clone)]
pub struct MessageMetadata {
    pub server_id: String,
    pub server_name: String,
    pub received_at: u64,
    pub source_addr: Option<String>,
    pub protocol: String,
}

/// Result of a message filter operation.
#[derive(Debug, Clone)]
pub struct FilterResult {
    pub action: FilterAction,
    pub modified_xml: Option<String>,
    pub reason: Option<String>,
}

/// Action to take with a filtered message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterAction {
    /// Pass the message through unchanged
    Pass,
    /// Pass the message with modifications
    Modify,
    /// Drop the message
    Drop,
}

/// Manager for multiple message filter plugins.
pub struct PluginManager {
    /// Loaded plugins indexed by name
    plugins: HashMap<String, MessageFilterPlugin>,
    /// Directory to load plugins from
    plugin_dir: PathBuf,
    /// Global configuration for plugins
    config: HashMap<String, String>,
    /// Server configurations
    servers: Vec<ServerConfig>,
}

impl PluginManager {
    /// Create a new plugin manager.
    ///
    /// # Arguments
    /// * `plugin_dir` - Directory containing plugin WASM files
    /// * `config` - Global configuration for plugins
    /// * `servers` - List of server configurations
    pub fn new<P: AsRef<Path>>(
        plugin_dir: P,
        config: HashMap<String, String>,
        servers: Vec<ServerConfig>,
    ) -> Self {
        Self {
            plugins: HashMap::new(),
            plugin_dir: plugin_dir.as_ref().to_path_buf(),
            config,
            servers,
        }
    }

    /// Load all plugins from the plugin directory.
    pub fn load_all(&mut self) -> Result<usize> {
        let mut loaded = 0;

        if !self.plugin_dir.exists() {
            tracing::warn!(
                "Plugin directory does not exist: {}",
                self.plugin_dir.display()
            );
            return Ok(0);
        }

        let entries = std::fs::read_dir(&self.plugin_dir)
            .context("Failed to read plugin directory")
            .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

        for entry in entries {
            let entry = entry
                .context("Failed to read directory entry")
                .map_err(|e| OmniTAKError::PluginError(e.to_string()))?;

            let path = entry.path();

            // Only load .wasm files
            if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
                continue;
            }

            match self.load_plugin(&path) {
                Ok(name) => {
                    tracing::info!("Loaded plugin: {}", name);
                    loaded += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to load plugin {}: {}", path.display(), e);
                }
            }
        }

        Ok(loaded)
    }

    /// Load a specific plugin.
    pub fn load_plugin<P: AsRef<Path>>(&mut self, path: P) -> Result<String> {
        let plugin = MessageFilterPlugin::load(path, self.config.clone(), self.servers.clone())?;
        let name = plugin.info().name.clone();
        self.plugins.insert(name.clone(), plugin);
        Ok(name)
    }

    /// Unload a specific plugin.
    pub fn unload_plugin(&mut self, name: &str) -> Result<()> {
        if let Some(mut plugin) = self.plugins.remove(name) {
            plugin.shutdown()?;
            tracing::info!("Unloaded plugin: {}", name);
            Ok(())
        } else {
            Err(OmniTAKError::PluginError(format!(
                "Plugin not found: {}",
                name
            )))
        }
    }

    /// Reload a specific plugin.
    pub fn reload_plugin(&mut self, name: &str) -> Result<()> {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.reload(self.config.clone(), self.servers.clone())?;
            Ok(())
        } else {
            Err(OmniTAKError::PluginError(format!(
                "Plugin not found: {}",
                name
            )))
        }
    }

    /// Reload all plugins.
    pub fn reload_all(&mut self) -> Result<usize> {
        let plugin_names: Vec<String> = self.plugins.keys().cloned().collect();
        let mut reloaded = 0;

        for name in plugin_names {
            match self.reload_plugin(&name) {
                Ok(()) => reloaded += 1,
                Err(e) => tracing::error!("Failed to reload plugin {}: {}", name, e),
            }
        }

        Ok(reloaded)
    }

    /// Filter a message through all loaded plugins.
    ///
    /// Plugins are applied in insertion order. If any plugin drops the message,
    /// processing stops and None is returned.
    pub fn filter_message(
        &mut self,
        xml: &str,
        metadata: MessageMetadata,
    ) -> Result<Option<String>> {
        let mut current_xml = xml.to_string();

        for (name, plugin) in &mut self.plugins {
            let result = plugin.filter_message(&current_xml, metadata.clone())?;

            match result.action {
                FilterAction::Pass => {
                    // Continue with unchanged XML
                }
                FilterAction::Modify => {
                    if let Some(modified) = result.modified_xml {
                        current_xml = modified;
                    }
                }
                FilterAction::Drop => {
                    tracing::debug!(
                        "Message dropped by plugin {}: {}",
                        name,
                        result
                            .reason
                            .unwrap_or_else(|| "no reason given".to_string())
                    );
                    return Ok(None);
                }
            }
        }

        Ok(Some(current_xml))
    }

    /// Get information about all loaded plugins.
    pub fn list_plugins(&self) -> Vec<&PluginInfo> {
        self.plugins.values().map(|p| p.info()).collect()
    }

    /// Update global configuration for all plugins.
    pub fn update_config(&mut self, config: HashMap<String, String>) {
        self.config = config;
    }

    /// Update server list for all plugins.
    pub fn update_servers(&mut self, servers: Vec<ServerConfig>) {
        self.servers = servers;
    }

    /// Shutdown all plugins.
    pub fn shutdown_all(&mut self) -> Result<()> {
        for (name, plugin) in &mut self.plugins {
            if let Err(e) = plugin.shutdown() {
                tracing::error!("Failed to shutdown plugin {}: {}", name, e);
            }
        }
        self.plugins.clear();
        Ok(())
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        let _ = self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Protocol;

    #[test]
    fn test_plugin_manager_creation() {
        let config = HashMap::new();
        let servers = vec![ServerConfig::builder()
            .name("test-server")
            .host("localhost")
            .port(8089)
            .protocol(Protocol::Tcp)
            .build()];

        let manager = PluginManager::new("/tmp/plugins", config, servers);
        assert_eq!(manager.list_plugins().len(), 0);
    }

    #[test]
    fn test_filter_action_equality() {
        assert_eq!(FilterAction::Pass, FilterAction::Pass);
        assert_eq!(FilterAction::Modify, FilterAction::Modify);
        assert_eq!(FilterAction::Drop, FilterAction::Drop);
        assert_ne!(FilterAction::Pass, FilterAction::Drop);
    }
}
