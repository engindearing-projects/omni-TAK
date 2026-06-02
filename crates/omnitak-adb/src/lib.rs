//! OmniTAK ADB Integration
//!
//! This crate provides Android Debug Bridge (ADB) integration for pulling
//! TAK server certificates and configuration from connected Android devices
//! running ATAK.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod device;
pub mod monitor;
pub mod parser;

pub use device::{AdbDevice, DeviceInfo};
pub use monitor::DeviceMonitor;

/// ADB client for interacting with Android devices
#[derive(Debug, Clone)]
pub struct AdbClient {
    /// ADB executable path (defaults to "adb")
    pub adb_path: String,
}

impl Default for AdbClient {
    fn default() -> Self {
        Self {
            adb_path: "adb".to_string(),
        }
    }
}

impl AdbClient {
    /// Create a new ADB client
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if ADB is available
    pub fn is_available(&self) -> bool {
        Command::new(&self.adb_path)
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get ADB version
    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.adb_path)
            .arg("version")
            .output()
            .context("Failed to execute ADB")?;

        if !output.status.success() {
            return Err(anyhow!("ADB command failed"));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version
            .lines()
            .next()
            .unwrap_or("Unknown")
            .trim()
            .to_string())
    }

    /// List connected devices
    pub fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let output = Command::new(&self.adb_path)
            .args(["devices", "-l"])
            .output()
            .context("Failed to list devices")?;

        if !output.status.success() {
            return Err(anyhow!("Failed to list devices"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let devices = parser::parse_device_list(&stdout)?;

        Ok(devices)
    }

    /// Get a specific device by serial
    pub fn get_device(&self, serial: &str) -> Result<AdbDevice> {
        let devices = self.list_devices()?;

        if !devices.iter().any(|d| d.serial == serial) {
            return Err(anyhow!("Device {} not found", serial));
        }

        Ok(AdbDevice {
            serial: serial.to_string(),
            adb_client: self.clone(),
        })
    }

    /// Auto-detect a single connected device
    pub fn auto_detect_device(&self) -> Result<AdbDevice> {
        let devices = self.list_devices()?;

        match devices.len() {
            0 => Err(anyhow!("No devices connected")),
            1 => Ok(AdbDevice {
                serial: devices[0].serial.clone(),
                adb_client: self.clone(),
            }),
            _ => Err(anyhow!(
                "Multiple devices connected. Please specify a device serial."
            )),
        }
    }

    /// Execute ADB shell command
    pub fn shell(&self, device: &str, command: &[&str]) -> Result<String> {
        let mut args = vec!["-s", device, "shell"];
        args.extend_from_slice(command);

        let output = Command::new(&self.adb_path)
            .args(&args)
            .output()
            .context("Failed to execute ADB shell command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Shell command failed: {}", stderr));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Pull file from device
    pub fn pull(&self, device: &str, remote_path: &str, local_path: &Path) -> Result<()> {
        let status = Command::new(&self.adb_path)
            .args([
                "-s",
                device,
                "pull",
                remote_path,
                local_path.to_str().unwrap(),
            ])
            .status()
            .context("Failed to pull file")?;

        if !status.success() {
            return Err(anyhow!("Failed to pull file: {}", remote_path));
        }

        Ok(())
    }

    /// Check if a path exists on device
    pub fn path_exists(&self, device: &str, path: &str) -> bool {
        let result = Command::new(&self.adb_path)
            .args([
                "-s", device, "shell", "test", "-e", path, "&&", "echo", "exists",
            ])
            .output();

        if let Ok(output) = result {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return stdout.contains("exists");
        }

        false
    }
}

/// TAK certificate bundle extracted from device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakCertificateBundle {
    /// Server address (host:port)
    pub server_address: Option<String>,
    /// Server hostname
    pub server_host: Option<String>,
    /// Server port
    pub server_port: u16,
    /// Certificate files found
    pub certificates: Vec<CertificateFile>,
    /// TAK server name/identifier
    pub server_name: String,
    /// Protocol (tcp, tls, udp, ws)
    pub protocol: String,
}

/// Certificate file information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateFile {
    /// Original filename on device
    pub original_name: String,
    /// Local path where certificate is saved
    pub local_path: PathBuf,
    /// Certificate type (client, ca, key, p12)
    pub cert_type: CertificateType,
}

/// Type of certificate file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertificateType {
    /// Client certificate (.pem, .crt)
    Client,
    /// CA certificate (.pem, .crt)
    CA,
    /// Private key (.key, .pem)
    Key,
    /// PKCS#12 bundle (.p12, .pfx)
    P12,
    /// Unknown type
    Unknown,
}

impl CertificateType {
    /// Determine certificate type from filename
    pub fn from_filename(filename: &str) -> Self {
        let lower = filename.to_lowercase();

        if lower.ends_with(".p12") || lower.ends_with(".pfx") {
            return Self::P12;
        }

        if lower.contains("ca") || lower.contains("truststore") {
            return Self::CA;
        }

        if lower.ends_with(".key") || lower.contains("key") {
            return Self::Key;
        }

        if lower.ends_with(".pem") || lower.ends_with(".crt") || lower.ends_with(".cer") {
            return Self::Client;
        }

        Self::Unknown
    }
}

/// ATAK package variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtakPackage {
    /// Civilian ATAK
    Civilian,
    /// Military ATAK (if available)
    Military,
    /// WinTAK
    WinTak,
    /// Custom package
    Custom(&'static str),
}

impl AtakPackage {
    /// Get package name
    pub fn package_name(&self) -> &str {
        match self {
            Self::Civilian => "com.atakmap.app.civ",
            Self::Military => "com.atakmap.app.mil",
            Self::WinTak => "com.atakmap.app.wintak",
            Self::Custom(name) => name,
        }
    }

    /// Get all known package names
    pub fn all() -> Vec<Self> {
        vec![Self::Civilian, Self::Military, Self::WinTak]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_certificate_type_detection() {
        assert_eq!(
            CertificateType::from_filename("client.p12"),
            CertificateType::P12
        );
        assert_eq!(
            CertificateType::from_filename("truststore-ca.pem"),
            CertificateType::CA
        );
        assert_eq!(
            CertificateType::from_filename("client.key"),
            CertificateType::Key
        );
        assert_eq!(
            CertificateType::from_filename("client.pem"),
            CertificateType::Client
        );
    }

    #[test]
    fn test_atak_package_names() {
        assert_eq!(AtakPackage::Civilian.package_name(), "com.atakmap.app.civ");
        assert_eq!(AtakPackage::Military.package_name(), "com.atakmap.app.mil");
    }
}
