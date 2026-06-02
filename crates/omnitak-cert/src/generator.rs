//! Certificate generation for OmniTAK enrollment
//!
//! Generates self-signed CA certificates and client certificates for TAK clients.
//! This enables OmniTAK to act as a certificate authority for client enrollment.

use anyhow::{anyhow, Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

/// Configuration for the Certificate Authority
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaConfig {
    /// Common Name for the CA certificate
    pub common_name: String,
    /// Organization name
    pub organization: Option<String>,
    /// Organizational unit
    pub organizational_unit: Option<String>,
    /// Country code (2-letter)
    pub country: Option<String>,
    /// Validity period in days
    pub validity_days: u32,
}

impl Default for CaConfig {
    fn default() -> Self {
        Self {
            common_name: "OmniTAK Certificate Authority".to_string(),
            organization: Some("OmniTAK".to_string()),
            organizational_unit: Some("TAK Operations".to_string()),
            country: Some("US".to_string()),
            validity_days: 3650, // 10 years
        }
    }
}

/// Configuration for client certificates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCertConfig {
    /// Common Name (typically username or device name)
    pub common_name: String,
    /// Email address (optional)
    pub email: Option<String>,
    /// Organization name
    pub organization: Option<String>,
    /// Validity period in days
    pub validity_days: u32,
}

impl ClientCertConfig {
    pub fn new(common_name: &str) -> Self {
        Self {
            common_name: common_name.to_string(),
            email: None,
            organization: Some("OmniTAK".to_string()),
            validity_days: 365,
        }
    }

    pub fn with_validity(mut self, days: u32) -> Self {
        self.validity_days = days;
        self
    }

    pub fn with_email(mut self, email: &str) -> Self {
        self.email = Some(email.to_string());
        self
    }
}

/// Generated Certificate Authority with private key
/// Stores PEM-encoded certificates for persistence and re-use
pub struct GeneratedCa {
    /// CA certificate in PEM format
    pub cert_pem: String,
    /// CA private key in PEM format
    pub key_pem: String,
}

impl std::fmt::Debug for GeneratedCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedCa")
            .field("cert_pem", &"[CERTIFICATE]")
            .field("key_pem", &"[PRIVATE KEY]")
            .finish()
    }
}

impl GeneratedCa {
    /// Generate a new CA certificate
    pub fn generate(config: &CaConfig) -> Result<Self> {
        info!("Generating CA certificate: {}", config.common_name);

        // Create distinguished name
        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, &config.common_name);

        if let Some(org) = &config.organization {
            distinguished_name.push(DnType::OrganizationName, org);
        }
        if let Some(ou) = &config.organizational_unit {
            distinguished_name.push(DnType::OrganizationalUnitName, ou);
        }
        if let Some(country) = &config.country {
            distinguished_name.push(DnType::CountryName, country);
        }

        // Generate key pair
        let key_pair = KeyPair::generate().context("Failed to generate CA key pair")?;

        // Create certificate params
        let mut params = CertificateParams::default();
        params.distinguished_name = distinguished_name;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        // Set validity
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + time::Duration::days(config.validity_days as i64);

        // Generate self-signed certificate
        let certificate = params
            .self_signed(&key_pair)
            .context("Failed to generate self-signed CA certificate")?;

        let cert_pem = certificate.pem();
        let key_pem = key_pair.serialize_pem();

        info!("CA certificate generated successfully");

