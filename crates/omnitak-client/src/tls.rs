use crate::client::{
    ClientConfig, CotMessage, HealthCheck, HealthStatus, MessageMetadata, TakClient,
    calculate_backoff,
};
use crate::state::{ConnectionState, ConnectionStatus};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use base64::Engine;
use bytes::{Buf, BytesMut};
use native_tls::{Certificate, Identity, TlsConnector as NativeTlsConnector};
use omnitak_cert::{CertificateBundle, CertificateData};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::timeout;
use tokio_native_tls::TlsConnector;
use tokio_native_tls::TlsStream;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, instrument, warn};

/// Frame delimiter for newline-delimited protocol
const NEWLINE_DELIMITER: u8 = b'\n';

/// Maximum frame size (10MB)
const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;

/// Protocol framing mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramingMode {
    /// Newline-delimited frames
    Newline,
    /// Length-prefixed frames (4-byte big-endian length header)
    LengthPrefixed,
    /// XML-delimited frames (read until '</event>' character for TAK CoT messages)
    Xml,
}

/// TLS certificate source - either from files or in-memory data
#[derive(Debug, Clone)]
pub enum TlsCertSource {
    /// Load certificates from file paths
    Files {
        /// Path to client certificate (PEM format)
        cert_path: PathBuf,
        /// Path to client private key (PEM format)
        key_path: PathBuf,
        /// Path to CA certificate for server verification (PEM format)
        ca_cert_path: Option<PathBuf>,
    },
    /// Use certificates from memory (uploaded via web UI)
    Memory {
        /// Client certificate data
        cert_data: CertificateData,
        /// Private key data (optional for PKCS#12)
        key_data: Option<CertificateData>,
        /// CA certificate data (optional)
        ca_data: Option<CertificateData>,
        /// Password for encrypted certificates (PKCS#12)
        password: Option<String>,
    },
    /// Use pre-parsed certificate bundle
    Bundle(CertificateBundle),
}

/// TLS certificate configuration
#[derive(Debug, Clone)]
pub struct TlsCertConfig {
    /// Certificate source (files, memory, or bundle)
    pub source: TlsCertSource,
}

/// Configuration specific to TLS client
#[derive(Debug, Clone)]
pub struct TlsClientConfig {
    /// Base client configuration
    pub base: ClientConfig,
    /// TLS certificate configuration
    pub cert_config: TlsCertConfig,
    /// Server name for SNI (Server Name Indication)
    /// If None, will be extracted from server_addr
    pub server_name: Option<String>,
    /// Enable TLS 1.3 only (recommended for security)
    pub tls13_only: bool,
    /// Verify server certificate (should always be true in production)
    pub verify_server: bool,
    /// Framing mode for protocol messages
    pub framing: FramingMode,
}

impl TlsClientConfig {
    /// Create a new TLS client configuration from file paths
    pub fn new(cert_path: PathBuf, key_path: PathBuf) -> Self {
        Self {
            base: ClientConfig::default(),
            cert_config: TlsCertConfig {
                source: TlsCertSource::Files {
                    cert_path,
                    key_path,
                    ca_cert_path: None,
                },
            },
            server_name: None,
            tls13_only: false, // Support both TLS 1.2 and 1.3 for TAK server compatibility
            verify_server: true,
            framing: FramingMode::Xml, // Default to XML for TAK servers
        }
    }

    /// Create a new TLS client configuration from in-memory certificate data
    pub fn from_memory(
        cert_data: CertificateData,
        key_data: Option<CertificateData>,
        ca_data: Option<CertificateData>,
        password: Option<String>,
    ) -> Self {
        Self {
            base: ClientConfig::default(),
            cert_config: TlsCertConfig {
                source: TlsCertSource::Memory {
                    cert_data,
                    key_data,
                    ca_data,
                    password,
                },
            },
            server_name: None,
            tls13_only: false, // Support both TLS 1.2 and 1.3 for TAK server compatibility
            verify_server: true,
            framing: FramingMode::Xml, // Default to XML for TAK servers
        }
    }

