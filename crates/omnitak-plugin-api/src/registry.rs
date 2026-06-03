//! Plugin Registry - Discover and download plugins from remote repositories

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Plugin registry manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryManifest {
    /// Registry format version
    pub version: String,
    /// Registry name
    pub name: String,
    /// Registry description
    pub description: String,
    /// Registry URL (for self-reference)
    pub url: String,
    /// Last update timestamp (ISO 8601)
    pub updated_at: String,
    /// Available plugins
    pub plugins: Vec<RegistryPlugin>,
}

/// Plugin entry in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPlugin {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Plugin version
    pub version: String,
    /// Plugin author
    pub author: String,
    /// Plugin description
    pub description: String,
    /// Plugin category
    pub category: PluginCategory,
    /// Download URL for the WASM binary
    pub download_url: String,
    /// SHA-256 hash of the WASM binary for verification
    pub sha256: String,
    /// File size in bytes
    pub size: u64,
    /// Plugin capabilities/features
    pub capabilities: Vec<String>,
    /// Minimum OmniTAK version required
    pub min_omnitak_version: String,
    /// Plugin license
    pub license: String,
    /// Homepage/documentation URL
    pub homepage: Option<String>,
    /// Repository URL
    pub repository: Option<String>,
    /// Keywords for search
    pub keywords: Vec<String>,
    /// Dependencies on other plugins
    pub dependencies: Vec<String>,
    /// Download count (for popularity)
    pub downloads: u64,
    /// Average rating (0-5)
    pub rating: f32,
    /// Number of ratings
    pub rating_count: u64,
    /// Publication date (ISO 8601)
    pub published_at: String,
}

/// Plugin category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginCategory {
    /// Message filtering
    Filter,
    /// Message transformation
    Transformer,
    /// Video streaming
    Video,
    /// Sensor integration
    Sensor,
    /// Map/geospatial
    Mapping,
    /// Communication
    Communication,
    /// Analytics/reporting
    Analytics,
    /// Security
    Security,
    /// Utility
    Utility,
    /// Other
    Other,
}

impl PluginCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Transformer => "transformer",
            Self::Video => "video",
            Self::Sensor => "sensor",
            Self::Mapping => "mapping",
            Self::Communication => "communication",
            Self::Analytics => "analytics",
            Self::Security => "security",
            Self::Utility => "utility",
            Self::Other => "other",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Filter => "🔍",
            Self::Transformer => "🔄",
            Self::Video => "📹",
            Self::Sensor => "📡",
            Self::Mapping => "🗺",
            Self::Communication => "📡",
            Self::Analytics => "📊",
            Self::Security => "🔒",
            Self::Utility => "🔧",
            Self::Other => "📦",
        }
    }
}

/// Registry client for fetching plugin information
pub struct RegistryClient {
    /// HTTP client
    client: reqwest::Client,
    /// Registry URLs
    registries: Vec<String>,
    /// Cached manifests
    cache: HashMap<String, RegistryManifest>,
}

