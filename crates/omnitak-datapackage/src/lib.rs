//! TAK Data Package support for OmniTAK
//!
//! This crate provides functionality for creating, reading, and managing
//! TAK Data Packages (.dpk/.zip files) which are used for:
//! - Mission planning data distribution
//! - Device staging and configuration
//! - Cross-device data sharing between ATAK, WinTAK, and OmniTAK

pub mod builder;
pub mod content;
pub mod error;
pub mod manifest;
pub mod reader;

pub use builder::DataPackageBuilder;
pub use content::{ContentType, PackageContent, PackageSummary};
pub use error::{DataPackageError, Result};
pub use manifest::{Manifest, ManifestContent, ManifestParameter};
pub use reader::DataPackageReader;

/// TAK Data Package version
pub const MANIFEST_VERSION: &str = "2";

/// Standard manifest path within the package
pub const MANIFEST_PATH: &str = "MANIFEST/manifest.xml";

/// Common file types found in TAK Data Packages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakFileType {
    /// CoT event (.cot)
    CotEvent,
    /// KML/KMZ map overlay
    KmlOverlay,
    /// Map tiles (MBTiles)
    MapTiles,
    /// Configuration file
    Config,
    /// Certificate file
    Certificate,
    /// Preference file
    Preferences,
    /// Image/attachment
    Attachment,
    /// Unknown file type
    Unknown,
}

impl TakFileType {
    /// Determine file type from extension
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "cot" | "xml" => Self::CotEvent,
            "kml" | "kmz" => Self::KmlOverlay,
            "mbtiles" | "sqlite" => Self::MapTiles,
            "pref" | "json" | "yaml" | "yml" => Self::Config,
            "p12" | "pem" | "crt" | "cer" | "key" => Self::Certificate,
            "jpg" | "jpeg" | "png" | "gif" | "bmp" => Self::Attachment,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_detection() {
        assert_eq!(TakFileType::from_extension("cot"), TakFileType::CotEvent);
        assert_eq!(TakFileType::from_extension("KML"), TakFileType::KmlOverlay);
        assert_eq!(TakFileType::from_extension("p12"), TakFileType::Certificate);
        assert_eq!(TakFileType::from_extension("unknown"), TakFileType::Unknown);
    }
}
