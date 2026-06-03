use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::DEFAULT_MAX_EXECUTION_TIME_US;
use crate::DEFAULT_MAX_MEMORY_BYTES;

/// Resource limits for plugin execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum execution time per call
    pub max_execution_time: Duration,
    /// Maximum memory allocation (bytes)
    pub max_memory_bytes: u64,
    /// Maximum number of concurrent executions
    pub max_concurrent_executions: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_micros(DEFAULT_MAX_EXECUTION_TIME_US),
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_concurrent_executions: 100,
        }
    }
}

/// Sandbox security policy
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxPolicy {
    /// Allow network access
    pub allow_network: bool,
    /// Allow filesystem read access
    pub allow_filesystem_read: bool,
    /// Allow filesystem write access
    pub allow_filesystem_write: bool,
    /// Allow environment variable access
    pub allow_env_vars: bool,
    /// Allowed filesystem paths (if filesystem access enabled)
    pub allowed_paths: Vec<String>,
}

impl SandboxPolicy {
    /// Create a strict sandbox (no permissions)
    pub fn strict() -> Self {
        Self::default()
    }

    /// Create a permissive sandbox (all permissions)
    pub fn permissive() -> Self {
        Self {
            allow_network: true,
            allow_filesystem_read: true,
            allow_filesystem_write: true,
            allow_env_vars: true,
            allowed_paths: vec!["/".to_string()],
        }
    }

    /// Create sandbox with read-only filesystem access
    pub fn read_only_fs(paths: Vec<String>) -> Self {
        Self {
            allow_network: false,
            allow_filesystem_read: true,
            allow_filesystem_write: false,
            allow_env_vars: false,
            allowed_paths: paths,
        }
    }
}
