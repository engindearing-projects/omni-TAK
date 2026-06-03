//! TAK Data Package manifest parsing and generation
//!
//! Implements the MissionPackageManifest v2 format used by ATAK/WinTAK

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

use crate::error::{DataPackageError, Result};
use crate::MANIFEST_VERSION;

/// TAK Data Package Manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest version (typically "2")
    pub version: String,
    /// Configuration parameters
    pub configuration: Vec<ManifestParameter>,
    /// Package contents
    pub contents: Vec<ManifestContent>,
}

/// Configuration parameter in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestParameter {
    /// Parameter name
    pub name: String,
    /// Parameter value
    pub value: String,
}

/// Content entry in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestContent {
    /// Whether to ignore this content
    pub ignore: bool,
    /// Path within the ZIP archive
    pub zip_entry: String,
}

impl Manifest {
    /// Create a new manifest with default configuration
    pub fn new(uid: &str, name: &str) -> Self {
        Self {
            version: MANIFEST_VERSION.to_string(),
            configuration: vec![
                ManifestParameter {
                    name: "uid".to_string(),
                    value: uid.to_string(),
                },
                ManifestParameter {
                    name: "name".to_string(),
                    value: name.to_string(),
                },
                ManifestParameter {
                    name: "onReceiveDelete".to_string(),
                    value: "false".to_string(),
                },
            ],
            contents: Vec::new(),
        }
    }