    /// Create a new TLS client configuration from a certificate bundle
    pub fn from_bundle(bundle: CertificateBundle) -> Self {
        Self {
            base: ClientConfig::default(),
            cert_config: TlsCertConfig {
                source: TlsCertSource::Bundle(bundle),
            },
            server_name: None,
            tls13_only: false, // Support both TLS 1.2 and 1.3 for TAK server compatibility
            verify_server: true,
            framing: FramingMode::Xml, // Default to XML for TAK servers
        }
    }

    /// Set CA certificate path (only for file-based config)
    pub fn with_ca_cert(mut self, ca_cert_path: PathBuf) -> Self {
        if let TlsCertSource::Files {
            ca_cert_path: ref mut ca,
            ..
        } = self.cert_config.source
        {
            *ca = Some(ca_cert_path);
        }
        self
    }

    /// Set server name for SNI
    pub fn with_server_name(mut self, server_name: String) -> Self {
        self.server_name = Some(server_name);
        self
    }
}

/// TLS client for secure TAK server connections
pub struct TlsClient {
    config: TlsClientConfig,
    status: Arc<ConnectionStatus>,
    stream: Option<TlsStream<TcpStream>>,
    tls_config: Arc<NativeTlsConnector>,
    recv_tx: Option<Sender<Result<CotMessage>>>,
    recv_rx: Option<Receiver<Result<CotMessage>>>,
    shutdown_tx: Option<Sender<()>>,
}

impl TlsClient {
    /// Create a new TLS client
    pub fn new(config: TlsClientConfig) -> Result<Self> {
        let tls_config = Self::build_tls_config(&config)?;
        let (recv_tx, recv_rx) = mpsc::channel(config.base.recv_buffer_size);

        Ok(Self {
            config,
            status: Arc::new(ConnectionStatus::new()),
            stream: None,
            tls_config: Arc::new(tls_config),
            recv_tx: Some(recv_tx),
            recv_rx: Some(recv_rx),
            shutdown_tx: None,
        })
    }

