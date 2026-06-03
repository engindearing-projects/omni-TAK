//! OmniTAK ADB Setup Tool
//!
//! This tool uses Android Debug Bridge (ADB) to pull TAK server certificates
//! and configuration from a connected Android device running ATAK/WinTAK,
//! then generates an omniTAK configuration file.
//!
//! # Usage
//!
//! ```bash
//! # List available devices
//! omnitak-adb-setup --list-devices
//!
//! # Pull certificates and generate config
//! omnitak-adb-setup --output config/config.yaml
//!
//! # Specify device serial number if multiple devices connected
//! omnitak-adb-setup --device abc123 --output config/config.yaml
//!
//! # Pull certificates to a specific directory
//! omnitak-adb-setup --cert-dir ./certs --output config/config.yaml
//! ```

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info, warn};

/// OmniTAK ADB Setup Tool - Pull TAK certificates from Android devices
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// List available ADB devices
    #[arg(short, long)]
    list_devices: bool,

    /// Device serial number (optional if only one device connected)
    #[arg(short, long)]
    device: Option<String>,

    /// Output configuration file path
    #[arg(short, long, default_value = "config/config.yaml")]
    output: PathBuf,

    /// Certificate output directory
    #[arg(short = 'C', long, default_value = "certs")]
    cert_dir: PathBuf,

    /// ATAK package name (use for different ATAK variants)
    #[arg(long, default_value = "com.atakmap.app.civ")]
    package: String,

    /// Skip certificate validation
    #[arg(long)]
    skip_validation: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(false)
        .init();

    info!("OmniTAK ADB Setup Tool v{}", env!("CARGO_PKG_VERSION"));

    // Check if ADB is available
    check_adb_available()?;

    // List devices if requested
    if args.list_devices {
        list_devices()?;
        return Ok(());
    }

    // Get device serial
    let device_serial = get_device_serial(args.device.as_deref())?;
    info!("Using device: {}", device_serial);

    // Create certificate directory
    fs::create_dir_all(&args.cert_dir).context("Failed to create certificate directory")?;

    // Pull certificates and configuration
    let config = pull_tak_config(
        &device_serial,
        &args.package,
        &args.cert_dir,
        args.skip_validation,
    )?;

    // Generate omniTAK configuration
    let output_dir = args.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(output_dir).context("Failed to create output directory")?;

    save_config(&config, &args.output)?;

    info!("✓ Configuration saved to: {}", args.output.display());
    info!("✓ Certificates saved to: {}", args.cert_dir.display());
    info!("\nYou can now start OmniTAK with:");
    info!("  cargo run -- --config {}", args.output.display());

    Ok(())
}

/// Check if ADB is available in the system PATH
fn check_adb_available() -> Result<()> {
    debug!("Checking for ADB...");

    let output = Command::new("adb")
        .arg("version")
        .output()
        .context("Failed to execute 'adb'. Make sure Android SDK Platform-Tools is installed and in your PATH")?;

    if !output.status.success() {
        return Err(anyhow!("ADB is not working correctly"));
    }

    let version = String::from_utf8_lossy(&output.stdout);
    debug!("Found ADB: {}", version.lines().next().unwrap_or("unknown"));

    Ok(())
}

/// List available ADB devices
fn list_devices() -> Result<()> {
    info!("Listing available devices...");

    let output = Command::new("adb")
        .arg("devices")
        .arg("-l")
        .output()
        .context("Failed to list devices")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to list devices: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("\n{}", stdout);

    Ok(())
}

/// Get device serial number (auto-detect if only one device)
fn get_device_serial(device: Option<&str>) -> Result<String> {
    if let Some(serial) = device {
        return Ok(serial.to_string());
    }

    // Auto-detect device
    debug!("Auto-detecting device...");

    let output = Command::new("adb")
        .arg("devices")
        .output()
        .context("Failed to list devices")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let devices: Vec<&str> = stdout
        .lines()
        .skip(1) // Skip header
        .filter(|line| !line.trim().is_empty() && line.contains("device"))
        .collect();

    match devices.len() {
        0 => Err(anyhow!(
            "No devices found. Connect an Android device with ADB enabled"
        )),
        1 => {
            let serial = devices[0]
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow!("Failed to parse device serial"))?;
            Ok(serial.to_string())
        }
        _ => {
            error!("Multiple devices found. Please specify device serial with --device");
            list_devices()?;
            Err(anyhow!("Multiple devices found"))
        }
    }
}

