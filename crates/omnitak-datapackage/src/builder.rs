//! TAK Data Package builder for creating .dpk/.zip files

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::content::{ContentType, PackageContent, PackageSummary};
use crate::error::{DataPackageError, Result};
use crate::manifest::Manifest;
use crate::MANIFEST_PATH;

/// Builder for creating TAK Data Packages
pub struct DataPackageBuilder {
    /// Package manifest
    manifest: Manifest,
    /// Content items to include
    contents: Vec<PackageContent>,
    /// In-memory content data
    content_data: Vec<(String, Vec<u8>)>,
    /// Maximum package size (default 500MB)
    max_size: u64,
}

impl DataPackageBuilder {
    /// Create a new package builder
    pub fn new(name: &str) -> Self {
        let uid = uuid::Uuid::new_v4().to_string();
        Self {
            manifest: Manifest::new(&uid, name),
            contents: Vec::new(),
            content_data: Vec::new(),
            max_size: 500 * 1024 * 1024, // 500MB default
        }
    }

    /// Create a builder with a specific UID
    pub fn with_uid(uid: &str, name: &str) -> Self {
        Self {
            manifest: Manifest::new(uid, name),
            contents: Vec::new(),
            content_data: Vec::new(),
            max_size: 500 * 1024 * 1024,
        }
    }

    /// Set maximum package size
    pub fn max_size(mut self, size: u64) -> Self {
        self.max_size = size;
        self
    }

    /// Set whether package should be deleted after receipt
    pub fn on_receive_delete(mut self, delete: bool) -> Self {
        self.manifest
            .set_parameter("onReceiveDelete", if delete { "true" } else { "false" });
        self
    }

    /// Add a configuration parameter
    pub fn add_parameter(mut self, name: &str, value: &str) -> Self {
        self.manifest.set_parameter(name, value);
        self
    }

    /// Add a file from disk
    pub fn add_file<P: AsRef<Path>>(mut self, path: P, zip_entry: &str) -> Result<Self> {
        let path_ref = path.as_ref();
        if !path_ref.exists() {
            return Err(DataPackageError::MissingFile(
                path_ref.display().to_string(),
            ));
        }

        // Security check: prevent path traversal
        if zip_entry.contains("..") {
            return Err(DataPackageError::PathTraversal(zip_entry.to_string()));
        }

        let content = PackageContent::from_path(path_ref, zip_entry);
        self.contents.push(content);
        self.manifest.add_content(zip_entry, false);

        Ok(self)
    }