    /// Get connection status
    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    /// Build native-tls configuration
    fn build_tls_config(config: &TlsClientConfig) -> Result<NativeTlsConnector> {
        info!("Building TLS configuration with native-tls");

        let mut builder = NativeTlsConnector::builder();

        // Load certificates based on source
        match &config.cert_config.source {
            TlsCertSource::Files {
                cert_path,
                key_path,
                ca_cert_path,
            } => {
                info!("Loading certificates from files");

                // Check if this is a P12 file (both cert and key point to same file)
                if cert_path == key_path
                    && cert_path.extension().and_then(|s| s.to_str()) == Some("p12")
                {
                    // Load PKCS#12 file directly
                    let p12_data =
                        std::fs::read(cert_path).context("Failed to read P12 certificate file")?;

                    // Try with common passwords
                    let identity = Identity::from_pkcs12(&p12_data, "omnitak")
                        .or_else(|_| Identity::from_pkcs12(&p12_data, ""))
                        .or_else(|_| Identity::from_pkcs12(&p12_data, "changeit"))
                        .or_else(|_| Identity::from_pkcs12(&p12_data, "atakatak"))
                        .context("Failed to load P12 identity (tried 'omnitak', empty, 'changeit', 'atakatak')")?;

                    builder.identity(identity);
                    info!("Loaded P12 identity from file");
                } else {
                    // Convert PEM files to PKCS#12 using openssl command
                    // This is necessary because native-tls doesn't support loading from separate PEM files
                    info!("Converting PEM certificates to PKCS#12 format");

                    let temp_p12 = std::env::temp_dir()
                        .join(format!("omnitak_temp_{}.p12", std::process::id()));

                    // Create PKCS#12 file from PEM cert and key using 3DES encryption for compatibility
                    let openssl_result = std::process::Command::new("openssl")
                        .args([
                            "pkcs12",
                            "-export",
                            "-descert", // Use 3DES for better compatibility with native-tls
                            "-out",
                            temp_p12.to_str().unwrap(),
                            "-inkey",
                            key_path.to_str().unwrap(),
                            "-in",
                            cert_path.to_str().unwrap(),
                            "-password",
                            "pass:", // Empty password
                        ])
                        .output()
                        .context("Failed to execute openssl command")?;

                    if !openssl_result.status.success() {
                        return Err(anyhow!(
                            "Failed to convert PEM to PKCS12: {}",
                            String::from_utf8_lossy(&openssl_result.stderr)
                        ));
                    }

                    // Load the temporary P12 file
                    let p12_data =
                        std::fs::read(&temp_p12).context("Failed to read temporary P12 file")?;

                    let identity = Identity::from_pkcs12(&p12_data, "")
                        .map_err(|e| anyhow!("Failed to load converted P12 identity: {}. This may be due to encryption compatibility issues with native-tls on your platform.", e))?;

                    builder.identity(identity);

                    // Clean up temporary file
                    let _ = std::fs::remove_file(&temp_p12);

                    info!("Loaded client certificate and key from PEM files (converted to P12)");
                }

                // Load CA certificate if provided AND if we're verifying certs
                // Skip CA loading when verification is disabled to avoid macOS Security.framework issues
                if let Some(ca_path) = ca_cert_path {
                    if config.verify_server {
                        let ca_data =
                            std::fs::read(ca_path).context("Failed to read CA certificate file")?;

                        // Try loading as PEM first, then as P12 truststore
                        let ca_cert = Certificate::from_pem(&ca_data)
                            .or_else(|_| {
                                // If PEM loading fails, try as P12 truststore
                                // Extract CA certs from P12 file
                                Identity::from_pkcs12(&ca_data, "")
                                    .or_else(|_| Identity::from_pkcs12(&ca_data, "changeit"))
                                    .map_err(|e| anyhow!("Failed to load CA from P12: {}", e))
                                    .and_then(|_| Err(anyhow!("P12 CA extraction not yet implemented, please use PEM format for CA")))
                            })
                            .context("Failed to load CA certificate (tried PEM and P12 formats)")?;

                        builder.add_root_certificate(ca_cert);
                        info!("Loaded custom CA certificate from file");
                    } else {
                        info!(
                            "Skipping CA certificate loading because server verification is disabled"
                        );
                    }
                }
            }
            TlsCertSource::Memory {
                cert_data,
                key_data,
                ca_data,
                password,
            } => {
                info!("Loading certificates from memory");

                // Decode base64-encoded certificate data
                let cert_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&cert_data.data)
                    .context("Failed to decode certificate from base64")?;

                // With a separate key, treat the upload as PEM cert + PKCS#8 key.
                // Without one, treat the cert blob as a PKCS#12 bundle (cert + key
                // together) and try the supplied password plus common TAK defaults.
                let identity = match key_data.as_ref() {
                    Some(kd) => {
                        let key_bytes = base64::engine::general_purpose::STANDARD
                            .decode(&kd.data)
                            .context("Failed to decode key from base64")?;
                        Identity::from_pkcs8(&cert_bytes, &key_bytes)
                            .context("Failed to create identity from PEM cert/key")?
                    }
                    None => {
                        let mut passwords: Vec<&str> = Vec::new();
                        if let Some(p) = password.as_deref() {
                            passwords.push(p);
                        }
                        passwords.extend(["omnitak", "", "changeit", "atakatak"]);

                        let mut identity = None;
                        let mut last_err = None;
                        for p in passwords {
                            match Identity::from_pkcs12(&cert_bytes, p) {
                                Ok(id) => {
                                    identity = Some(id);
                                    break;
                                }
                                Err(e) => last_err = Some(e),
                            }
                        }
                        identity.ok_or_else(|| {
                            anyhow!(
                                "Failed to load PKCS#12 identity from upload (no separate key; tried supplied password + 'omnitak'/empty/'changeit'/'atakatak'): {}",
                                last_err.map(|e| e.to_string()).unwrap_or_default()
                            )
                        })?
                    }
                };
                builder.identity(identity);

                // Load CA if provided AND if we're verifying certs
                if let Some(ca) = ca_data {
                    if config.verify_server {
                        let ca_bytes = base64::engine::general_purpose::STANDARD
                            .decode(&ca.data)
                            .context("Failed to decode CA from base64")?;
                        let ca_cert = Certificate::from_pem(&ca_bytes)
                            .context("Failed to load CA certificate from memory")?;
                        builder.add_root_certificate(ca_cert);
                        info!("Loaded custom CA certificate from memory");
                    } else {
                        info!(
                            "Skipping CA certificate loading because server verification is disabled"
                        );
                    }
                }
            }
            TlsCertSource::Bundle(_bundle) => {
                return Err(anyhow!(
                    "Bundle source not supported with native-tls, use Files or Memory"
                ));
            }
        }