impl RegistryClient {
    /// Create a new registry client with default registries
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            registries: vec![
                // Default OmniTAK plugin registry (placeholder)
                "https://plugins.omnitak.org/registry.json".to_string(),
            ],
            cache: HashMap::new(),
        }
    }

    /// Add a custom registry URL
    pub fn add_registry(&mut self, url: String) {
        if !self.registries.contains(&url) {
            self.registries.push(url);
        }
    }

    /// Fetch registry manifest from URL
    pub async fn fetch_registry(&mut self, url: &str) -> Result<RegistryManifest, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch registry: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Registry returned status: {}", response.status()));
        }

        let manifest: RegistryManifest = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse registry manifest: {}", e))?;

        // Cache the manifest
        self.cache.insert(url.to_string(), manifest.clone());

        Ok(manifest)
    }

    /// Refresh all registries
    pub async fn refresh_all(&mut self) -> Result<Vec<RegistryManifest>, String> {
        let mut manifests = Vec::new();
        let mut errors = Vec::new();

        for url in self.registries.clone() {
            match self.fetch_registry(&url).await {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => errors.push(format!("{}: {}", url, e)),
            }
        }

        if manifests.is_empty() && !errors.is_empty() {
            return Err(format!(
                "Failed to fetch any registries: {}",
                errors.join(", ")
            ));
        }

        Ok(manifests)
    }

    /// Search for plugins across all cached registries
    pub fn search(&self, query: &str) -> Vec<&RegistryPlugin> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for manifest in self.cache.values() {
            for plugin in &manifest.plugins {
                let matches = plugin.name.to_lowercase().contains(&query_lower)
                    || plugin.description.to_lowercase().contains(&query_lower)
                    || plugin.id.to_lowercase().contains(&query_lower)
                    || plugin.author.to_lowercase().contains(&query_lower)
                    || plugin
                        .keywords
                        .iter()
                        .any(|k| k.to_lowercase().contains(&query_lower));

                if matches {
                    results.push(plugin);
                }
            }
        }

        // Sort by downloads (popularity)
        results.sort_by(|a, b| b.downloads.cmp(&a.downloads));

        results
    }

    /// Get plugins by category
    pub fn by_category(&self, category: PluginCategory) -> Vec<&RegistryPlugin> {
        let mut results = Vec::new();

        for manifest in self.cache.values() {
            for plugin in &manifest.plugins {
                if plugin.category == category {
                    results.push(plugin);
                }
            }
        }

        results.sort_by(|a, b| b.downloads.cmp(&a.downloads));
        results
    }

    /// Get all available plugins
    pub fn all_plugins(&self) -> Vec<&RegistryPlugin> {
        let mut results = Vec::new();

        for manifest in self.cache.values() {
            for plugin in &manifest.plugins {
                results.push(plugin);
            }
        }

        results.sort_by(|a, b| b.downloads.cmp(&a.downloads));
        results
    }

    /// Download plugin WASM binary
    pub async fn download_plugin(&self, plugin: &RegistryPlugin) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(&plugin.download_url)
            .send()
            .await
            .map_err(|e| format!("Failed to download plugin: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Plugin download returned status: {}",
                response.status()
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read plugin bytes: {}", e))?;

        // Verify SHA-256 hash
        let hash = sha256_hex(&bytes);
        if hash != plugin.sha256 {
            return Err(format!(
                "SHA-256 mismatch: expected {}, got {}",
                plugin.sha256, hash
            ));
        }

        Ok(bytes.to_vec())
    }
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate SHA-256 hash and return as hex string
fn sha256_hex(data: &[u8]) -> String {
    use std::io::Write;

    // Simple SHA-256 implementation placeholder
    // In production, use sha2 crate
    let mut hasher = Sha256::new();
    hasher.write_all(data).unwrap();
    hasher.finalize()
}

/// Simple SHA-256 hasher (placeholder - use sha2 crate in production)
struct Sha256 {
    data: Vec<u8>,
}

impl Sha256 {
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    fn finalize(self) -> String {
        // Placeholder - compute actual SHA-256 hash
        // For now, just return a dummy hash based on length
        format!("{:064x}", self.data.len())
    }
}

impl std::io::Write for Sha256 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Create an example registry manifest
pub fn create_example_registry() -> RegistryManifest {
    RegistryManifest {
        version: "1.0.0".to_string(),
        name: "OmniTAK Plugin Registry".to_string(),
        description: "Official plugin registry for OmniTAK".to_string(),
        url: "https://plugins.omnitak.org/registry.json".to_string(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        plugins: vec![
            RegistryPlugin {
                id: "callsign-filter".to_string(),
                name: "Callsign Filter".to_string(),
                version: "1.0.0".to_string(),
                author: "OmniTAK Team".to_string(),
                description: "Filter messages by callsign patterns using regex".to_string(),
                category: PluginCategory::Filter,
                download_url: "https://plugins.omnitak.org/callsign-filter-1.0.0.wasm".to_string(),
                sha256: "abc123...".to_string(),
                size: 50_000,
                capabilities: vec!["filter".to_string(), "regex".to_string()],
                min_omnitak_version: "0.2.0".to_string(),
                license: "MIT".to_string(),
                homepage: Some("https://github.com/omnitak/callsign-filter".to_string()),
                repository: Some("https://github.com/omnitak/callsign-filter".to_string()),
                keywords: vec![
                    "filter".to_string(),
                    "callsign".to_string(),
                    "regex".to_string(),
                ],
                dependencies: vec![],
                downloads: 1500,
                rating: 4.5,
                rating_count: 25,
                published_at: "2025-01-15T00:00:00Z".to_string(),
            },
            RegistryPlugin {
                id: "geo-fence".to_string(),
                name: "Geographic Fence".to_string(),
                version: "1.2.0".to_string(),
                author: "OmniTAK Team".to_string(),
                description: "Block messages outside defined geographic boundaries".to_string(),
                category: PluginCategory::Filter,
                download_url: "https://plugins.omnitak.org/geo-fence-1.2.0.wasm".to_string(),
                sha256: "def456...".to_string(),
                size: 75_000,
                capabilities: vec!["filter".to_string(), "geofence".to_string()],
                min_omnitak_version: "0.2.0".to_string(),
                license: "Apache-2.0".to_string(),
                homepage: None,
                repository: Some("https://github.com/omnitak/geo-fence".to_string()),
                keywords: vec![
                    "filter".to_string(),
                    "geofence".to_string(),
                    "boundary".to_string(),
                ],
                dependencies: vec![],
                downloads: 2300,
                rating: 4.8,
                rating_count: 42,
                published_at: "2025-02-01T00:00:00Z".to_string(),
            },
            RegistryPlugin {
                id: "video-stream-rtsp".to_string(),
                name: "RTSP Video Streamer".to_string(),
                version: "0.9.0".to_string(),
                author: "Community Contributor".to_string(),
                description: "Stream video from RTSP sources (drones, cameras) to TAK clients"
                    .to_string(),
                category: PluginCategory::Video,
                download_url: "https://plugins.omnitak.org/video-stream-rtsp-0.9.0.wasm"
                    .to_string(),
                sha256: "ghi789...".to_string(),
                size: 500_000,
                capabilities: vec![
                    "video".to_string(),
                    "rtsp".to_string(),
                    "streaming".to_string(),
                ],
                min_omnitak_version: "0.2.0".to_string(),
                license: "GPL-3.0".to_string(),
                homepage: None,
                repository: None,
                keywords: vec![
                    "video".to_string(),
                    "rtsp".to_string(),
                    "drone".to_string(),
                    "uav".to_string(),
                ],
                dependencies: vec![],
                downloads: 850,
                rating: 4.2,
                rating_count: 15,
                published_at: "2025-03-10T00:00:00Z".to_string(),
            },
            RegistryPlugin {
                id: "weather-sensor".to_string(),
                name: "Weather Station Integration".to_string(),
                version: "1.0.0".to_string(),
                author: "OmniTAK Team".to_string(),
                description: "Integrate weather station data as CoT messages".to_string(),
                category: PluginCategory::Sensor,
                download_url: "https://plugins.omnitak.org/weather-sensor-1.0.0.wasm".to_string(),
                sha256: "jkl012...".to_string(),
                size: 120_000,
                capabilities: vec!["sensor".to_string(), "weather".to_string()],
                min_omnitak_version: "0.2.0".to_string(),
                license: "MIT".to_string(),
                homepage: None,
                repository: None,
                keywords: vec![
                    "sensor".to_string(),
                    "weather".to_string(),
                    "temperature".to_string(),
                ],
                dependencies: vec![],
                downloads: 620,
                rating: 4.0,
                rating_count: 10,
                published_at: "2025-04-01T00:00:00Z".to_string(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_example_registry() {
        let registry = create_example_registry();
        assert_eq!(registry.plugins.len(), 4);
        assert!(registry.plugins.iter().any(|p| p.id == "callsign-filter"));
    }

    #[test]
    fn test_category_icon() {
        assert_eq!(PluginCategory::Video.icon(), "📹");
        assert_eq!(PluginCategory::Filter.icon(), "🔍");
    }

    #[test]
    fn test_serialize_registry() {
        let registry = create_example_registry();
        let json = serde_json::to_string_pretty(&registry).unwrap();
        assert!(json.contains("callsign-filter"));
    }
}