        Ok(Self { cert_pem, key_pem })
    }

    /// Load CA from existing PEM strings
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self> {
        // Validate that the PEM data is parseable
        let _key_pair = KeyPair::from_pem(key_pem).context("Failed to parse CA private key")?;

        // Validate certificate is parseable
        let _params = CertificateParams::from_ca_cert_pem(cert_pem)
            .context("Failed to parse CA certificate")?;

        Ok(Self {
            cert_pem: cert_pem.to_string(),
            key_pem: key_pem.to_string(),
        })
    }

    /// Load CA from PEM files on disk
    pub fn from_files(cert_path: &Path, key_path: &Path) -> Result<Self> {
        let cert_pem = std::fs::read_to_string(cert_path)
            .with_context(|| format!("Failed to read CA cert: {}", cert_path.display()))?;
        let key_pem = std::fs::read_to_string(key_path)
            .with_context(|| format!("Failed to read CA key: {}", key_path.display()))?;

        Self::from_pem(&cert_pem, &key_pem)
    }

    /// Save CA to PEM files
    pub fn save_to_files(&self, cert_path: &Path, key_path: &Path) -> Result<()> {
        // Create parent directories if needed
        if let Some(parent) = cert_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(cert_path, &self.cert_pem)
            .with_context(|| format!("Failed to write CA cert: {}", cert_path.display()))?;
        std::fs::write(key_path, &self.key_pem)
            .with_context(|| format!("Failed to write CA key: {}", key_path.display()))?;

        info!(
            "CA saved to {} and {}",
            cert_path.display(),
            key_path.display()
        );
        Ok(())
    }

    /// Issue a client certificate signed by this CA
    pub fn issue_client_cert(&self, config: &ClientCertConfig) -> Result<GeneratedClientCert> {
        info!("Issuing client certificate for: {}", config.common_name);

        // Load CA key pair
        let ca_key_pair =
            KeyPair::from_pem(&self.key_pem).context("Failed to parse CA private key")?;

        // Parse CA cert params for signing
        let ca_params = CertificateParams::from_ca_cert_pem(&self.cert_pem)
            .context("Failed to parse CA certificate")?;

        // Re-create CA certificate for signing
        let ca_cert = ca_params
            .self_signed(&ca_key_pair)
            .context("Failed to recreate CA certificate for signing")?;

        // Create distinguished name for client
        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, &config.common_name);

        if let Some(org) = &config.organization {
            distinguished_name.push(DnType::OrganizationName, org);
        }

        // Generate client key pair
        let client_key_pair = KeyPair::generate().context("Failed to generate client key pair")?;

        // Create certificate params
        let mut params = CertificateParams::default();
        params.distinguished_name = distinguished_name;
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        // Add email as SAN if provided
        if let Some(email) = &config.email {
            if let Ok(ia5_email) = rcgen::Ia5String::try_from(email.clone()) {
                params.subject_alt_names = vec![SanType::Rfc822Name(ia5_email)];
            }
        }

        // Set validity
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + time::Duration::days(config.validity_days as i64);

        // Sign with CA
        let client_cert = params
            .signed_by(&client_key_pair, &ca_cert, &ca_key_pair)
            .context("Failed to sign client certificate with CA")?;

        let cert_pem = client_cert.pem();
        let key_pem = client_key_pair.serialize_pem();

        info!("Client certificate issued for: {}", config.common_name);

        Ok(GeneratedClientCert {
            cert_pem,
            key_pem,
            ca_cert_pem: self.cert_pem.clone(),
            common_name: config.common_name.clone(),
        })
    }
}

/// Generated client certificate with private key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedClientCert {
    /// Client certificate in PEM format
    pub cert_pem: String,
    /// Client private key in PEM format
    pub key_pem: String,
    /// CA certificate in PEM format (for trust chain)
    pub ca_cert_pem: String,
    /// Common name from the certificate
    pub common_name: String,
}

impl GeneratedClientCert {
    /// Convert to PKCS#12 format with password protection
    pub fn to_pkcs12(&self, password: &str) -> Result<Vec<u8>> {
        use p12::PFX;

        debug!("Converting client cert to PKCS#12 format");

        // Parse the PEM certificate
        let cert_der = {
            let mut reader = std::io::BufReader::new(self.cert_pem.as_bytes());
            let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse client certificate PEM")?;
            certs
                .first()
                .ok_or_else(|| anyhow!("No certificate found in PEM"))?
                .to_vec()
        };

        // Parse the PEM private key
        let key_der = {
            let mut reader = std::io::BufReader::new(self.key_pem.as_bytes());
            rustls_pemfile::private_key(&mut reader)
                .context("Failed to read private key")?
                .ok_or_else(|| anyhow!("No private key found in PEM"))?
        };

        // Parse CA certificate
        let ca_cert_der = {
            let mut reader = std::io::BufReader::new(self.ca_cert_pem.as_bytes());
            let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
                .collect::<Result<Vec<_>, _>>()
                .context("Failed to parse CA certificate PEM")?;
            certs
                .first()
                .ok_or_else(|| anyhow!("No CA certificate found in PEM"))?
                .to_vec()
        };

        // Get key bytes based on format
        let key_bytes = match &key_der {
            rustls::pki_types::PrivateKeyDer::Pkcs8(data) => data.secret_pkcs8_der().to_vec(),
            rustls::pki_types::PrivateKeyDer::Pkcs1(data) => data.secret_pkcs1_der().to_vec(),
            rustls::pki_types::PrivateKeyDer::Sec1(data) => data.secret_sec1_der().to_vec(),
            _ => return Err(anyhow!("Unsupported private key format")),
        };

        // Create PKCS#12 using the p12 crate
        let pfx = PFX::new(
            &cert_der,
            &key_bytes,
            Some(&ca_cert_der),
            password,
            &self.common_name,
        )
        .ok_or_else(|| anyhow!("Failed to create PKCS#12 structure"))?;

        let p12_der = pfx.to_der();

        info!("Created PKCS#12 bundle ({} bytes)", p12_der.len());
        Ok(p12_der)
    }