/// Pull TAK configuration from device
fn pull_tak_config(
    device: &str,
    package: &str,
    cert_dir: &Path,
    skip_validation: bool,
) -> Result<OmniTakConfig> {
    info!("Pulling TAK configuration from device...");

    // Paths to check for certificates on Android device
    let cert_paths: Vec<String> = vec![
        "/sdcard/atak/cert".to_string(),
        "/storage/emulated/0/atak/cert".to_string(),
        format!("/data/data/{}/files/cert", package),
    ];

    let mut found_certs = false;
    let mut config = OmniTakConfig::default();

    for path in &cert_paths {
        debug!("Checking path: {}", path);

        // List files in directory
        let output = Command::new("adb")
            .args(["-s", device, "shell", "ls", path])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let files = String::from_utf8_lossy(&output.stdout);
                if !files.trim().is_empty() && !files.contains("No such file") {
                    info!("Found certificates in: {}", path);
                    pull_certificates(device, path, cert_dir, &mut config)?;
                    found_certs = true;
                    break;
                }
            }
        }
    }

    if !found_certs {
        warn!("No certificates found in standard locations");
        warn!("Attempting to pull from any accessible location...");

        // Try to find .p12 files anywhere on the device
        find_and_pull_p12_files(device, cert_dir, &mut config)?;
    }

    // Pull TAK server preferences/configuration
    pull_tak_server_config(device, package, &mut config)?;

    // Validate configuration
    if !skip_validation && config.servers.is_empty() {
        warn!("No TAK server configurations found");
        warn!(
            "You may need to manually configure server details in: {}",
            "config/config.yaml"
        );
    }

    Ok(config)
}

/// Pull certificates from device
fn pull_certificates(
    device: &str,
    remote_path: &str,
    local_dir: &Path,
    config: &mut OmniTakConfig,
) -> Result<()> {
    info!("Pulling certificates from {}...", remote_path);

    // List all certificate files
    let output = Command::new("adb")
        .args(["-s", device, "shell", "ls", remote_path])
        .output()?;

    let files = String::from_utf8_lossy(&output.stdout);

    for file in files.lines() {
        let file = file.trim();
        if file.is_empty() {
            continue;
        }

        // Check for certificate files
        if file.ends_with(".p12")
            || file.ends_with(".pem")
            || file.ends_with(".key")
            || file.ends_with(".crt")
            || file.ends_with(".pfx")
            || file.ends_with(".cer")
        {
            let remote_file = format!("{}/{}", remote_path, file);
            let local_file = local_dir.join(file);

            debug!("Pulling: {} -> {}", remote_file, local_file.display());

            let status = Command::new("adb")
                .args([
                    "-s",
                    device,
                    "pull",
                    &remote_file,
                    local_file.to_str().unwrap(),
                ])
                .status()?;

            if status.success() {
                info!("  ✓ Pulled: {}", file);

                // Track certificate files for config generation
                if file.ends_with(".p12") || file.ends_with(".pfx") {
                    config.p12_files.push(local_file);
                } else if file.ends_with(".pem") && file.contains("truststore") {
                    config.ca_cert = Some(local_file);
                } else if file.ends_with(".pem") {
                    config.client_cert = Some(local_file);
                } else if file.ends_with(".key") {
                    config.client_key = Some(local_file);
                }
            } else {
                warn!("  ✗ Failed to pull: {}", file);
            }
        }
    }

    Ok(())
}