    /// Get a configuration parameter by name
    pub fn get_parameter(&self, name: &str) -> Option<&str> {
        self.configuration
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.as_str())
    }

    /// Set a configuration parameter (creates if not exists)
    pub fn set_parameter(&mut self, name: &str, value: &str) {
        if let Some(param) = self.configuration.iter_mut().find(|p| p.name == name) {
            param.value = value.to_string();
        } else {
            self.configuration.push(ManifestParameter {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
    }

    /// Add content to the manifest
    pub fn add_content(&mut self, zip_entry: &str, ignore: bool) {
        self.contents.push(ManifestContent {
            ignore,
            zip_entry: zip_entry.to_string(),
        });
    }

    /// Get package UID
    pub fn uid(&self) -> Option<&str> {
        self.get_parameter("uid")
    }

    /// Get package name
    pub fn name(&self) -> Option<&str> {
        self.get_parameter("name")
    }

    /// Check if package should be deleted after receipt
    pub fn on_receive_delete(&self) -> bool {
        self.get_parameter("onReceiveDelete")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// Parse manifest from XML string
    pub fn from_xml(xml: &str) -> Result<Self> {
        Self::parse_from_reader(xml.as_bytes())
    }

    /// Parse manifest from reader
    pub fn parse_from_reader<R: BufRead>(reader: R) -> Result<Self> {
        let mut xml_reader = Reader::from_reader(reader);
        xml_reader.config_mut().trim_text(true);

        let mut version = MANIFEST_VERSION.to_string();
        let mut configuration = Vec::new();
        let mut contents = Vec::new();
        let mut buf = Vec::new();

        let mut in_configuration = false;
        let mut in_contents = false;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"MissionPackageManifest" => {
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| {
                                DataPackageError::InvalidManifest(format!(
                                    "Invalid attribute: {}",
                                    e
                                ))
                            })?;
                            if attr.key.as_ref() == b"version" {
                                version = String::from_utf8(attr.value.to_vec())?;
                            }
                        }
                    }
                    b"Configuration" => in_configuration = true,
                    b"Contents" => in_contents = true,
                    _ => {}
                },
                Ok(Event::Empty(ref e)) => {
                    if e.name().as_ref() == b"Parameter" && in_configuration {
                        let mut name = String::new();
                        let mut value = String::new();
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| {
                                DataPackageError::InvalidManifest(format!(
                                    "Invalid attribute: {}",
                                    e
                                ))
                            })?;
                            match attr.key.as_ref() {
                                b"name" => name = String::from_utf8(attr.value.to_vec())?,
                                b"value" => value = String::from_utf8(attr.value.to_vec())?,
                                _ => {}
                            }
                        }
                        if !name.is_empty() {
                            configuration.push(ManifestParameter { name, value });
                        }
                    } else if e.name().as_ref() == b"Content" && in_contents {
                        let mut ignore = false;
                        let mut zip_entry = String::new();
                        for attr in e.attributes() {
                            let attr = attr.map_err(|e| {
                                DataPackageError::InvalidManifest(format!(
                                    "Invalid attribute: {}",
                                    e
                                ))
                            })?;
                            match attr.key.as_ref() {
                                b"ignore" => {
                                    let val = String::from_utf8(attr.value.to_vec())?;
                                    ignore = val.eq_ignore_ascii_case("true");
                                }
                                b"zipEntry" => {
                                    zip_entry = String::from_utf8(attr.value.to_vec())?;
                                }
                                _ => {}
                            }
                        }
                        if !zip_entry.is_empty() {
                            contents.push(ManifestContent { ignore, zip_entry });
                        }
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"Configuration" => in_configuration = false,
                    b"Contents" => in_contents = false,
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(DataPackageError::Xml(e)),
                _ => {}
            }
            buf.clear();
        }

        Ok(Self {
            version,
            configuration,
            contents,
        })
    }

    /// Serialize manifest to XML string
    pub fn to_xml(&self) -> Result<String> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        // XML declaration
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

        // Root element
        let mut root = BytesStart::new("MissionPackageManifest");
        root.push_attribute(("version", self.version.as_str()));
        writer.write_event(Event::Start(root))?;

        // Configuration section
        writer.write_event(Event::Start(BytesStart::new("Configuration")))?;
        for param in &self.configuration {
            let mut elem = BytesStart::new("Parameter");
            elem.push_attribute(("name", param.name.as_str()));
            elem.push_attribute(("value", param.value.as_str()));
            writer.write_event(Event::Empty(elem))?;
        }
        writer.write_event(Event::End(BytesEnd::new("Configuration")))?;

        // Contents section
        writer.write_event(Event::Start(BytesStart::new("Contents")))?;
        for content in &self.contents {
            let mut elem = BytesStart::new("Content");
            elem.push_attribute(("ignore", if content.ignore { "true" } else { "false" }));
            elem.push_attribute(("zipEntry", content.zip_entry.as_str()));
            writer.write_event(Event::Empty(elem))?;
        }
        writer.write_event(Event::End(BytesEnd::new("Contents")))?;

        // Close root
        writer.write_event(Event::End(BytesEnd::new("MissionPackageManifest")))?;

        let result = writer.into_inner();
        Ok(String::from_utf8(result)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_creation() {
        let manifest = Manifest::new("test-uid-123", "test-package.zip");
        assert_eq!(manifest.uid(), Some("test-uid-123"));
        assert_eq!(manifest.name(), Some("test-package.zip"));
        assert!(!manifest.on_receive_delete());
    }

    #[test]
    fn test_manifest_serialization() {
        let mut manifest = Manifest::new("pkg-001", "mission.zip");
        manifest.add_content("waypoints.cot", false);
        manifest.add_content("map.mbtiles", false);

        let xml = manifest.to_xml().unwrap();
        assert!(xml.contains("MissionPackageManifest"));
        assert!(xml.contains("version=\"2\""));
        assert!(xml.contains("waypoints.cot"));
        assert!(xml.contains("map.mbtiles"));
    }

    #[test]
    fn test_manifest_parsing() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<MissionPackageManifest version="2">
  <Configuration>
    <Parameter name="uid" value="test-123"/>
    <Parameter name="name" value="test.zip"/>
    <Parameter name="onReceiveDelete" value="true"/>
  </Configuration>
  <Contents>
    <Content ignore="false" zipEntry="data.cot"/>
    <Content ignore="true" zipEntry="temp.txt"/>
  </Contents>
</MissionPackageManifest>"#;

        let manifest = Manifest::from_xml(xml).unwrap();
        assert_eq!(manifest.version, "2");
        assert_eq!(manifest.uid(), Some("test-123"));
        assert_eq!(manifest.name(), Some("test.zip"));
        assert!(manifest.on_receive_delete());
        assert_eq!(manifest.contents.len(), 2);
        assert!(!manifest.contents[0].ignore);
        assert!(manifest.contents[1].ignore);
    }

    #[test]
    fn test_roundtrip() {
        let mut original = Manifest::new("roundtrip-test", "package.dpk");
        original.set_parameter("onReceiveDelete", "true");
        original.add_content("file1.cot", false);
        original.add_content("file2.kml", true);

        let xml = original.to_xml().unwrap();
        let parsed = Manifest::from_xml(&xml).unwrap();

        assert_eq!(parsed.uid(), original.uid());
        assert_eq!(parsed.name(), original.name());
        assert_eq!(parsed.on_receive_delete(), original.on_receive_delete());
        assert_eq!(parsed.contents.len(), original.contents.len());
    }
}
