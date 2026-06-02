use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::error::{PluginError, PluginResult};
use crate::metadata::{FilterMetadata, PluginCapability, PluginInfo, TransformerMetadata};
use crate::runtime::PluginRuntime;
use crate::security::{ResourceLimits, SandboxPolicy};
use crate::wasm_filter::WasmFilterPlugin;
use crate::wasm_transformer::WasmTransformerPlugin;

/// Plugin manager configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManagerConfig {
    /// Resource limits for plugins
    pub resource_limits: ResourceLimits,
    /// Sandbox security policy
    pub sandbox_policy: SandboxPolicy,
    /// Plugin directory path
    pub plugin_dir: String,
    /// Enable hot-reload
    pub hot_reload: bool,
}

impl Default for PluginManagerConfig {
    fn default() -> Self {
        Self {
            resource_limits: ResourceLimits::default(),
            sandbox_policy: SandboxPolicy::strict(),
            plugin_dir: "plugins".to_string(),
            hot_reload: false,
        }
    }
}

/// Central plugin manager
pub struct PluginManager {
    runtime: Arc<PluginRuntime>,
    config: PluginManagerConfig,
    filter_plugins: DashMap<String, Arc<WasmFilterPlugin>>,
    transformer_plugins: DashMap<String, Arc<WasmTransformerPlugin>>,
    plugin_registry: DashMap<String, PluginInfo>,
}

impl PluginManager {
    /// Create a new plugin manager
    pub fn new(config: PluginManagerConfig) -> PluginResult<Self> {
        let runtime = Arc::new(PluginRuntime::with_config(
            config.resource_limits.clone(),
            config.sandbox_policy.clone(),
        )?);

        Ok(Self {
            runtime,
            config,
            filter_plugins: DashMap::new(),
            transformer_plugins: DashMap::new(),
            plugin_registry: DashMap::new(),
        })
    }

    /// Load a filter plugin from file
    pub fn load_filter_plugin(
        &self,
        path: &str,
        metadata: FilterMetadata,
    ) -> PluginResult<Arc<WasmFilterPlugin>> {
        tracing::info!("Loading filter plugin: {} from {}", metadata.name, path);

        // Verify plugin file exists
        if !Path::new(path).exists() {
            return Err(PluginError::LoadError(format!(
                "Plugin file not found: {}",
                path
            )));
        }

        // Calculate file hash for verification
        let plugin_bytes = std::fs::read(path)?;
        let hash = Self::calculate_hash(&plugin_bytes);

        // Load plugin
        let plugin =
            WasmFilterPlugin::from_bytes(self.runtime.clone(), &plugin_bytes, metadata.clone())?;
        let plugin = Arc::new(plugin);

        // Register plugin
        let plugin_id = metadata.id.clone();
        let plugin_info = PluginInfo {
            id: plugin_id.clone(),
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            author: metadata.author.clone(),
            description: metadata.description.clone(),
            capabilities: vec![PluginCapability::Filter],
            binary_hash: hash,
        };

        self.plugin_registry.insert(plugin_id.clone(), plugin_info);
        self.filter_plugins.insert(plugin_id, plugin.clone());

        tracing::info!("Successfully loaded filter plugin: {}", metadata.name);
        Ok(plugin)
    }

    /// Load a transformer plugin from file
    pub fn load_transformer_plugin(
        &self,
        path: &str,
        metadata: TransformerMetadata,
    ) -> PluginResult<Arc<WasmTransformerPlugin>> {
        tracing::info!(
            "Loading transformer plugin: {} from {}",
            metadata.name,
            path
        );

        let plugin_bytes = std::fs::read(path)?;
        let hash = Self::calculate_hash(&plugin_bytes);

        let plugin = WasmTransformerPlugin::from_bytes(
            self.runtime.clone(),
            &plugin_bytes,
            metadata.clone(),
        )?;
        let plugin = Arc::new(plugin);

        let plugin_id = metadata.id.clone();
        let plugin_info = PluginInfo {
            id: plugin_id.clone(),
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            author: metadata.author.clone(),
            description: metadata.description.clone(),
            capabilities: vec![PluginCapability::Transform],
            binary_hash: hash,
        };

        self.plugin_registry.insert(plugin_id.clone(), plugin_info);
        self.transformer_plugins.insert(plugin_id, plugin.clone());

        tracing::info!("Successfully loaded transformer plugin: {}", metadata.name);
        Ok(plugin)
    }

    /// Get a filter plugin by ID
    pub fn get_filter_plugin(&self, id: &str) -> Option<Arc<WasmFilterPlugin>> {
        self.filter_plugins.get(id).map(|p| p.clone())
    }

    /// Get a transformer plugin by ID
    pub fn get_transformer_plugin(&self, id: &str) -> Option<Arc<WasmTransformerPlugin>> {
        self.transformer_plugins.get(id).map(|p| p.clone())
    }

    /// List all loaded plugins
    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        self.plugin_registry
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Unload a plugin
    pub fn unload_plugin(&self, id: &str) -> PluginResult<()> {
        self.filter_plugins.remove(id);
        self.transformer_plugins.remove(id);
        self.plugin_registry
            .remove(id)
            .ok_or_else(|| PluginError::NotFound(id.to_string()))?;

        tracing::info!("Unloaded plugin: {}", id);
        Ok(())
    }

    /// Load all plugins from the configured plugin directory
    pub async fn load_all_plugins(&self) -> PluginResult<usize> {
        let plugin_dir = Path::new(&self.config.plugin_dir);
        if !plugin_dir.exists() {
            tracing::warn!(
                "Plugin directory does not exist: {}",
                self.config.plugin_dir
            );
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(plugin_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                tracing::info!("Found plugin: {:?}", path);
                // TODO: Load plugin metadata from companion file (.toml)
                // For now, skip automatic loading
                count += 1;
            }
        }

        Ok(count)
    }

    /// Calculate SHA-256 hash of plugin binary
    fn calculate_hash(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager_creation() {
        let config = PluginManagerConfig::default();
        let manager = PluginManager::new(config);
        assert!(manager.is_ok());
    }

    #[test]
    fn test_hash_calculation() {
        let data = b"test data";
        let hash = PluginManager::calculate_hash(data);
        assert_eq!(hash.len(), 64); // SHA-256 hex string length
    }
}
