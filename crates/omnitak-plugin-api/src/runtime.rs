use wasmtime::*;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

use crate::error::{PluginError, PluginResult};
use crate::security::{ResourceLimits, SandboxPolicy};

/// WASM plugin runtime environment
pub struct PluginRuntime {
    engine: Engine,
    resource_limits: ResourceLimits,
    sandbox_policy: SandboxPolicy,
}

impl PluginRuntime {
    /// Create a new plugin runtime with default configuration
    pub fn new() -> PluginResult<Self> {
        Self::with_config(ResourceLimits::default(), SandboxPolicy::default())
    }

    /// Create a plugin runtime with custom configuration
    pub fn with_config(
        resource_limits: ResourceLimits,
        sandbox_policy: SandboxPolicy,
    ) -> PluginResult<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);

        // Configure resource limits
        config.epoch_interruption(true);
        config.max_wasm_stack(1024 * 1024); // 1MB stack

        // Use Cranelift optimizer for best performance
        config.cranelift_opt_level(OptLevel::Speed);

        let engine =
            Engine::new(&config).map_err(|e| PluginError::CompilationError(e.to_string()))?;

        Ok(Self {
            engine,
            resource_limits,
            sandbox_policy,
        })
    }

    /// Get the WASM engine
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Get resource limits
    pub fn resource_limits(&self) -> &ResourceLimits {
        &self.resource_limits
    }

    /// Get sandbox policy
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }

    /// Create a new store with configured limits
    pub fn create_store(&self) -> Store<PluginState> {
        let wasi_ctx = self.create_wasi_context();
        let state = PluginState {
            wasi_ctx,
            limits: self.resource_limits.clone(),
        };
        let mut store = Store::new(self.engine(), state);

        // Set memory limits
        store.limiter(|state| &mut state.limits);

        store
    }

    /// Create WASI context based on sandbox policy
    fn create_wasi_context(&self) -> WasiCtx {
        let builder = &mut WasiCtxBuilder::new();

        // Configure based on sandbox policy
        if self.sandbox_policy.allow_env_vars {
            builder.inherit_env();
        }

        if self.sandbox_policy.allow_network {
            builder.inherit_network();
        }

        // Add allowed filesystem paths
        for path in &self.sandbox_policy.allowed_paths {
            if self.sandbox_policy.allow_filesystem_write
                || self.sandbox_policy.allow_filesystem_read
            {
                // Determine permissions based on policy
                let (dir_perms, file_perms) = if self.sandbox_policy.allow_filesystem_write {
                    (
                        wasmtime_wasi::DirPerms::all(),
                        wasmtime_wasi::FilePerms::all(),
                    )
                } else {
                    (
                        wasmtime_wasi::DirPerms::READ,
                        wasmtime_wasi::FilePerms::READ,
                    )
                };

                // Add preopened directory with specified permissions
                if let Err(e) = builder.preopened_dir(path, path, dir_perms, file_perms) {
                    tracing::warn!("Failed to add preopened directory {}: {}", path, e);
                }
            }
        }

        builder.build()
    }

    /// Load and compile a plugin from bytes
    pub fn load_plugin(&self, wasm_bytes: &[u8]) -> PluginResult<wasmtime::component::Component> {
        wasmtime::component::Component::from_binary(&self.engine, wasm_bytes)
            .map_err(|e| PluginError::LoadError(e.to_string()))
    }

    /// Load and compile a plugin from file
    pub fn load_plugin_from_file(
        &self,
        path: &str,
    ) -> PluginResult<wasmtime::component::Component> {
        let wasm_bytes = std::fs::read(path)?;
        self.load_plugin(&wasm_bytes)
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new().expect("Failed to create default plugin runtime")
    }
}

/// Plugin execution state
pub struct PluginState {
    wasi_ctx: WasiCtx,
    limits: ResourceLimits,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi_ctx
    }

    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        unimplemented!("Resource table not yet implemented")
    }
}

// Resource limiter implementation
impl wasmtime::ResourceLimiter for ResourceLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if desired as u64 > self.max_memory_bytes {
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        // Allow reasonable table growth
        Ok(desired < 10000)
    }
}

// Implement Host trait for plugin imports (async version)
#[wasmtime::component::__internal::async_trait]
impl crate::omnitak::plugin::host::Host for PluginState {
    async fn log(&mut self, level: crate::omnitak::plugin::host::LogLevel, message: String) {
        match level {
            crate::omnitak::plugin::host::LogLevel::Trace => tracing::trace!("Plugin: {}", message),
            crate::omnitak::plugin::host::LogLevel::Debug => tracing::debug!("Plugin: {}", message),
            crate::omnitak::plugin::host::LogLevel::Info => tracing::info!("Plugin: {}", message),
            crate::omnitak::plugin::host::LogLevel::Warn => tracing::warn!("Plugin: {}", message),
            crate::omnitak::plugin::host::LogLevel::Error => tracing::error!("Plugin: {}", message),
        }
    }

    async fn get_current_time_ms(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    async fn query_elevation(&mut self, _lat: f64, _lon: f64) -> Option<f64> {
        // Elevation queries not implemented yet
        None
    }
}
