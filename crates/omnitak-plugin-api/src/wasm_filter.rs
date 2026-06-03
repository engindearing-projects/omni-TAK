use std::sync::Arc;
use std::time::Instant;
use wasmtime::component::{Component, Linker};
use wasmtime::Store;

use crate::error::{PluginError, PluginResult};
use crate::metadata::FilterMetadata;
use crate::runtime::{PluginRuntime, PluginState};

// Re-export filter types from omnitak-filter
pub use omnitak_filter::rules::{CotMessage, FilterResult, FilterRule};

// Import the generated WIT bindings
use crate::FilterPlugin;

/// WASM-based filter plugin that implements the FilterRule trait
pub struct WasmFilterPlugin {
    runtime: Arc<PluginRuntime>,
    component: Component,
    metadata: FilterMetadata,
}

impl WasmFilterPlugin {
    /// Create a new WASM filter plugin from compiled component
    pub fn new(
        runtime: Arc<PluginRuntime>,
        component: Component,
        metadata: FilterMetadata,
    ) -> Self {
        Self {
            runtime,
            component,
            metadata,
        }
    }

    /// Load a plugin from WASM file
    pub fn from_file(
        runtime: Arc<PluginRuntime>,
        path: &str,
        metadata: FilterMetadata,
    ) -> PluginResult<Self> {
        let component = runtime.load_plugin_from_file(path)?;
        Ok(Self::new(runtime, component, metadata))
    }

    /// Load a plugin from WASM bytes
    pub fn from_bytes(
        runtime: Arc<PluginRuntime>,
        wasm_bytes: &[u8],
        metadata: FilterMetadata,
    ) -> PluginResult<Self> {
        let component = runtime.load_plugin(wasm_bytes)?;
        Ok(Self::new(runtime, component, metadata))
    }

    /// Get plugin metadata
    pub fn metadata(&self) -> &FilterMetadata {
        &self.metadata
    }

    /// Create a new plugin instance
    async fn create_plugin_instance(&self) -> PluginResult<(Store<PluginState>, FilterPlugin)> {
        let mut store = self.runtime.create_store();
        let mut linker = Linker::new(self.runtime.engine());

        // Add host functions (WASI + custom bindings)
        Self::add_host_functions(&mut linker)?;

        // Instantiate the plugin using the generated bindings
        let plugin = FilterPlugin::instantiate_async(&mut store, &self.component, &linker)
            .await
            .map_err(|e| PluginError::InstantiationError(e.to_string()))?;

        Ok((store, plugin))
    }

    /// Add host functions that plugins can call
    fn add_host_functions(linker: &mut Linker<PluginState>) -> PluginResult<()> {
        // Add WASI support
        wasmtime_wasi::add_to_linker_async(linker)
            .map_err(|e| PluginError::InstantiationError(e.to_string()))?;

        // Add our custom host interface bindings using the generated trait
        FilterPlugin::add_to_linker(linker, |state| state)
            .map_err(|e| PluginError::InstantiationError(e.to_string()))?;

        Ok(())
    }

    /// Evaluate filter (internal implementation)
    async fn evaluate_async(&self, msg: &CotMessage<'_>) -> PluginResult<FilterResult> {
        let start = Instant::now();

        // Create new instance for this evaluation
        let (mut store, plugin) = self.create_plugin_instance().await?;

        // Convert CotMessage to WIT cot-message type
        // Note: WIT type has more fields than CotMessage, we provide defaults
        let wit_msg = crate::exports::omnitak::plugin::filter::CotMessage {
            cot_type: msg.cot_type.to_string(),
            uid: msg.uid.to_string(),
            callsign: msg.callsign.map(|s| s.to_string()),
            group: msg.group.map(|s| s.to_string()),
            team: msg.team.map(|s| s.to_string()),
            lat: msg.lat,
            lon: msg.lon,
            hae: msg.hae,
            time: String::new(), // Not available in CotMessage
            xml_payload: None,   // Not available in CotMessage
        };

        // Call the WASM exported evaluate function
        let result = plugin
            .omnitak_plugin_filter()
            .call_evaluate(&mut store, &wit_msg)
            .await
            .map_err(|e| PluginError::ExecutionError(e.to_string()))?;

        // Check timeout
        let elapsed = start.elapsed();
        if elapsed.as_micros() > self.metadata.max_execution_time_us as u128 {
            return Err(PluginError::Timeout(self.metadata.max_execution_time_us));
        }

        // Convert WIT filter-result to FilterResult
        match result {
            crate::exports::omnitak::plugin::filter::FilterResult::Pass => Ok(FilterResult::Pass),
            crate::exports::omnitak::plugin::filter::FilterResult::Block => Ok(FilterResult::Block),
        }
    }
}

impl FilterRule for WasmFilterPlugin {
    fn evaluate(&self, msg: &CotMessage) -> FilterResult {
        // Since FilterRule is sync but WASM is async, we need to block
        // In production, consider using a thread pool or async runtime
        tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|handle| handle.block_on(async { self.evaluate_async(msg).await.ok() }))
            .unwrap_or(FilterResult::Block) // Fail closed on error
    }

    fn describe(&self) -> String {
        format!(
            "WASM Filter Plugin: {} v{} (by {})\n{}\nMax execution time: {}μs",
            self.metadata.name,
            self.metadata.version,
            self.metadata.author,
            self.metadata.description,
            self.metadata.max_execution_time_us
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let _runtime = Arc::new(PluginRuntime::new().unwrap());
        let metadata = FilterMetadata {
            id: "test-filter".to_string(),
            name: "Test Filter".to_string(),
            version: "0.1.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test filter plugin".to_string(),
            max_execution_time_us: 1000,
        };

        // Note: Can't create actual plugin without WASM binary
        assert_eq!(metadata.id, "test-filter");
    }
}
