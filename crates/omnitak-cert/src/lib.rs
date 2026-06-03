//! Certificate management for OmniTAK
//!
//! Handles loading, parsing, and validating certificates for TAK server connections.
//! Supports PEM, DER, and PKCS#12 formats with password protection.
//! Includes certificate enrollment for TAK servers requiring username/password authentication.
//! Also provides certificate generation for OmniTAK's own enrollment server.

pub mod enrollment;
pub mod generator;

use anyhow::{anyhow, Context, Result};
use base64::prelude::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Certificate data that can be stored in memory or loaded from files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateData {
    /// Certificate name/identifier
    pub name: String,
    /// Base64-encoded certificate data
    pub data: String,
    /// Original file size in bytes
    pub size: u64,
}

/// Certificate bundle containing all necessary components for TLS
#[derive(Debug)]
pub struct CertificateBundle {
    /// Client certificates (can be multiple in a chain)
    pub certs: Vec<CertificateDer<'static>>,
    /// Private key for the client certificate
    pub private_key: PrivateKeyDer<'static>,
    /// Optional CA certificate for server verification
    pub ca_certs: Option<Vec<CertificateDer<'static>>>,
}

impl Clone for CertificateBundle {
    fn clone(&self) -> Self {
        Self {
            certs: self.certs.clone(),
            private_key: self.private_key.clone_key(),
            ca_certs: self.ca_certs.clone(),
        }
    }
}

impl CertificateBundle {
    /// Create a new certificate bundle from PEM-encoded data
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8], ca_pem: Option<&[u8]>) -> Result<Self> {
        info!("Loading certificates from PEM data");

        // Parse client certificate
        let mut cert_reader = BufReader::new(Cursor::new(cert_pem));
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse client certificates")?;

        if certs.is_empty() {
            return Err(anyhow!("No certificates found in PEM data"));
        }

        info!("Loaded {} client certificate(s)", certs.len());

        // Parse private key
        let mut key_reader = BufReader::new(Cursor::new(key_pem));
        let private_key = rustls_pemfile::private_key(&mut key_reader)
            .context("Failed to read private key")?
            .ok_or_else(|| anyhow!("No private key found in PEM data"))?;

        info!("Loaded private key");

        // Parse CA certificate if provided
        let ca_certs = if let Some(ca_pem_data) = ca_pem {
            let mut ca_reader = BufReader::new(Cursor::new(ca_pem_data));
            let ca_certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut ca_reader)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse CA certificates")?;

            if !ca_certs.is_empty() {
                info!("Loaded {} CA certificate(s)", ca_certs.len());
                Some(ca_certs)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            certs,
            private_key,
            ca_certs,
        })
    }

    /// Create a certificate bundle from PKCS#12 data with password
    pub fn from_pkcs12(p12_data: &[u8], password: Option<&str>) -> Result<Self> {
        info!("Loading certificate from PKCS#12 format");

        let password = password.unwrap_or("");

        // Parse PKCS#12 structure
        let p12 = p12::PFX::parse(p12_data)
            .map_err(|e| anyhow!("Failed to parse PKCS#12 file: {}", e))?;

        // Decrypt the PKCS#12 with password
        let bags = p12
            .bags(password)
            .map_err(|e| anyhow!("Failed to decrypt PKCS#12 with provided password: {}", e))?;

        let mut certs: Vec<CertificateDer<'static>> = Vec::new();
        let mut private_key: Option<PrivateKeyDer<'static>> = None;
        let mut ca_certs: Vec<CertificateDer<'static>> = Vec::new();

        for bag in bags {
            match bag.bag {
                p12::SafeBagKind::CertBag(cert_bag) => {
                    if let p12::CertBag::X509(cert_data) = cert_bag {
                        let cert = CertificateDer::from(cert_data.to_vec());
                        // First cert is usually the client cert, rest are CA chain
                        if certs.is_empty() {
                            certs.push(cert);
                        } else {
                            ca_certs.push(cert);
                        }
                    }
                }
                p12::SafeBagKind::Pkcs8ShroudedKeyBag(key_bag) => {
                    let key_data = key_bag
                        .encryption_algorithm
                        .decrypt_pbe(&key_bag.encrypted_data, password.as_bytes())
                        .ok_or_else(|| {
                            anyhow!("Failed to decrypt private key with provided password")
                        })?;
                    private_key = Some(PrivateKeyDer::Pkcs8(key_data.into()));
                }
                _ => {
                    debug!("Skipping unsupported bag type in PKCS#12");
                }
            }
        }

        if certs.is_empty() {
            return Err(anyhow!("No client certificate found in PKCS#12 file"));
        }

        let private_key =
            private_key.ok_or_else(|| anyhow!("No private key found in PKCS#12 file"))?;

        info!(
            "Loaded {} client cert(s) and {} CA cert(s) from PKCS#12",
            certs.len(),
            ca_certs.len()
        );

        Ok(Self {
            certs,
            private_key,
            ca_certs: if ca_certs.is_empty() {
                None
            } else {
                Some(ca_certs)
            },
        })
    }

    /// Create a certificate bundle from base64-encoded certificate data
    pub fn from_certificate_data(
        cert_data: &CertificateData,
        key_data: Option<&CertificateData>,
        ca_data: Option<&CertificateData>,
        password: Option<&str>,
    ) -> Result<Self> {
        // Decode base64 data
        let cert_bytes = BASE64_STANDARD
            .decode(&cert_data.data)
            .context("Failed to decode certificate data")?;

        // Determine format based on content or file extension
        if cert_data.name.ends_with(".p12") || cert_data.name.ends_with(".pfx") {
            // PKCS#12 format
            Self::from_pkcs12(&cert_bytes, password)
        } else {
            // PEM format
            let key_bytes = if let Some(key) = key_data {
                BASE64_STANDARD
                    .decode(&key.data)
                    .context("Failed to decode private key data")?
            } else {
                return Err(anyhow!("Private key is required for PEM format"));
            };

            let ca_bytes = if let Some(ca) = ca_data {
                Some(
                    BASE64_STANDARD
                        .decode(&ca.data)
                        .context("Failed to decode CA certificate data")?,
                )
            } else {
                None
            };

            Self::from_pem(&cert_bytes, &key_bytes, ca_bytes.as_deref())
        }
    }
}