    /// Add multiple files from a directory
    pub fn add_directory<P: AsRef<Path>>(mut self, dir_path: P, prefix: &str) -> Result<Self> {
        let dir = dir_path.as_ref();
        if !dir.is_dir() {
            return Err(DataPackageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Directory not found: {}", dir.display()),
            )));
        }

        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let rel_path = entry
                .path()
                .strip_prefix(dir)
                .map_err(|_| DataPackageError::PathTraversal(entry.path().display().to_string()))?;

            let zip_entry = if prefix.is_empty() {
                rel_path.to_string_lossy().to_string()
            } else {
                format!("{}/{}", prefix, rel_path.display())
            };

            // Replace backslashes with forward slashes for ZIP compatibility
            let zip_entry = zip_entry.replace('\\', "/");

            let content = PackageContent::from_path(entry.path(), &zip_entry);
            self.contents.push(content);
            self.manifest.add_content(&zip_entry, false);
        }

        Ok(self)
    }

    /// Add in-memory data
    pub fn add_bytes(
        mut self,
        zip_entry: &str,
        data: Vec<u8>,
        content_type: ContentType,
    ) -> Result<Self> {
        // Security check: prevent path traversal
        if zip_entry.contains("..") {
            return Err(DataPackageError::PathTraversal(zip_entry.to_string()));
        }

        let content = PackageContent::from_bytes(zip_entry, &data, content_type);
        self.contents.push(content);
        self.content_data.push((zip_entry.to_string(), data));
        self.manifest.add_content(zip_entry, false);

        Ok(self)
    }

    /// Add a CoT event (XML string)
    pub fn add_cot_event(self, name: &str, xml: &str) -> Result<Self> {
        let zip_entry = format!("{}.cot", name);
        self.add_bytes(&zip_entry, xml.as_bytes().to_vec(), ContentType::CotEvent)
    }

    /// Get package summary
    pub fn summary(&self) -> PackageSummary {
        let mut summary = PackageSummary::default();
        for content in &self.contents {
            summary.add_content(content);
        }
        summary
    }

    /// Build the package and write to file
    pub fn build<P: AsRef<Path>>(self, output_path: P) -> Result<PathBuf> {
        let output = output_path.as_ref();

        // Check total size
        let total_size: u64 = self.contents.iter().map(|c| c.size).sum();
        if total_size > self.max_size {
            return Err(DataPackageError::PackageTooLarge {
                size: total_size,
                max_size: self.max_size,
            });
        }

        let file = File::create(output)?;
        let mut zip = ZipWriter::new(file);

        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        // Write manifest
        let manifest_xml = self.manifest.to_xml()?;
        zip.start_file(MANIFEST_PATH, options)?;
        zip.write_all(manifest_xml.as_bytes())?;

        // Write content files
        for content in &self.contents {
            zip.start_file(&content.path, options)?;

            if let Some(source_path) = &content.source_path {
                // Read from disk
                let mut file = File::open(source_path)?;
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)?;
                zip.write_all(&buffer)?;
            } else {
                // Find in-memory data
                if let Some((_, data)) = self
                    .content_data
                    .iter()
                    .find(|(path, _)| path == &content.path)
                {
                    zip.write_all(data)?;
                }
            }
        }

        zip.finish()?;

        tracing::info!(
            "Created TAK Data Package: {} ({} files, {} bytes)",
            output.display(),
            self.contents.len(),
            total_size
        );

        Ok(output.to_path_buf())
    }

    /// Build to in-memory buffer
    pub fn build_to_memory(self) -> Result<Vec<u8>> {
        let total_size: u64 = self.contents.iter().map(|c| c.size).sum();
        if total_size > self.max_size {
            return Err(DataPackageError::PackageTooLarge {
                size: total_size,
                max_size: self.max_size,
            });
        }

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);

            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o644);

            // Write manifest
            let manifest_xml = self.manifest.to_xml()?;
            zip.start_file(MANIFEST_PATH, options)?;
            zip.write_all(manifest_xml.as_bytes())?;

            // Write content files
            for content in &self.contents {
                zip.start_file(&content.path, options)?;

                if let Some(source_path) = &content.source_path {
                    let mut file = File::open(source_path)?;
                    let mut file_buffer = Vec::new();
                    file.read_to_end(&mut file_buffer)?;
                    zip.write_all(&file_buffer)?;
                } else {
                    if let Some((_, data)) = self
                        .content_data
                        .iter()
                        .find(|(path, _)| path == &content.path)
                    {
                        zip.write_all(data)?;
                    }
                }
            }

            zip.finish()?;
        }

        Ok(buffer.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_builder_creation() {
        let builder = DataPackageBuilder::new("test-package.zip");
        let summary = builder.summary();
        assert_eq!(summary.total_files, 0);
    }

    #[test]
    fn test_add_cot_event() {
        let cot_xml = r#"<event version="2.0" uid="test" type="a-f-G-E-V" />"#;
        let builder = DataPackageBuilder::new("test.zip")
            .add_cot_event("waypoint1", cot_xml)
            .unwrap();

        let summary = builder.summary();
        assert_eq!(summary.cot_events, 1);
    }

    #[test]
    fn test_path_traversal_prevention() {
        let builder = DataPackageBuilder::new("test.zip");
        let result = builder.add_bytes("../etc/passwd", vec![1, 2, 3], ContentType::Unknown);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_to_memory() {
        let cot_xml = r#"<event version="2.0" uid="test" type="a-f-G-E-V" />"#;
        let data = DataPackageBuilder::new("test.zip")
            .add_cot_event("event1", cot_xml)
            .unwrap()
            .build_to_memory()
            .unwrap();

        assert!(!data.is_empty());
        // Verify it's a valid ZIP (starts with PK)
        assert_eq!(&data[0..2], b"PK");
    }
}
