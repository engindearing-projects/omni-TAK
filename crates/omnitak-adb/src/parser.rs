//! Parsing utilities for ADB output and ATAK configuration

use anyhow::Result;
use std::collections::HashMap;
use tracing::debug;

use crate::DeviceInfo;

/// Parse ADB device list output
pub fn parse_device_list(output: &str) -> Result<Vec<DeviceInfo>> {
    let mut devices = Vec::new();

    for line in output.lines().skip(1) {
        // Skip "List of devices attached" header
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse line format: "serial    device product:... model:... device:... transport_id:..."
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let serial = parts[0].to_string();
        let state = parts[1].to_string();

        // Parse optional attributes
        let mut model = None;
        let mut product = None;
        let mut transport_id = None;

        for part in &parts[2..] {
            if let Some((key, value)) = part.split_once(':') {
                match key {
                    "model" => model = Some(value.to_string()),
                    "product" => product = Some(value.to_string()),
                    "transport_id" => transport_id = Some(value.to_string()),
                    _ => {}
                }
            }
        }

        devices.push(DeviceInfo {
            serial,
            state,
            model,
            product,
            transport_id,
        });
    }

    Ok(devices)
}

/// Parse ATAK preferences XML for TAK server configuration
pub fn parse_tak_preferences(content: &str) -> HashMap<String, String> {
    let mut config = HashMap::new();

    debug!("Parsing TAK preferences XML...");

    for line in content.lines() {
        // Look for server address
        if line.contains("caLocation") || line.contains("serverConnectString") {
            if let Some(value) = extract_xml_string_value(line) {
                debug!("Found server reference: {}", value);

                // Parse server:port format
                if let Some((host, port)) = parse_server_address(&value) {
                    config.insert("host".to_string(), host);
                    config.insert("port".to_string(), port.to_string());
                    config.insert(
                        "address".to_string(),
                        format!("{}:{}", config["host"], port),
                    );
                }
            }
        }

        // Look for certificate password
        if line.contains("certificatePassword") || line.contains("clientPassword") {
            if let Some(value) = extract_xml_string_value(line) {
                config.insert("cert_password".to_string(), value);
            }
        }

        // Look for server description/name
        if line.contains("serverDescription") || line.contains("connectString") {
            if let Some(value) = extract_xml_string_value(line) {
                if !value.contains(':') {
                    // Not an address, probably a name
                    config.insert("name".to_string(), value);
                }
            }
        }
    }

    // Set default name if not found
    if !config.contains_key("name") {
        config.insert("name".to_string(), "tak-server-from-device".to_string());
    }

    config
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

    // Remove any protocol prefixes
    let addr = addr
        .trim_start_matches("ssl://")
        .trim_start_matches("tcp://")
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    if let Some(colon) = addr.rfind(':') {
        let host = addr[..colon].to_string();
        let port = addr[colon + 1..].parse().unwrap_or(8089);
        Some((host, port))
    } else {
        // Default port 8089 for TLS
        Some((addr.to_string(), 8089))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_server_address() {
        assert_eq!(
            parse_server_address("example.com:8089"),
            Some(("example.com".to_string(), 8089))
        );
        assert_eq!(
            parse_server_address("ssl://example.com:8089"),
            Some(("example.com".to_string(), 8089))
        );
        assert_eq!(
            parse_server_address("example.com"),
            Some(("example.com".to_string(), 8089))
        );
    }

    #[test]
    fn test_extract_xml_value() {
        let xml = r#"<string name="test">value123</string>"#;
        assert_eq!(extract_xml_string_value(xml), Some("value123".to_string()));
    }

    #[test]
    fn test_parse_device_list() {
        let output = r#"List of devices attached
abc123    device product:model model:Phone transport_id:1
"#;
        let devices = parse_device_list(output).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "abc123");
        assert_eq!(devices[0].state, "device");
    }
}