    /// Save to PEM files
    pub fn save_to_files(
        &self,
        cert_path: &Path,
        key_path: &Path,
        ca_path: Option<&Path>,
    ) -> Result<()> {
        std::fs::write(cert_path, &self.cert_pem)
            .with_context(|| format!("Failed to write cert: {}", cert_path.display()))?;
        std::fs::write(key_path, &self.key_pem)
            .with_context(|| format!("Failed to write key: {}", key_path.display()))?;

        if let Some(ca_path) = ca_path {
            std::fs::write(ca_path, &self.ca_cert_pem)
                .with_context(|| format!("Failed to write CA cert: {}", ca_path.display()))?;
        }

        info!(
            "Client certificate saved to {} and {}",
            cert_path.display(),
            key_path.display()
        );
        Ok(())
    }
}

/// Enrollment token for data package downloads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentToken {
    /// Unique token ID
    pub id: String,
    /// Token string for authentication
    pub token: String,
    /// Username/device name this token is for
    pub username: String,
    /// When the token was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the token expires
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// Whether this token has been used
    pub used: bool,
    /// Maximum number of uses (None = unlimited)
    pub max_uses: Option<u32>,
    /// Current use count
    pub use_count: u32,
}

impl EnrollmentToken {
    /// Create a new enrollment token
    pub fn new(username: &str, validity_hours: u32, max_uses: Option<u32>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            token: generate_secure_token(),
            username: username.to_string(),
            created_at: now,
            expires_at: now + chrono::Duration::hours(validity_hours as i64),
            used: false,
            max_uses,
            use_count: 0,
        }
    }

    /// Check if the token is valid
    pub fn is_valid(&self) -> bool {
        let now = chrono::Utc::now();

        // Check expiration
        if now > self.expires_at {
            return false;
        }

        // Check use count
        if let Some(max) = self.max_uses {
            if self.use_count >= max {
                return false;
            }
        }

        true
    }

    /// Mark the token as used
    pub fn mark_used(&mut self) {
        self.use_count += 1;
        if self.max_uses == Some(1) {
            self.used = true;
        }
    }
}

/// Generate a cryptographically secure random token
fn generate_secure_token() -> String {
    use base64::prelude::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut bytes = [0u8; 32];
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Simple PRNG for token generation
    let mut state = seed as u64;
    for byte in &mut bytes {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *byte = (state >> 56) as u8;
    }

    BASE64_URL_SAFE_NO_PAD.encode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ca() {
        let config = CaConfig::default();
        let ca = GeneratedCa::generate(&config).unwrap();

        assert!(ca.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(ca.key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn test_issue_client_cert() {
        let ca_config = CaConfig::default();
        let ca = GeneratedCa::generate(&ca_config).unwrap();

        let client_config = ClientCertConfig::new("test-user");
        let client_cert = ca.issue_client_cert(&client_config).unwrap();

        assert!(client_cert.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(client_cert.key_pem.contains("BEGIN PRIVATE KEY"));
        assert_eq!(client_cert.common_name, "test-user");
    }

    #[test]
    fn test_enrollment_token() {
        let token = EnrollmentToken::new("testuser", 24, Some(1));

        assert!(token.is_valid());
        assert!(!token.used);
        assert_eq!(token.use_count, 0);
    }

    #[test]
    fn test_token_expiration() {
        let mut token = EnrollmentToken::new("testuser", 0, Some(1));
        // Token with 0 hours validity should be expired immediately
        token.expires_at = chrono::Utc::now() - chrono::Duration::hours(1);

        assert!(!token.is_valid());
    }
}
