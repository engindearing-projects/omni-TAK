//! ADB device operations

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info, warn};

use crate::{AdbClient, AtakPackage, CertificateFile, CertificateType, TakCertificateBundle};

/// Information about a connected device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Device serial number
    pub serial: String,
    /// Device state (device, offline, unauthorized)
    pub state: String,
    /// Device model
    pub model: Option<String>,
    /// Device product
    pub product: Option<String>,
    /// Transport ID
    pub transport_id: Option<String>,
}

/// Represents a connected ADB device
#[derive(Debug, Clone)]
pub struct AdbDevice {
    /// Device serial number
    pub serial: String,
    /// ADB client instance
    pub adb_client: AdbClient,
}

impl AdbDevice {
    /// Pull TAK certificates from device
    pub fn pull_tak_certificates(
        &self,
        output_dir: &Path,
        package: AtakPackage,
    ) -> Result<TakCertificateBundle> {
        info!("Pulling TAK certificates from device {}...", self.serial);

        // Create output directory
        fs::create_dir_all(output_dir).context("Failed to create output directory")?;

        // Paths to check for certificates
        let cert_paths = self.get_cert_paths(package);

        let mut certificates = Vec::new();

        // Try each path
        for path in &cert_paths {
            debug!("Checking path: {}", path);

            if self.adb_client.path_exists(&self.serial, path) {
                info!("Found certificates in: {}", path);

                // Pull all certificate files
                let files = self.list_cert_files(path)?;
                for file in files {
                    let remote_path = format!("{}/{}", path, file);
                    let local_path = output_dir.join(&file);

                    if let Ok(()) = self
                        .adb_client
                        .pull(&self.serial, &remote_path, &local_path)
                    {
                        info!("  ✓ Pulled: {}", file);

                        certificates.push(CertificateFile {
                            original_name: file.clone(),
                            local_path,
                            cert_type: CertificateType::from_filename(&file),
                        });
                    } else {
                        warn!("  ✗ Failed to pull: {}", file);
                    }
                }

                break;
            }
        }

        if certificates.is_empty() {
            warn!("No certificates found in standard locations");
            // Try to find .p12 files anywhere
            certificates = self.find_p12_files(output_dir)?;
        }

        // Pull server configuration
        let server_config = self.pull_server_config(package)?;

        Ok(TakCertificateBundle {
            server_address: server_config.get("address").cloned(),
            server_host: server_config.get("host").cloned(),
            server_port: server_config
                .get("port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(8089),
            certificates,
            server_name: server_config
                .get("name")
                .cloned()
                .unwrap_or_else(|| "tak-server-from-device".to_string()),
            protocol: "tls".to_string(),
        })
    }

    /// Get certificate paths to check
    fn get_cert_paths(&self, package: AtakPackage) -> Vec<String> {
        vec![
            "/sdcard/atak/cert".to_string(),
            "/sdcard/atak/certs".to_string(),
            "/storage/emulated/0/atak/cert".to_string(),
            "/storage/emulated/0/atak/certs".to_string(),
            format!("/data/data/{}/files/cert", package.package_name()),
            format!("/data/data/{}/files/certs", package.package_name()),
        ]
    }

    /// List certificate files in a directory
    fn list_cert_files(&self, path: &str) -> Result<Vec<String>> {
        let output = self
            .adb_client
            .shell(&self.serial, &["ls", path])
            .context("Failed to list directory")?;

        let files: Vec<String> = output
            .lines()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty()
                    && (line.ends_with(".p12")
                        || line.ends_with(".pem")
                        || line.ends_with(".key")
                        || line.ends_with(".crt")
                        || line.ends_with(".pfx")
                        || line.ends_with(".cer"))
            })
            .map(|s| s.trim().to_string())
            .collect();

        Ok(files)
    }

    /// Find .p12 files anywhere on device
    fn find_p12_files(&self, output_dir: &Path) -> Result<Vec<CertificateFile>> {
        info!("Searching for .p12 files on device...");

        let mut certificates = Vec::new();
        let search_paths = vec!["/sdcard", "/storage/emulated/0"];

        for base_path in search_paths {
            let result = Command::new(&self.adb_client.adb_path)
                .args([
                    "-s",
                    &self.serial,
                    "shell",
                    "find",
                    base_path,
                    "-name",
                    "*.p12",
                    "-type",
                    "f",
                    "2>/dev/null",
                ])
                .output();

            if let Ok(output) = result {
                if output.status.success() {
                    let files = String::from_utf8_lossy(&output.stdout);
                    for file in files.lines() {
                        let file = file.trim();
                        if !file.is_empty() {
                            let filename = Path::new(file)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown.p12");
                            let local_path = output_dir.join(filename);

                            if self
                                .adb_client
                                .pull(&self.serial, file, &local_path)
                                .is_ok()
                            {
                                info!("  ✓ Found and pulled: {}", filename);
                                certificates.push(CertificateFile {
                                    original_name: filename.to_string(),
                                    local_path,
                                    cert_type: CertificateType::P12,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(certificates)
    }

    /// Pull TAK server configuration from device
    fn pull_server_config(
        &self,
        package: AtakPackage,
    ) -> Result<std::collections::HashMap<String, String>> {
        info!("Pulling TAK server configuration...");

        let pref_paths = vec![
            format!(
                "/data/data/{}/shared_prefs/{}_preferences.xml",
                package.package_name(),
                package.package_name()
            ),
            format!(
                "/data/data/{}/shared_prefs/com.atakmap.app_preferences.xml",
                package.package_name()
            ),
        ];

        for pref_path in pref_paths {
            debug!("Checking preferences: {}", pref_path);

            let temp_file = std::env::temp_dir().join("atak_prefs.xml");

            if self
                .adb_client
                .pull(&self.serial, &pref_path, &temp_file)
                .is_ok()
            {
                info!("  ✓ Pulled preferences from: {}", pref_path);

                // Parse preferences
                if let Ok(content) = fs::read_to_string(&temp_file) {
                    let config = crate::parser::parse_tak_preferences(&content);

                    // Clean up
                    let _ = fs::remove_file(temp_file);

                    return Ok(config);
                }
            }
        }

        warn!("Could not access preferences (may require root access)");

        // Return default config
        let mut config = std::collections::HashMap::new();
        config.insert("name".to_string(), "tak-server-from-device".to_string());
        config.insert("host".to_string(), "takserver.example.com".to_string());
        config.insert("port".to_string(), "8089".to_string());

        Ok(config)
    }
}