        // Configure certificate verification
        if !config.verify_server {
            warn!("Server certificate verification is DISABLED - this is insecure!");
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }

        // Build the connector - native-tls automatically supports TLS 1.2 and 1.3
        // This provides maximum compatibility with TAK servers
        let tls_config = builder.build().context("Failed to build TLS config")?;

        info!("TLS configuration built successfully with native-tls (OpenSSL backend)");
        Ok(tls_config)
    }

    /// Extract server name from address for SNI
    fn get_server_name(&self) -> String {
        self.config
            .server_name
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Extract hostname from server_addr
                self.config
                    .base
                    .server_addr
                    .split(':')
                    .next()
                    .unwrap_or("localhost")
                    .to_string()
            })
    }

    /// Establish TLS connection
    #[instrument(skip(self))]
    async fn establish_connection(&mut self) -> Result<()> {
        self.status.set_state(ConnectionState::Connecting);

        info!("Connecting to {} with TLS", self.config.base.server_addr);

        // Establish TCP connection first
        let tcp_stream = timeout(
            self.config.base.connect_timeout,
            TcpStream::connect(&self.config.base.server_addr),
        )
        .await
        .context("Connection timeout")?
        .context("Failed to connect")?;

        // Perform TLS handshake
        let server_name = self.get_server_name();
        info!("Attempting TLS handshake with server name: {}", server_name);
        let connector = TlsConnector::from((*self.tls_config).clone());

        let tls_stream = match timeout(
            self.config.base.connect_timeout,
            connector.connect(&server_name, tcp_stream),
        )
        .await
        {
            Ok(Ok(stream)) => {
                info!("TLS handshake successful");
                stream
            }
            Ok(Err(e)) => {
                error!("TLS handshake failed with detailed error: {:?}", e);
                error!("Error source chain: {}", e);
                if let Some(source) = e.source() {
                    error!("Error source: {:?}", source);
                }
                return Err(anyhow!("TLS handshake failed: {}", e));
            }
            Err(_) => {
                error!(
                    "TLS handshake timeout after {:?}",
                    self.config.base.connect_timeout
                );
                return Err(anyhow!("TLS handshake timeout"));
            }
        };

        self.stream = Some(tls_stream);
        self.status.set_state(ConnectionState::Connected);
        self.status.metrics().mark_connected();

        Ok(())
    }

    /// Connect to the server without starting the receive task
    /// This is useful when you want to manually manage reading and writing
    pub async fn connect_only(&mut self) -> Result<()> {
        let config = self.config.base.reconnect.clone();

        // Inline retry logic to avoid closure capture issues

        if !config.enabled {
            self.establish_connection().await
        } else {
            let mut attempt = 0u32;
            loop {
                match self.establish_connection().await {
                    Ok(()) => {
                        if attempt > 0 {
                            info!(
                                attempt = attempt,
                                "Successfully reconnected after {} attempts", attempt
                            );
                        }
                        break Ok(());
                    }
                    Err(e) => {
                        attempt += 1;

                        if let Some(max) = config.max_attempts
                            && attempt >= max
                        {
                            error!(
                                attempt = attempt,
                                error = %e,
                                "Max reconnect attempts reached"
                            );
                            break Err(e);
                        }

                        let backoff = calculate_backoff(attempt - 1, &config);
                        warn!(
                            attempt = attempt,
                            backoff_secs = backoff.as_secs(),
                            error = %e,
                            "Connection attempt failed, retrying after backoff"
                        );

                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }
    }

    /// Get a mutable reference to the stream for manual reading/writing
    pub fn stream_mut(&mut self) -> Option<&mut TlsStream<TcpStream>> {
        self.stream.as_mut()
    }

    /// Get the framing mode
    pub fn framing(&self) -> FramingMode {
        self.config.framing
    }

    /// Write a frame to the stream (public method for direct access)
    pub async fn write_frame_direct(&mut self, data: &[u8]) -> Result<()> {
        self.write_frame(data).await
    }

    /// Read a frame from the TLS stream
    async fn read_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        match self.config.framing {
            FramingMode::Newline => self.read_newline_frame(buffer).await,
            FramingMode::LengthPrefixed => self.read_length_prefixed_frame(buffer).await,
            FramingMode::Xml => self.read_xml_frame(buffer).await,
        }
    }

    /// Read a newline-delimited frame from the TLS stream
    async fn read_newline_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        loop {
            // Check if we have a complete frame in the buffer
            if let Some(pos) = buffer.iter().position(|&b| b == NEWLINE_DELIMITER) {
                let frame = buffer.split_to(pos + 1);
                let mut frame_bytes = frame.freeze();

                // Remove the newline delimiter
                if frame_bytes.last() == Some(&NEWLINE_DELIMITER) {
                    frame_bytes.truncate(frame_bytes.len() - 1);
                }

                self.status
                    .metrics()
                    .record_bytes_received(frame_bytes.len() as u64);
                return Ok(Some(frame_bytes));
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("Frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Read a length-prefixed frame from the TLS stream
    async fn read_length_prefixed_frame(
        &mut self,
        buffer: &mut BytesMut,
    ) -> Result<Option<bytes::Bytes>> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        loop {
            // Check if we have at least 4 bytes for the length header
            if buffer.len() >= 4 {
                let mut length_bytes = [0u8; 4];
                length_bytes.copy_from_slice(&buffer[..4]);
                let frame_length = u32::from_be_bytes(length_bytes) as usize;

                // Validate frame length
                if frame_length > MAX_FRAME_SIZE {
                    return Err(anyhow!("Frame length {} exceeds maximum", frame_length));
                }

                // Check if we have the complete frame
                if buffer.len() >= 4 + frame_length {
                    buffer.advance(4); // Skip length header
                    let frame = buffer.split_to(frame_length).freeze();
                    self.status
                        .metrics()
                        .record_bytes_received(frame.len() as u64);
                    return Ok(Some(frame));
                }
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE + 4 {
                return Err(anyhow!("Frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete frame"));
                }
            }
        }
    }

    /// Read an XML-delimited frame from the TLS stream (for TAK CoT messages)
    /// TAK Protocol: Messages are delimited by the "</event>" token
    /// Per TAK spec: "Messages are delimited and broken apart by searching for
    /// the token '</event>' and breaking apart immediately after that token."
    async fn read_xml_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<bytes::Bytes>> {
        const XML_END_TOKEN: &[u8] = b"</event>";

        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        loop {
            // Search for the complete </event> token
            if buffer.len() >= XML_END_TOKEN.len()
                && let Some(pos) = buffer
                    .windows(XML_END_TOKEN.len())
                    .position(|window| window == XML_END_TOKEN)
            {
                // Split immediately after the </event> token
                let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                let frame_bytes = frame.freeze();

                // Validate that it looks like XML (starts with '<')
                if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                    warn!("Received data not starting with '<', skipping invalid frame");
                    continue;
                }

                self.status
                    .metrics()
                    .record_bytes_received(frame_bytes.len() as u64);
                return Ok(Some(frame_bytes));
            }

            // Check buffer size limit
            if buffer.len() >= MAX_FRAME_SIZE {
                return Err(anyhow!("XML frame too large"));
            }

            // Read more data
            let read_result = timeout(self.config.base.read_timeout, stream.read_buf(buffer))
                .await
                .context("Read timeout")?
                .context("Read error")?;

            if read_result == 0 {
                if buffer.is_empty() {
                    return Ok(None); // Clean disconnect
                } else {
                    return Err(anyhow!("Connection closed with incomplete XML frame"));
                }
            }
        }
    }

    /// Write a frame to the TLS stream
    async fn write_frame(&mut self, data: &[u8]) -> Result<()> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        match self.config.framing {
            FramingMode::Newline => {
                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;

                timeout(
                    self.config.base.write_timeout,
                    stream.write_all(&[NEWLINE_DELIMITER]),
                )
                .await
                .context("Write timeout")?
                .context("Write error")?;
            }
            FramingMode::LengthPrefixed => {
                let length = data.len() as u32;
                let length_bytes = length.to_be_bytes();

                timeout(
                    self.config.base.write_timeout,
                    stream.write_all(&length_bytes),
                )
                .await
                .context("Write timeout")?
                .context("Write error")?;

                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;
            }
            FramingMode::Xml => {
                // For XML framing, write the data as-is (should already be complete XML)
                timeout(self.config.base.write_timeout, stream.write_all(data))
                    .await
                    .context("Write timeout")?
                    .context("Write error")?;
            }
        }

        stream.flush().await.context("Flush error")?;
        self.status.metrics().record_bytes_sent(data.len() as u64);

        Ok(())
    }

    /// Start background receive task
    fn start_receive_task(&mut self) {
        let mut buffer = BytesMut::with_capacity(8192);
        let status = Arc::clone(&self.status);
        let tx = self.recv_tx.as_ref().unwrap().clone();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);
        let framing = self.config.framing;

        // Move the stream out for the task
        if let Some(mut stream) = self.stream.take() {
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            debug!("TLS receive task shutting down");
                            break;
                        }
                        result = Self::read_frame_static(&mut stream, &mut buffer, &status, framing) => {
                            match result {
                                Ok(Some(frame)) => {
                                    status.metrics().record_message_received();
                                    let message = CotMessage {
                                        data: frame,
                                        metadata: Some(MessageMetadata {
                                            received_at: std::time::SystemTime::now(),
                                            source_addr: None,
                                        }),
                                    };

                                    if tx.send(Ok(message)).await.is_err() {
                                        debug!("Receive channel closed");
                                        break;
                                    }
                                }
                                Ok(None) => {
                                    info!("TLS connection closed by remote");
                                    status.set_state(ConnectionState::Disconnected);
                                    break;
                                }
                                Err(e) => {
                                    error!(error = %e, "Error reading frame");
                                    status.metrics().record_error();
                                    let _ = tx.send(Err(e)).await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    /// Static helper for reading frames (used in async task)
    pub async fn read_frame_static(
        stream: &mut TlsStream<TcpStream>,
        buffer: &mut BytesMut,
        status: &ConnectionStatus,
        framing: FramingMode,
    ) -> Result<Option<bytes::Bytes>> {
        match framing {
            FramingMode::Newline => loop {
                if let Some(pos) = buffer.iter().position(|&b| b == NEWLINE_DELIMITER) {
                    let frame = buffer.split_to(pos + 1);
                    let mut frame_bytes = frame.freeze();

                    if frame_bytes.last() == Some(&NEWLINE_DELIMITER) {
                        frame_bytes.truncate(frame_bytes.len() - 1);
                    }

                    status
                        .metrics()
                        .record_bytes_received(frame_bytes.len() as u64);
                    return Ok(Some(frame_bytes));
                }

                if buffer.len() >= MAX_FRAME_SIZE {
                    return Err(anyhow!("Frame too large"));
                }

                let n = stream.read_buf(buffer).await.context("Read error")?;

                if n == 0 {
                    if buffer.is_empty() {
                        return Ok(None);
                    } else {
                        return Err(anyhow!("Connection closed with incomplete frame"));
                    }
                }
            },
            FramingMode::LengthPrefixed => {
                loop {
                    // Check if we have at least 4 bytes for the length header
                    if buffer.len() >= 4 {
                        let mut length_bytes = [0u8; 4];
                        length_bytes.copy_from_slice(&buffer[..4]);
                        let frame_length = u32::from_be_bytes(length_bytes) as usize;

                        // Validate frame length
                        if frame_length > MAX_FRAME_SIZE {
                            return Err(anyhow!("Frame length {} exceeds maximum", frame_length));
                        }

                        // Check if we have the complete frame
                        if buffer.len() >= 4 + frame_length {
                            buffer.advance(4); // Skip length header
                            let frame = buffer.split_to(frame_length).freeze();
                            status.metrics().record_bytes_received(frame.len() as u64);
                            return Ok(Some(frame));
                        }
                    }

                    // Check buffer size limit
                    if buffer.len() >= MAX_FRAME_SIZE + 4 {
                        return Err(anyhow!("Frame too large"));
                    }

                    let n = stream.read_buf(buffer).await.context("Read error")?;

                    if n == 0 {
                        if buffer.is_empty() {
                            return Ok(None);
                        } else {
                            return Err(anyhow!("Connection closed with incomplete frame"));
                        }
                    }
                }
            }
            FramingMode::Xml => {
                // TAK Protocol: Messages are delimited by searching for "</event>" token
                // Per TAK spec: "Messages are delimited and broken apart by searching for
                // the token '</event>' and breaking apart immediately after that token."
                const XML_END_TOKEN: &[u8] = b"</event>";

                loop {
                    // Search for the complete </event> token
                    if buffer.len() >= XML_END_TOKEN.len() {
                        // Look for the </event> token in the buffer
                        if let Some(pos) = buffer
                            .windows(XML_END_TOKEN.len())
                            .position(|window| window == XML_END_TOKEN)
                        {
                            // Split immediately after the </event> token
                            let frame = buffer.split_to(pos + XML_END_TOKEN.len());
                            let frame_bytes = frame.freeze();

                            // Validate that it looks like XML (starts with '<')
                            // Most CoT messages start with <?xml but some may start directly with <event>
                            if frame_bytes.is_empty() || frame_bytes[0] != b'<' {
                                // Skip invalid data - continue reading
                                warn!(
                                    "Received data not starting with '<', skipping invalid frame"
                                );
                                continue;
                            }

                            status
                                .metrics()
                                .record_bytes_received(frame_bytes.len() as u64);
                            return Ok(Some(frame_bytes));
                        }
                    }

                    // Check buffer size limit
                    if buffer.len() >= MAX_FRAME_SIZE {
                        return Err(anyhow!("XML frame too large"));
                    }

                    let n = stream.read_buf(buffer).await.context("Read error")?;

                    if n == 0 {
                        if buffer.is_empty() {
                            return Ok(None);
                        } else {
                            return Err(anyhow!("Connection closed with incomplete XML frame"));
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl TakClient for TlsClient {
    async fn connect(&mut self) -> Result<()> {
        let config = self.config.base.reconnect.clone();

        // Inline retry logic to avoid closure capture issues
        let result = if !config.enabled {
            self.establish_connection().await
        } else {
            let mut attempt = 0u32;
            loop {
                match self.establish_connection().await {
                    Ok(()) => {
                        if attempt > 0 {
                            info!(
                                attempt = attempt,
                                "Successfully reconnected after {} attempts", attempt
                            );
                        }
                        break Ok(());
                    }
                    Err(e) => {
                        attempt += 1;

                        if let Some(max) = config.max_attempts
                            && attempt >= max
                        {
                            error!(
                                attempt = attempt,
                                error = %e,
                                "Max reconnect attempts reached"
                            );
                            break Err(e);
                        }

                        let backoff = calculate_backoff(attempt - 1, &config);
                        warn!(
                            attempt = attempt,
                            backoff_secs = backoff.as_secs(),
                            error = %e,
                            "Connection attempt failed, retrying after backoff"
                        );

                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        };

        if result.is_ok() {
            self.start_receive_task();
        }

        result
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from {}", self.config.base.server_addr);

        // Signal shutdown to receive task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        // Close the stream
        if let Some(mut stream) = self.stream.take() {
            stream
                .shutdown()
                .await
                .context("Failed to shutdown stream")?;
        }

        self.status.set_state(ConnectionState::Disconnected);
        self.status.metrics().mark_disconnected();

        Ok(())
    }

    async fn send_cot(&mut self, message: CotMessage) -> Result<()> {
        if !self.is_connected() {
            return Err(anyhow!("Not connected"));
        }

        debug!(size = message.data.len(), "Sending CoT message over TLS");

        self.write_frame(&message.data).await?;
        self.status.metrics().record_message_sent();

        Ok(())
    }

    fn receive_cot(&mut self) -> ReceiverStream<Result<CotMessage>> {
        let rx = self.recv_rx.take().unwrap_or_else(|| {
            let (_, rx) = mpsc::channel(1);
            rx
        });

        ReceiverStream::new(rx)
    }

    async fn health_check(&self) -> HealthCheck {
        let status = self.status.state();

        let health_status = match status {
            ConnectionState::Connected => {
                let time_since_activity = self.status.metrics().time_since_last_activity();
                if time_since_activity > std::time::Duration::from_secs(60) {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Healthy
                }
            }
            ConnectionState::Connecting | ConnectionState::Reconnecting => HealthStatus::Degraded,
            ConnectionState::Disconnected => HealthStatus::Disconnected,
            ConnectionState::Failed => HealthStatus::Unhealthy,
        };

        HealthCheck {
            status: health_status,
            message: self.status.error_message(),
            rtt: None,
        }
    }

    fn is_connected(&self) -> bool {
        self.status.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_cert_config() {
        let config = TlsCertConfig {
            source: TlsCertSource::Files {
                cert_path: PathBuf::from("/path/to/cert.pem"),
                key_path: PathBuf::from("/path/to/key.pem"),
                ca_cert_path: None,
            },
        };

        if let TlsCertSource::Files { cert_path, .. } = &config.source {
            assert_eq!(cert_path, &PathBuf::from("/path/to/cert.pem"));
        } else {
            panic!("Expected Files variant");
        }
    }

    #[test]
    fn test_tls_client_config_builder() {
        let config = TlsClientConfig::new(PathBuf::from("/cert.pem"), PathBuf::from("/key.pem"))
            .with_server_name("example.com".to_string());

        assert_eq!(config.server_name, Some("example.com".to_string()));
        assert!(config.tls13_only);
        assert_eq!(config.framing, FramingMode::Xml);
    }

    #[test]
    fn test_framing_mode() {
        assert_eq!(FramingMode::Xml, FramingMode::Xml);
        assert_ne!(FramingMode::Newline, FramingMode::Xml);
        assert_ne!(FramingMode::LengthPrefixed, FramingMode::Xml);
    }
}