/// Find and pull .p12 files from device
fn find_and_pull_p12_files(
    device: &str,
    local_dir: &Path,
    config: &mut OmniTakConfig,
) -> Result<()> {
    info!("Searching for .p12 certificate files...");

    // Search in accessible locations
    let search_paths = vec!["/sdcard", "/storage/emulated/0"];

    for base_path in search_paths {
        let output = Command::new("adb")
            .args([
                "-s", device, "shell", "find", base_path, "-name", "*.p12", "-type", "f",
            ])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let files = String::from_utf8_lossy(&output.stdout);
                for file in files.lines() {
                    let file = file.trim();
                    if !file.is_empty() {
                        let filename = Path::new(file)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown.p12");
                        let local_file = local_dir.join(filename);

                        debug!("Found: {}", file);

                        let status = Command::new("adb")
                            .args(["-s", device, "pull", file, local_file.to_str().unwrap()])
                            .status()?;

                        if status.success() {
                            info!("  ✓ Pulled: {}", filename);
                            config.p12_files.push(local_file);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Pull TAK server configuration from preferences
fn pull_tak_server_config(device: &str, package: &str, config: &mut OmniTakConfig) -> Result<()> {
    info!("Pulling TAK server configuration...");

    // Try to pull preferences XML
    let pref_paths = vec![
        format!(
            "/data/data/{}/shared_prefs/{}_preferences.xml",
            package, package
        ),
        format!(
            "/data/data/{}/shared_prefs/com.atakmap.app_preferences.xml",
            package
        ),
    ];

    for pref_path in pref_paths {
        debug!("Checking preferences: {}", pref_path);

        let temp_file = std::env::temp_dir().join("atak_prefs.xml");

        let status = Command::new("adb")
            .args([
                "-s",
                device,
                "pull",
                &pref_path,
                temp_file.to_str().unwrap(),
            ])
            .status();

        if let Ok(status) = status {
            if status.success() {
                info!("  ✓ Pulled preferences from: {}", pref_path);

                // Parse preferences for server information
                if let Ok(content) = fs::read_to_string(&temp_file) {
                    parse_tak_preferences(&content, config);
                }

                // Clean up temp file
                let _ = fs::remove_file(temp_file);

                return Ok(());
            }
        }
    }

    warn!("Could not access preferences (may require root access)");
    info!("Will generate default server configuration");

    // Add default server if none found
    if config.servers.is_empty() {
        config.servers.push(ServerInfo {
            name: "tak-server-from-device".to_string(),
            address: "takserver.example.com".to_string(),
            port: 8089,
            protocol: "tls".to_string(),
        });
    }

    Ok(())
}

/// Parse TAK preferences XML for server information
fn parse_tak_preferences(content: &str, config: &mut OmniTakConfig) {
    debug!("Parsing TAK preferences...");

    // Simple XML parsing - look for common server preference keys
    // This is a simplified approach; a full XML parser could be used for robustness

    for line in content.lines() {
        // Look for server address
        if line.contains("caLocation") || line.contains("serverConnectString") {
            if let Some(value) = extract_xml_string_value(line) {
                debug!("Found server reference: {}", value);

                // Parse server:port format
                if let Some((host, port)) = parse_server_address(&value) {
                    config.servers.push(ServerInfo {
                        name: format!("tak-server-{}", config.servers.len() + 1),
                        address: host,
                        port,
                        protocol: "tls".to_string(),
                    });
                }
            }
        }
    }

    if !config.servers.is_empty() {
        info!("  ✓ Found {} TAK server(s)", config.servers.len());
    }
}

/// Extract string value from XML line
fn extract_xml_string_value(line: &str) -> Option<String> {
    // Look for: <string name="key">value</string>
    if let Some(start) = line.find('>') {
        if let Some(end) = line.rfind('<') {
            if start < end {
                let value = &line[start + 1..end];
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Parse server address in format "host:port" or just "host"
fn parse_server_address(addr: &str) -> Option<(String, u16)> {
    let addr = addr.trim();

    if let Some(colon) = addr.rfind(':') {
        let host = addr[..colon].to_string();
        let port = addr[colon + 1..].parse().unwrap_or(8089);
        Some((host, port))
    } else {
        // Default port 8089 for TLS
        Some((addr.to_string(), 8089))
    }
}

/// Save configuration to YAML file
fn save_config(config: &OmniTakConfig, output: &Path) -> Result<()> {
    info!("Generating configuration file...");

    let mut yaml = String::new();
    yaml.push_str("# OmniTAK Configuration\n");
    yaml.push_str("# Generated by omnitak-adb-setup\n");
    yaml.push_str(&format!("# Generated at: {}\n\n", chrono::Utc::now()));

    yaml.push_str("application:\n");
    yaml.push_str("  max_connections: 100\n");
    yaml.push_str("  worker_threads: 4\n\n");

    yaml.push_str("servers:\n");

    for (i, server) in config.servers.iter().enumerate() {
        yaml.push_str(&format!("  - id: {}\n", server.name));
        yaml.push_str(&format!(
            "    address: \"{}:{}\"\n",
            server.address, server.port
        ));
        yaml.push_str(&format!("    protocol: {}\n", server.protocol));
        yaml.push_str("    auto_reconnect: true\n");
        yaml.push_str("    reconnect_delay_ms: 5000\n");

        if server.protocol == "tls" {
            yaml.push_str("    tls:\n");

            // If we have a .p12 file, note that it needs conversion
            if !config.p12_files.is_empty() {
                let p12_file = &config.p12_files[0];
                yaml.push_str("      # IMPORTANT: Convert .p12 to PEM format first:\n");
                yaml.push_str(&format!(
                    "      # openssl pkcs12 -in {} -out client.pem -clcerts -nokeys\n",
                    p12_file.display()
                ));
                yaml.push_str(&format!(
                    "      # openssl pkcs12 -in {} -out client.key -nocerts -nodes\n",
                    p12_file.display()
                ));
                yaml.push_str(&format!(
                    "      # openssl pkcs12 -in {} -out ca.pem -cacerts -nokeys\n",
                    p12_file.display()
                ));
                yaml.push_str(&format!(
                    "      cert_path: \"{}/client.pem\"\n",
                    config.p12_files[0].parent().unwrap().display()
                ));
                yaml.push_str(&format!(
                    "      key_path: \"{}/client.key\"\n",
                    config.p12_files[0].parent().unwrap().display()
                ));
                yaml.push_str(&format!(
                    "      ca_path: \"{}/ca.pem\"\n",
                    config.p12_files[0].parent().unwrap().display()
                ));
            } else if let (Some(cert), Some(key), Some(ca)) =
                (&config.client_cert, &config.client_key, &config.ca_cert)
            {
                yaml.push_str(&format!("      cert_path: \"{}\"\n", cert.display()));
                yaml.push_str(&format!("      key_path: \"{}\"\n", key.display()));
                yaml.push_str(&format!("      ca_path: \"{}\"\n", ca.display()));
            } else {
                yaml.push_str("      cert_path: \"certs/client.pem\"\n");
                yaml.push_str("      key_path: \"certs/client.key\"\n");
                yaml.push_str("      ca_path: \"certs/ca.pem\"\n");
            }

            yaml.push_str("      validate_certs: true\n");
        }

        if i < config.servers.len() - 1 {
            yaml.push('\n');
        }
    }

    yaml.push('\n');
    yaml.push_str("filters:\n");
    yaml.push_str("  mode: whitelist\n");
    yaml.push_str("  rules:\n");
    yaml.push_str("    - id: all-friendly\n");
    yaml.push_str("      type: affiliation\n");
    yaml.push_str("      allow: [friend, assumedfriend]\n");
    yaml.push_str("      destinations: [");
    for (i, server) in config.servers.iter().enumerate() {
        yaml.push_str(&server.name);
        if i < config.servers.len() - 1 {
            yaml.push_str(", ");
        }
    }
    yaml.push_str("]\n\n");

    yaml.push_str("api:\n");
    yaml.push_str("  bind_addr: \"127.0.0.1:9443\"\n");
    yaml.push_str("  enable_tls: false\n");
    yaml.push_str("  jwt_expiration: 86400\n");
    yaml.push_str("  rate_limit_rps: 100\n");
    yaml.push_str("  enable_swagger: true\n");
    yaml.push_str("  enable_static_files: true\n\n");

    yaml.push_str("logging:\n");
    yaml.push_str("  level: \"info\"\n");
    yaml.push_str("  format: \"text\"\n\n");

    yaml.push_str("metrics:\n");
    yaml.push_str("  enabled: true\n");

    // Write to file
    fs::write(output, yaml).context("Failed to write configuration file")?;

    Ok(())
}

/// OmniTAK configuration structure
#[derive(Debug, Default)]
struct OmniTakConfig {
    servers: Vec<ServerInfo>,
    p12_files: Vec<PathBuf>,
    client_cert: Option<PathBuf>,
    client_key: Option<PathBuf>,
    ca_cert: Option<PathBuf>,
}

/// Server information
#[derive(Debug, Clone)]
struct ServerInfo {
    name: String,
    address: String,
    port: u16,
    protocol: String,
}