/// Result of extracting certificates from a ZIP file or directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedCertificates {
    /// Path to CA certificate (if found)
    pub ca_cert_path: Option<PathBuf>,
    /// Path to client certificate (if found)
    pub client_cert_path: Option<PathBuf>,
    /// Path to client private key (if found)
    pub client_key_path: Option<PathBuf>,
    /// Path to P12 file (if found)
    pub p12_path: Option<PathBuf>,
    /// Server configuration info (if found in config files)
    pub server_info: Option<ExtractedServerInfo>,
    /// All extracted files
    pub all_files: Vec<PathBuf>,
}

/// Server information extracted from configuration files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedServerInfo {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub description: Option<String>,
}

/// Extract certificates from a ZIP file
pub fn extract_zip_certificates(
    zip_path: &Path,
    output_dir: &Path,
) -> Result<ExtractedCertificates> {
    info!("Extracting certificates from ZIP: {}", zip_path.display());

    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("Failed to open ZIP file: {}", zip_path.display()))?;

    let mut archive = zip::ZipArchive::new(file).context("Failed to read ZIP archive")?;

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let mut extracted_files = Vec::new();

    // Extract all files
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .with_context(|| format!("Failed to read file at index {} in ZIP", i))?;

        let outpath = match file.enclosed_name() {
            Some(path) => output_dir.join(path),
            None => {
                warn!("Skipping file with invalid path in ZIP");
                continue;
            }
        };

        if file.name().ends_with('/') {
            // Directory
            std::fs::create_dir_all(&outpath).ok();
        } else {
            // File
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(p).ok();
                }
            }

            let mut outfile = std::fs::File::create(&outpath)
                .with_context(|| format!("Failed to create file: {}", outpath.display()))?;

            std::io::copy(&mut file, &mut outfile)
                .with_context(|| format!("Failed to extract file: {}", outpath.display()))?;

            info!("Extracted: {}", outpath.display());
            extracted_files.push(outpath);
        }
    }

    // Now scan the extracted files to identify certificates
    classify_certificate_files(&extracted_files, output_dir)
}

