use std::sync::Arc;
use wasmtime::component::Component;

use crate::error::PluginResult;
use crate::metadata::TransformerMetadata;
use crate::runtime::PluginRuntime;

/// WASM-based message transformer plugin
pub struct WasmTransformerPlugin {
    runtime: Arc<PluginRuntime>,
    component: Component,
    metadata: TransformerMetadata,
}

impl WasmTransformerPlugin {
    /// Create a new WASM transformer plugin
    pub fn new(
        runtime: Arc<PluginRuntime>,
        component: Component,
        metadata: TransformerMetadata,
    ) -> Self {
        Self {
            runtime,
            component,
            metadata,
        }
    }

    /// Load from file
    pub fn from_file(
        runtime: Arc<PluginRuntime>,
        path: &str,
        metadata: TransformerMetadata,
    ) -> PluginResult<Self> {
        let component = runtime.load_plugin_from_file(path)?;
        Ok(Self::new(runtime, component, metadata))
    }

    /// Load from bytes
    pub fn from_bytes(
        runtime: Arc<PluginRuntime>,
        wasm_bytes: &[u8],
        metadata: TransformerMetadata,
    ) -> PluginResult<Self> {
        let component = runtime.load_plugin(wasm_bytes)?;
        Ok(Self::new(runtime, component, metadata))
    }

    /// Get plugin metadata
    pub fn metadata(&self) -> &TransformerMetadata {
        &self.metadata
    }

    /// Transform a message
    pub async fn transform(&self, data: &[u8]) -> PluginResult<Vec<u8>> {
        // TODO: Implement WASM call
        // For now, return unchanged data
        Ok(data.to_vec())
    }

    /// Check if this transformer can handle a given CoT type
    pub fn can_transform(&self, cot_type: &str) -> bool {
        self.metadata.supported_types.iter().any(|pattern| {
            // Simple glob matching
            if pattern.ends_with('*') {
                let prefix = &pattern[..pattern.len() - 1];
                cot_type.starts_with(prefix)
            } else {
                cot_type == pattern
            }
        })
    }
}
