// OmniTAK Plugin API
// WASM-based plugin system for extensible TAK message processing

pub mod error;
pub mod manager;
pub mod metadata;
pub mod registry;
pub mod runtime;
pub mod security;
pub mod wasm_filter;
pub mod wasm_transformer;

// Generate bindings from WIT file
wasmtime::component::bindgen!({
    path: "wit/plugin.wit",
    world: "filter-plugin",
    async: true,
});

pub use error::{PluginError, PluginResult};
pub use manager::{PluginManager, PluginManagerConfig};
pub use metadata::{
    FilterMetadata, PluginCapability, PluginInfo, PluginMetadata, TransformerMetadata,
};
pub use registry::{PluginCategory, RegistryClient, RegistryManifest, RegistryPlugin};
pub use runtime::PluginRuntime;
pub use security::{ResourceLimits, SandboxPolicy};
pub use wasm_filter::WasmFilterPlugin;
pub use wasm_transformer::WasmTransformerPlugin;

// Re-export core types that plugins interact with
pub use omnitak_cot::{Event, Point};

/// Plugin API version
pub const PLUGIN_API_VERSION: &str = "0.1.0";

/// Maximum plugin execution time (microseconds)
pub const DEFAULT_MAX_EXECUTION_TIME_US: u64 = 1000; // 1ms

/// Maximum memory per plugin (bytes)
pub const DEFAULT_MAX_MEMORY_BYTES: u64 = 10 * 1024 * 1024; // 10MB

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_version() {
        assert_eq!(PLUGIN_API_VERSION, "0.1.0");
    }
}