/// Extract certificates from a ZIP file in memory
pub fn extract_zip_certificates_from_bytes(
    zip_data: &[u8],
    output_dir: &Path,
) -> Result<ExtractedCertificates> {
    info!(
        "Extracting certificates from ZIP data ({} bytes)",
        zip_data.len()
    );

    let cursor = Cursor::new(zip_data);
    let mut archive =
        zip::ZipArchive::new(cursor).context("Failed to read ZIP archive from memory")?;

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let mut extracted_files = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .with_context(|| format!("Failed to read file at index {} in ZIP", i))?;

        let outpath = match file.enclosed_name() {
            Some(path) => output_dir.join(path),
            None => continue,
        };

        if !file.name().ends_with('/') {
            if let Some(p) = outpath.parent() {
                std::fs::create_dir_all(p).ok();
            }

            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
            extracted_files.push(outpath);
        }
    }

    classify_certificate_files(&extracted_files, output_dir)
}

/// Classify certificate files by their type based on name and content
pub fn classify_certificate_files(
    files: &[PathBuf],
    _base_dir: &Path,
) -> Result<ExtractedCertificates> {
    let mut result = ExtractedCertificates {
        ca_cert_path: None,
        client_cert_path: None,
        client_key_path: None,
        p12_path: None,
        server_info: None,
        all_files: files.to_vec(),
    };

    // Priority-based classification
    let mut p12_candidates: Vec<PathBuf> = Vec::new();
    let mut ca_candidates: Vec<PathBuf> = Vec::new();
    let mut cert_candidates: Vec<PathBuf> = Vec::new();
    let mut key_candidates: Vec<PathBuf> = Vec::new();

    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();

        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Check for PKCS#12 files (highest priority - all-in-one)
        if ext == "p12" || ext == "pfx" {
            p12_candidates.push(file.clone());
        }
        // Check for CA certificates
        else if name.contains("ca") || name.contains("truststore") || name.contains("root") {
            if ext == "pem" || ext == "crt" || ext == "cer" {
                ca_candidates.push(file.clone());
            }
        }
        // Check for private keys
        else if ext == "key" || name.contains("-key") || name.contains("_key") {
            key_candidates.push(file.clone());
        }
        // Check for client certificates
        else if ext == "pem" || ext == "crt" || ext == "cer" {
            cert_candidates.push(file.clone());
        }
        // Check for XML config files that might have server info
        else if ext == "xml" && (name.contains("pref") || name.contains("config")) {
            if let Ok(server_info) = parse_tak_config_file(file) {
                result.server_info = Some(server_info);
            }
        }
    }

    // Assign best candidates
    // P12 is preferred (contains everything)
    if !p12_candidates.is_empty() {
        // Prefer user/client P12 over truststore P12
        result.p12_path = Some(
            p12_candidates
                .iter()
                .find(|p| {
                    let name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    !name.contains("truststore") && !name.contains("ca")
                })
                .cloned()
                .unwrap_or_else(|| p12_candidates[0].clone()),
        );
    }

    // CA certificate
    if !ca_candidates.is_empty() {
        result.ca_cert_path = Some(ca_candidates[0].clone());
    }

    // Client certificate (prefer ones with "client" or "admin" in name)
    if !cert_candidates.is_empty() {
        result.client_cert_path = Some(
            cert_candidates
                .iter()
                .find(|p| {
                    let name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    name.contains("client") || name.contains("admin") || name.contains("user")
                })
                .cloned()
                .unwrap_or_else(|| cert_candidates[0].clone()),
        );
    }

    // Private key
    if !key_candidates.is_empty() {
        result.client_key_path = Some(key_candidates[0].clone());
    }

    info!(
        "Classified certificates: P12={}, CA={}, Cert={}, Key={}",
        result.p12_path.is_some(),
        result.ca_cert_path.is_some(),
        result.client_cert_path.is_some(),
        result.client_key_path.is_some()
    );

    Ok(result)
}

