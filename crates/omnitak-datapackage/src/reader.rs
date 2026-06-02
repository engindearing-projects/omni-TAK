//! TAK Data Package reader for importing .dpk/.zip files

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

use crate::content::{ContentType, PackageContent, PackageSummary};
use crate::error::{DataPackageError, Result};
use crate::manifest::Manifest;
use crate::{TakFileType, MANIFEST_PATH};

/// Reader for TAK Data Packages
pub struct DataPackageReader {
    /// Package manifest
    manifest: Manifest,
    /// Content metadata
    contents: Vec<PackageContent>,
    /// Source path of the package
    source_path: PathBuf,
}

impl DataPackageReader {
    /// Open a data package from file
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref)?;
        let mut archive = ZipArchive::new(file)?;

        // Read manifest
        let manifest = Self::read_manifest(&mut archive)?;

        // Build content metadata
        let mut contents = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            let name = entry.name().to_string();

            // Skip manifest directory
            if name.starts_with("MANIFEST/") {
                continue;
            }

            let file_type = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(TakFileType::from_extension)
                .unwrap_or(TakFileType::Unknown);

            let ignore = manifest
                .contents
                .iter()
                .find(|c| c.zip_entry == name)
                .map(|c| c.ignore)
                .unwrap_or(false);

            contents.push(PackageContent {
                path: name,
                content_type: ContentType::from(file_type),
                size: entry.size(),
                ignore,
                description: None,
                source_path: None,
            });
        }

        Ok(Self {
            manifest,
            contents,
            source_path: path_ref.to_path_buf(),
        })
    }

    /// Read manifest from archive
    fn read_manifest<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<Manifest> {
        let mut manifest_file = archive
            .by_name(MANIFEST_PATH)
            .map_err(|_| DataPackageError::MissingFile(MANIFEST_PATH.to_string()))?;

        let mut manifest_xml = String::new();
        manifest_file.read_to_string(&mut manifest_xml)?;

        Manifest::from_xml(&manifest_xml)
    }

    /// Get the package manifest
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Get package UID
    pub fn uid(&self) -> Option<&str> {
        self.manifest.uid()
    }

    /// Get package name
    pub fn name(&self) -> Option<&str> {
        self.manifest.name()
    }

    /// Get all content items
    pub fn contents(&self) -> &[PackageContent] {
        &self.contents
    }

    /// Get package summary
    pub fn summary(&self) -> PackageSummary {
        let mut summary = PackageSummary::default();
        for content in &self.contents {
            summary.add_content(content);
        }
        summary
    }

    /// Get content by type
    pub fn get_content_by_type(&self, content_type: ContentType) -> Vec<&PackageContent> {
        self.contents
            .iter()
            .filter(|c| c.content_type == content_type)
            .collect()
    }

    /// Read a specific file from the package
    pub fn read_file(&self, zip_entry: &str) -> Result<Vec<u8>> {
        let file = File::open(&self.source_path)?;
        let mut archive = ZipArchive::new(file)?;

        let mut entry = archive
            .by_name(zip_entry)
            .map_err(|_| DataPackageError::MissingFile(zip_entry.to_string()))?;

        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer)?;

        Ok(buffer)
    }

    /// Read a file as string
    pub fn read_file_string(&self, zip_entry: &str) -> Result<String> {
        let bytes = self.read_file(zip_entry)?;
        Ok(String::from_utf8(bytes)?)
    }

    /// Extract all files to a directory
    pub fn extract_all<P: AsRef<Path>>(&self, output_dir: P) -> Result<Vec<PathBuf>> {
        let output = output_dir.as_ref();
        std::fs::create_dir_all(output)?;

        let file = File::open(&self.source_path)?;
        let mut archive = ZipArchive::new(file)?;

        let mut extracted = Vec::new();

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();

            // Skip manifest
            if name.starts_with("MANIFEST/") {
                continue;
            }

            // Security: check for path traversal
            if name.contains("..") {
                return Err(DataPackageError::PathTraversal(name));
            }

            let output_path = output.join(&name);

            // Create parent directories
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Extract file
            let mut output_file = File::create(&output_path)?;
            std::io::copy(&mut entry, &mut output_file)?;

            extracted.push(output_path);
        }

        tracing::info!(
            "Extracted {} files to {}",
            extracted.len(),
            output.display()
        );

        Ok(extracted)
    }

    /// Extract only specific content types
    pub fn extract_by_type<P: AsRef<Path>>(
        &self,
        output_dir: P,
        content_type: ContentType,
    ) -> Result<Vec<PathBuf>> {
        let output = output_dir.as_ref();
        std::fs::create_dir_all(output)?;

        let mut extracted = Vec::new();

        for content in &self.contents {
            if content.content_type == content_type {
                let data = self.read_file(&content.path)?;

                // Get just the filename
                let filename = Path::new(&content.path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();

                let output_path = output.join(filename.as_ref());
                let mut file = File::create(&output_path)?;
                file.write_all(&data)?;

                extracted.push(output_path);
            }
        }

        Ok(extracted)
    }

    /// Get all CoT events from the package
    pub fn get_cot_events(&self) -> Result<Vec<String>> {
        let cot_contents = self.get_content_by_type(ContentType::CotEvent);
        let mut events = Vec::new();

        for content in cot_contents {
            let xml = self.read_file_string(&content.path)?;
            events.push(xml);
        }

        Ok(events)
    }

    /// Validate package integrity
    pub fn validate(&self) -> Result<()> {
        // Check manifest version
        if self.manifest.version != "2" {
            return Err(DataPackageError::UnsupportedVersion(
                self.manifest.version.clone(),
            ));
        }

        // Check that manifest contents match actual files
        let file = File::open(&self.source_path)?;
        let mut archive = ZipArchive::new(file)?;

        let mut actual_files: HashMap<String, bool> = HashMap::new();
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i)?;
            let name = entry.name().to_string();
            if !name.starts_with("MANIFEST/") {
                actual_files.insert(name, true);
            }
        }

        // Verify all manifest entries exist
        for content in &self.manifest.contents {
            if !actual_files.contains_key(&content.zip_entry) {
                return Err(DataPackageError::ValidationFailed(format!(
                    "Manifest entry '{}' not found in archive",
                    content.zip_entry
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::DataPackageBuilder;
    use tempfile::TempDir;

    #[test]
    fn test_read_created_package() {
        let temp_dir = TempDir::new().unwrap();
        let pkg_path = temp_dir.path().join("test.zip");

        // Create a package
        let cot_xml = r#"<event version="2.0" uid="test" type="a-f-G-E-V" />"#;
        DataPackageBuilder::with_uid("test-uid", "test.zip")
            .add_cot_event("waypoint1", cot_xml)
            .unwrap()
            .build(&pkg_path)
            .unwrap();

        // Read it back
        let reader = DataPackageReader::open(&pkg_path).unwrap();
        assert_eq!(reader.uid(), Some("test-uid"));
        assert_eq!(reader.name(), Some("test.zip"));
        assert_eq!(reader.contents().len(), 1);

        let summary = reader.summary();
        assert_eq!(summary.cot_events, 1);
    }

    #[test]
    fn test_extract_files() {
        let temp_dir = TempDir::new().unwrap();
        let pkg_path = temp_dir.path().join("test.zip");
        let extract_dir = temp_dir.path().join("extracted");

        // Create package with multiple files
        DataPackageBuilder::new("multi.zip")
            .add_cot_event("event1", "<event />")
            .unwrap()
            .add_cot_event("event2", "<event />")
            .unwrap()
            .build(&pkg_path)
            .unwrap();

        // Extract
        let reader = DataPackageReader::open(&pkg_path).unwrap();
        let extracted = reader.extract_all(&extract_dir).unwrap();

        assert_eq!(extracted.len(), 2);
        assert!(extract_dir.join("event1.cot").exists());
        assert!(extract_dir.join("event2.cot").exists());
    }

    #[test]
    fn test_validate_package() {
        let temp_dir = TempDir::new().unwrap();
        let pkg_path = temp_dir.path().join("valid.zip");

        DataPackageBuilder::new("valid.zip")
            .add_cot_event("test", "<event />")
            .unwrap()
            .build(&pkg_path)
            .unwrap();

        let reader = DataPackageReader::open(&pkg_path).unwrap();
        assert!(reader.validate().is_ok());
    }
}