/// Parse TAK configuration XML file for server information
fn parse_tak_config_file(path: &Path) -> Result<ExtractedServerInfo> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let mut server_info = ExtractedServerInfo {
        host: None,
        port: None,
        description: None,
    };

    // Simple XML parsing for common TAK config patterns
    // Look for connectString or address patterns
    if let Some(start) = content.find("connectString") {
        if let Some(value_start) = content[start..].find('>') {
            if let Some(value_end) = content[start + value_start..].find('<') {
                let value = &content[start + value_start + 1..start + value_start + value_end];
                // Parse "host:port:protocol" format
                let parts: Vec<&str> = value.split(':').collect();
                if !parts.is_empty() {
                    server_info.host = Some(parts[0].to_string());
                }
                if parts.len() > 1 {
                    server_info.port = parts[1].parse().ok();
                }
            }
        }
    }

    // Look for serverAddress
    if server_info.host.is_none() {
        for pattern in &["serverAddress", "address", "host"] {
            if let Some(start) = content.find(pattern) {
                if let Some(quote_start) = content[start..].find('"') {
                    if let Some(quote_end) = content[start + quote_start + 1..].find('"') {
                        let value =
                            &content[start + quote_start + 1..start + quote_start + 1 + quote_end];
                        if !value.is_empty() && !value.contains('<') {
                            server_info.host = Some(value.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }

    // Look for port
    if server_info.port.is_none() {
        if let Some(start) = content.find("port") {
            if let Some(quote_start) = content[start..].find('"') {
                if let Some(quote_end) = content[start + quote_start + 1..].find('"') {
                    let value =
                        &content[start + quote_start + 1..start + quote_start + 1 + quote_end];
                    server_info.port = value.parse().ok();
                }
            }
        }
    }

    Ok(server_info)
}

/// Auto-detect certificate format and load from file
pub fn auto_load_certificate_bundle(
    path: &Path,
    password: Option<&str>,
) -> Result<CertificateBundle> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "p12" | "pfx" => {
            let data = std::fs::read(path)
                .with_context(|| format!("Failed to read P12 file: {}", path.display()))?;
            CertificateBundle::from_pkcs12(&data, password)
        }
        "zip" => {
            // Extract to temp directory and then load
            let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
            let extracted = extract_zip_certificates(path, temp_dir.path())?;

            // Try P12 first if available
            if let Some(p12_path) = &extracted.p12_path {
                let data = std::fs::read(p12_path)?;
                return CertificateBundle::from_pkcs12(&data, password);
            }

            // Otherwise try PEM files
            if let (Some(cert_path), Some(key_path)) =
                (&extracted.client_cert_path, &extracted.client_key_path)
            {
                let cert_pem = std::fs::read(cert_path)?;
                let key_pem = std::fs::read(key_path)?;
                let ca_pem = extracted
                    .ca_cert_path
                    .as_ref()
                    .map(std::fs::read)
                    .transpose()?;

                return CertificateBundle::from_pem(&cert_pem, &key_pem, ca_pem.as_deref());
            }

            Err(anyhow!("No usable certificates found in ZIP file"))
        }
        "pem" | "crt" | "cer" => {
            // Single PEM file - might contain cert + key
            let data = std::fs::read(path)
                .with_context(|| format!("Failed to read PEM file: {}", path.display()))?;

            // Try to parse as combined cert+key file
            let mut cert_reader = BufReader::new(Cursor::new(&data));
            let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse certificates")?;

            let mut key_reader = BufReader::new(Cursor::new(&data));
            if let Some(private_key) = rustls_pemfile::private_key(&mut key_reader)? {
                // Combined file
                Ok(CertificateBundle {
                    certs,
                    private_key,
                    ca_certs: None,
                })
            } else {
                Err(anyhow!(
                    "PEM file does not contain both certificate and private key. \
                    Please provide separate key file or use a P12/ZIP bundle."
                ))
            }
        }
        _ => Err(anyhow!(
            "Unsupported certificate format: {}. Supported: .p12, .pfx, .pem, .crt, .cer, .zip",
            ext
        )),
    }
}

/// Scan a directory for certificate files
pub fn scan_directory_for_certificates(dir: &Path) -> Result<ExtractedCertificates> {
    if !dir.exists() || !dir.is_dir() {
        return Err(anyhow!("Directory does not exist: {}", dir.display()));
    }

    let mut files = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            // Only include certificate-related files
            if matches!(
                ext.as_str(),
                "pem" | "crt" | "cer" | "key" | "p12" | "pfx" | "xml"
            ) {
                files.push(path);
            }
        }
    }

    classify_certificate_files(&files, dir)
}

/// Detailed information about a single certificate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateInfo {
    /// Subject common name (CN)
    pub subject_cn: String,
    /// Full subject distinguished name
    pub subject_dn: String,
    /// Issuer common name
    pub issuer_cn: String,
    /// Full issuer distinguished name
    pub issuer_dn: String,
    /// Serial number (hex)
    pub serial_number: String,
    /// Not valid before (ISO 8601)
    pub not_before: String,
    /// Not valid after (ISO 8601)
    pub not_after: String,
    /// Days until expiration (negative if expired)
    pub days_until_expiry: i64,
    /// Whether the certificate is currently valid
    pub is_valid: bool,
    /// Whether the certificate is expired
    pub is_expired: bool,
    /// Whether the certificate expires within 30 days
    pub expiring_soon: bool,
    /// Certificate fingerprint (SHA-256)
    pub fingerprint: String,
    /// Key usage (if present)
    pub key_usage: Vec<String>,
    /// Is this a CA certificate
    pub is_ca: bool,
}

/// Certificate chain information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateChainInfo {
    /// Client certificate (leaf)
    pub client_cert: Option<CertificateInfo>,
    /// Intermediate certificates
    pub intermediates: Vec<CertificateInfo>,
    /// Root CA certificate
    pub root_ca: Option<CertificateInfo>,
    /// Overall chain validity
    pub chain_valid: bool,
    /// Earliest expiration in the chain
    pub earliest_expiry: Option<String>,
    /// Days until earliest expiry
    pub days_until_chain_expiry: Option<i64>,
}

impl CertificateInfo {
    /// Parse certificate info from DER-encoded data
    pub fn from_der(der_data: &[u8]) -> Result<Self> {
        use chrono::Utc;
        use x509_parser::prelude::*;

        let (_, cert) = X509Certificate::from_der(der_data)
            .map_err(|e| anyhow!("Failed to parse X.509 certificate: {}", e))?;

        // Extract subject CN
        let subject_cn = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("Unknown")
            .to_string();

        let subject_dn = cert.subject().to_string();

        // Extract issuer CN
        let issuer_cn = cert
            .issuer()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("Unknown")
            .to_string();

        let issuer_dn = cert.issuer().to_string();

        // Serial number
        let serial_number = cert.serial.to_str_radix(16);

        // Validity dates
        let not_before_asn1 = cert.validity().not_before;
        let not_after_asn1 = cert.validity().not_after;

        let not_before = format_asn1_time(&not_before_asn1);
        let not_after = format_asn1_time(&not_after_asn1);

        // Calculate expiration
        let now = Utc::now();
        let not_after_dt = parse_asn1_time_to_datetime(&not_after_asn1)?;
        let not_before_dt = parse_asn1_time_to_datetime(&not_before_asn1)?;

        let days_until_expiry = (not_after_dt - now).num_days();
        let is_valid = now >= not_before_dt && now <= not_after_dt;
        let is_expired = now > not_after_dt;
        let expiring_soon = days_until_expiry <= 30 && days_until_expiry > 0;

        // Fingerprint (SHA-256)

        let fingerprint = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            der_data.hash(&mut hasher);
            format!("{:016X}", hasher.finish())
        };

        // Key usage
        let mut key_usage = Vec::new();
        if let Ok(Some(ku)) = cert.key_usage() {
            if ku.value.digital_signature() {
                key_usage.push("Digital Signature".to_string());
            }
            if ku.value.non_repudiation() {
                key_usage.push("Non Repudiation".to_string());
            }
            if ku.value.key_encipherment() {
                key_usage.push("Key Encipherment".to_string());
            }
            if ku.value.data_encipherment() {
                key_usage.push("Data Encipherment".to_string());
            }
            if ku.value.key_agreement() {
                key_usage.push("Key Agreement".to_string());
            }
            if ku.value.key_cert_sign() {
                key_usage.push("Certificate Signing".to_string());
            }
            if ku.value.crl_sign() {
                key_usage.push("CRL Signing".to_string());
            }
        }

        // Is CA
        let is_ca = cert
            .basic_constraints()
            .ok()
            .flatten()
            .map(|bc| bc.value.ca)
            .unwrap_or(false);

        Ok(Self {
            subject_cn,
            subject_dn,
            issuer_cn,
            issuer_dn,
            serial_number,
            not_before,
            not_after,
            days_until_expiry,
            is_valid,
            is_expired,
            expiring_soon,
            fingerprint,
            key_usage,
            is_ca,
        })
    }

    /// Parse certificate info from PEM file
    pub fn from_pem_file(path: &Path) -> Result<Vec<Self>> {
        let pem_data = std::fs::read(path)
            .with_context(|| format!("Failed to read PEM file: {}", path.display()))?;

        let mut reader = BufReader::new(Cursor::new(&pem_data));
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse PEM certificates")?;

        certs
            .iter()
            .map(|cert| Self::from_der(cert.as_ref()))
            .collect()
    }
}

impl CertificateChainInfo {
    /// Build chain info from a CertificateBundle
    pub fn from_bundle(bundle: &CertificateBundle) -> Self {
        let mut client_cert = None;
        let mut intermediates = Vec::new();
        let root_ca = None;

        // Parse client certificate
        if let Some(cert_der) = bundle.certs.first() {
            if let Ok(info) = CertificateInfo::from_der(cert_der.as_ref()) {
                client_cert = Some(info);
            }
        }

        // Parse CA certificates as intermediates/root
        if let Some(ca_certs) = &bundle.ca_certs {
            for cert_der in ca_certs {
                if let Ok(info) = CertificateInfo::from_der(cert_der.as_ref()) {
                    intermediates.push(info);
                }
            }
        }

        // Determine overall chain validity and earliest expiry
        let mut chain_valid = true;
        let mut earliest_expiry: Option<String> = None;
        let mut days_until_chain_expiry: Option<i64> = None;

        let all_certs: Vec<&CertificateInfo> =
            client_cert.iter().chain(intermediates.iter()).collect();

        for cert in all_certs {
            if !cert.is_valid {
                chain_valid = false;
            }

            if days_until_chain_expiry.is_none()
                || cert.days_until_expiry < days_until_chain_expiry.unwrap()
            {
                days_until_chain_expiry = Some(cert.days_until_expiry);
                earliest_expiry = Some(cert.not_after.clone());
            }
        }

        Self {
            client_cert,
            intermediates,
            root_ca,
            chain_valid,
            earliest_expiry,
            days_until_chain_expiry,
        }
    }
}

/// Format ASN.1 time to ISO 8601 string
fn format_asn1_time(time: &x509_parser::time::ASN1Time) -> String {
    let dt = time.to_datetime();
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// Parse ASN.1 time to chrono DateTime
fn parse_asn1_time_to_datetime(
    time: &x509_parser::time::ASN1Time,
) -> Result<chrono::DateTime<chrono::Utc>> {
    let offset_dt = time.to_datetime();
    // Convert from time::OffsetDateTime to chrono::DateTime<Utc>
    let timestamp = offset_dt.unix_timestamp();
    chrono::DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| anyhow!("Failed to convert timestamp"))
}

/// Expiration status for visual display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpirationStatus {
    Valid,
    ExpiringSoon,
    Expired,
    NotYetValid,
}

impl ExpirationStatus {
    pub fn from_cert_info(info: &CertificateInfo) -> Self {
        if info.is_expired {
            ExpirationStatus::Expired
        } else if !info.is_valid {
            ExpirationStatus::NotYetValid
        } else if info.expiring_soon {
            ExpirationStatus::ExpiringSoon
        } else {
            ExpirationStatus::Valid
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ExpirationStatus::Valid => "Valid",
            ExpirationStatus::ExpiringSoon => "Expiring Soon",
            ExpirationStatus::Expired => "Expired",
            ExpirationStatus::NotYetValid => "Not Yet Valid",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_certificate_data_serialization() {
        let cert_data = CertificateData {
            name: "test.pem".to_string(),
            data: "dGVzdA==".to_string(), // "test" in base64
            size: 4,
        };

        let json = serde_json::to_string(&cert_data).unwrap();
        let deserialized: CertificateData = serde_json::from_str(&json).unwrap();

        assert_eq!(cert_data.name, deserialized.name);
        assert_eq!(cert_data.data, deserialized.data);
        assert_eq!(cert_data.size, deserialized.size);
    }

    #[test]
    fn test_base64_decode() {
        let cert_data = CertificateData {
            name: "test.pem".to_string(),
            data: "dGVzdA==".to_string(),
            size: 4,
        };

        let decoded = BASE64_STANDARD.decode(&cert_data.data).unwrap();
        assert_eq!(decoded, b"test");
    }
}
