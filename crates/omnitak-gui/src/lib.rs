//! GUI application for OmniTAK TAK server aggregator.
//!
//! This crate provides a native desktop GUI built with egui/eframe for managing
//! OmniTAK server connections, viewing status, and monitoring message flow.

use eframe::egui;
use omnitak_core::types::{
    ConnectionId, ConnectionMetadata, Protocol, ServerConfig, ServerStatus, TlsConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod ui;

pub mod backend;
use backend::BackendService;

pub mod api_client;
pub use api_client::{ApiClient, ConnectionInfo, ConnectionType, CreateConnectionRequest};

pub mod config_io;
pub use config_io::{export_config, import_config, ConfigFile};

/// Main application state for the OmniTAK GUI.
pub struct OmniTakApp {
    /// Configuration state
    pub state: Arc<Mutex<AppState>>,

    /// UI state
    pub ui_state: UiState,

    /// Backend service (standalone mode - deprecated)
    pub backend: Option<BackendService>,

    /// API client (unified mode - preferred)
    pub api_client: Option<ApiClient>,

    /// Shared tokio runtime for async operations
    pub runtime: tokio::runtime::Runtime,

    /// Embedded API server process
    pub embedded_server: Option<Child>,

    /// API server URL
    pub api_url: String,

    /// API server port
    pub api_port: u16,

    /// Login credentials
    pub login_username: String,
    pub login_password: String,
    pub login_error: Option<String>,
    pub is_authenticated: bool,

    /// Status message for user notifications
    pub status_message: Option<(String, StatusLevel)>,

    /// Timestamp for status message expiry
    pub status_message_expiry: Option<std::time::Instant>,

    /// Last refresh timestamp
    pub last_refresh: std::time::Instant,

    /// Flag to track if auto-start has been performed
    pub auto_start_done: bool,

    /// Command palette state
    pub command_palette: ui::command_palette::CommandPaletteState,

    /// Flag to track if theme has been initialized
    pub theme_initialized: bool,
}

/// Status message level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl Default for OmniTakApp {
    fn default() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        let api_port = 9443;

        Self {
            state: Arc::new(Mutex::new(AppState::default())),
            ui_state: UiState::default(),
            backend: None,
            api_client: None,
            runtime,
            embedded_server: None,
            api_url: format!("http://localhost:{}", api_port),
            api_port,
            login_username: "admin".to_string(),
            login_password: "omnitak123".to_string(), // Default password for embedded server
            login_error: None,
            is_authenticated: false,
            status_message: None,
            status_message_expiry: None,
            last_refresh: std::time::Instant::now(),
            auto_start_done: false,
            command_palette: ui::command_palette::CommandPaletteState::default(),
            theme_initialized: false,
        }
    }
}

/// Application state (shared between UI and backend).
#[derive(Serialize, Deserialize)]
pub struct AppState {
    /// Server configurations
    pub servers: Vec<ServerConfig>,

    /// Connection metadata indexed by server name
    pub connections: HashMap<String, ConnectionMetadata>,

    /// Message log (recent messages)
    pub message_log: Vec<MessageLog>,

    /// Application metrics
    pub metrics: AppMetrics,

    /// Application settings
    pub settings: AppSettings,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            connections: HashMap::new(),
            message_log: Vec::new(),
            metrics: AppMetrics::default(),
            settings: AppSettings::default(),
        }
    }
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Auto-start connections on launch
    pub auto_start_connections: bool,

    /// Maximum number of messages to retain
    pub max_message_log_size: usize,

    /// Dark mode enabled
    pub dark_mode: bool,

    /// UI scale factor
    pub ui_scale: f32,

    /// Theme name (for future custom themes)
    pub theme: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_start_connections: false,
            max_message_log_size: 1000,
            dark_mode: true, // Default to dark mode
            ui_scale: 1.0,
            theme: "default".to_string(),
        }
    }
}

/// Application metrics.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppMetrics {
    /// Total messages received across all connections
    pub total_messages_received: u64,

    /// Total messages sent across all connections
    pub total_messages_sent: u64,

    /// Total bytes received
    pub total_bytes_received: u64,

    /// Total bytes sent
    pub total_bytes_sent: u64,

    /// Active connections count
    pub active_connections: usize,

    /// Failed connections count
    pub failed_connections: usize,
}

/// Message log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageLog {
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Server name
    pub server: String,

    /// Message content (truncated)
    pub content: String,

    /// Message type
    pub msg_type: String,

    /// Unique identifier
    pub uid: Option<String>,

    /// Affiliation (parsed from type)
    pub affiliation: Option<String>,

    /// Callsign/identifier
    pub callsign: Option<String>,

    /// Latitude
    pub lat: Option<f64>,

    /// Longitude
    pub lon: Option<f64>,

    /// Altitude (HAE in meters)
    pub altitude: Option<f64>,

    /// Full XML/raw content for detailed view
    pub raw_content: Option<String>,
}

/// UI state (not serialized).
pub struct UiState {
    /// Currently selected tab
    pub selected_tab: Tab,

    /// Add/Edit server dialog state
    pub server_dialog: Option<ServerDialogState>,

    /// Text filter for message log
    pub message_filter: String,

    /// Affiliation filter
    pub affiliation_filter: AffiliationFilter,

    /// Server filter
    pub server_filter: String,

    /// Auto-scroll message log
    pub auto_scroll: bool,

    /// Message details dialog
    pub message_details_dialog: Option<MessageLog>,

    /// Expanded message IDs (for collapsible cards)
    pub expanded_messages: std::collections::HashSet<String>,

    /// Plugin panel state
    pub plugin_panel: ui::plugins::PluginPanelState,

    /// Map panel state
    pub map_panel: ui::map::MapPanelState,

    /// File dialog promise for export
    pub export_promise: Option<poll_promise::Promise<Option<std::path::PathBuf>>>,

    /// File dialog promise for import
    pub import_promise: Option<poll_promise::Promise<Option<std::path::PathBuf>>>,

    /// Inline server form state (replaces modal dialog)
    pub inline_server_form: Option<ServerDialogState>,

    /// Certificate file picker promises
    pub cert_ca_promise: Option<poll_promise::Promise<Option<std::path::PathBuf>>>,
    pub cert_client_promise: Option<poll_promise::Promise<Option<std::path::PathBuf>>>,
    pub cert_key_promise: Option<poll_promise::Promise<Option<std::path::PathBuf>>>,

    /// Quick Connect wizard state
    pub quick_connect: Option<ui::quick_connect::QuickConnectState>,

    /// Certificate manager state
    pub certificate_manager: ui::certificates::CertificateManagerState,

    /// Data Package panel state
    pub datapackage_panel: ui::datapackage::DataPackagePanelState,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            selected_tab: Tab::default(),
            server_dialog: None,
            message_filter: String::new(),
            affiliation_filter: AffiliationFilter::default(),
            server_filter: String::new(),
            auto_scroll: true,
            message_details_dialog: None,
            expanded_messages: std::collections::HashSet::new(),
            plugin_panel: ui::plugins::PluginPanelState::default(),
            map_panel: ui::map::MapPanelState::default(),
            export_promise: None,
            import_promise: None,
            inline_server_form: None,
            cert_ca_promise: None,
            cert_client_promise: None,
            cert_key_promise: None,
            quick_connect: None,
            certificate_manager: ui::certificates::CertificateManagerState::default(),
            datapackage_panel: ui::datapackage::DataPackagePanelState::default(),
        }
    }
}

/// Affiliation filter options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AffiliationFilter {
    #[default]
    All,
    Pending,
    Unknown,
    AssumedFriend,
    Friend,
    Neutral,
    Suspect,
    Hostile,
}

/// Application tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Connections,
    Messages,
    Map,
    Plugins,
    DataPackages,
    Settings,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Dashboard
    }
}

/// Server add/edit dialog state.
pub struct ServerDialogState {
    /// Server being edited (None for new server)
    pub editing_index: Option<usize>,

    /// Server configuration being edited
    pub config: ServerConfig,

    /// TLS configuration dialog state
    pub tls_enabled: bool,
    pub ca_cert_path: String,
    pub client_cert_path: String,
    pub client_key_path: String,
    pub verify_cert: bool,
    pub server_name: String,
}

impl ServerDialogState {
    /// Creates a new dialog for adding a server.
    pub fn new() -> Self {
        Self {
            editing_index: None,
            config: ServerConfig::builder()
                .name("New Server")
                .host("localhost")
                .port(8089)
                .protocol(Protocol::Tls)
                .enabled(true)
                .build(),
            tls_enabled: true,
            ca_cert_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            verify_cert: true,
            server_name: String::new(),
        }
    }

    /// Creates a dialog for editing an existing server.
    pub fn edit(index: usize, config: ServerConfig) -> Self {
        let (
            tls_enabled,
            ca_cert_path,
            client_cert_path,
            client_key_path,
            verify_cert,
            server_name,
        ) = if let Some(tls) = &config.tls {
            (
                true,
                tls.ca_cert_path.to_string_lossy().to_string(),
                tls.client_cert_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                tls.client_key_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
                tls.verify_cert,
                tls.server_name.clone().unwrap_or_default(),
            )
        } else {
            (
                false,
                String::new(),
                String::new(),
                String::new(),
                true,
                String::new(),
            )
        };

        Self {
            editing_index: Some(index),
            config,
            tls_enabled,
            ca_cert_path,
            client_cert_path,
            client_key_path,
            verify_cert,
            server_name,
        }
    }

    /// Builds the final server config from the dialog state.
    pub fn build(&self) -> ServerConfig {
        let mut config = self.config.clone();

        if self.tls_enabled && !self.ca_cert_path.is_empty() {
            let mut tls = TlsConfig::new(PathBuf::from(&self.ca_cert_path));

            if !self.client_cert_path.is_empty() && !self.client_key_path.is_empty() {
                tls = tls.with_client_cert(
                    PathBuf::from(&self.client_cert_path),
                    PathBuf::from(&self.client_key_path),
                );
            }

            tls = tls.with_verify_cert(self.verify_cert);

            if !self.server_name.is_empty() {
                tls = tls.with_server_name(self.server_name.clone());
            }

            config.tls = Some(tls);
        } else {
            config.tls = None;
        }

        config
    }
}

impl OmniTakApp {
    /// Starts the embedded API server
    fn start_embedded_server(port: u16) -> Option<Child> {
        // Try to find the omnitak binary in several locations
        let possible_paths = vec![
            // Same directory as the GUI binary (for app bundle) - macOS case-insensitive fix
            std::env::current_exe()
                .ok()?
                .parent()?
                .join("omnitak-server"),
            // Development build location
            PathBuf::from("target/release/omnitak"),
            // Installed location
            PathBuf::from("/usr/local/bin/omnitak"),
        ];

        let server_path = possible_paths.iter().find(|p| p.exists())?;

        tracing::info!(
            "Starting embedded API server from: {}",
            server_path.display()
        );

        // Start the server process
        let child = Command::new(server_path)
            .arg("--bind")
            .arg(format!("0.0.0.0:{}", port))
            .arg("--admin-user")
            .arg("admin")
            .arg("--admin-password")
            .arg("omnitak123")
            .env("RUST_LOG", "info")
            .spawn()
            .ok()?;

        tracing::info!("Embedded API server started with PID: {}", child.id());

        // Give the server a moment to start
        std::thread::sleep(Duration::from_secs(2));

        Some(child)
    }

    /// Creates a new OmniTAK GUI application.
    pub fn new(cc: &eframe::CreationContext<'_>, config_path: Option<PathBuf>) -> Self {
        // Load state from config file if provided, otherwise use storage
        let state: AppState = if let Some(ref path) = config_path {
            // Try to load from config file using gui-servers.yaml format
            match import_config(path) {
                Ok(config) => {
                    tracing::info!(
                        "Loaded {} servers from {}",
                        config.servers.len(),
                        path.display()
                    );
                    AppState {
                        servers: config.servers,
                        ..Default::default()
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load config from {}: {}, using default",
                        path.display(),
                        e
                    );
                    Default::default()
                }
            }
        } else if let Some(storage) = cc.storage {
            // Fall back to eframe storage
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Default::default()
        };

        // Create shared tokio runtime
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        // Start embedded API server
        let api_port = 9443;
        let embedded_server = Self::start_embedded_server(api_port);

        if embedded_server.is_none() {
            tracing::warn!(
                "Failed to start embedded API server - will try to connect to external server"
            );
        }

        // Initialize API client (unified mode)
        let api_url = format!("http://localhost:{}", api_port);
        let api_client = match ApiClient::new(&api_url) {
            Ok(client) => {
                tracing::info!("API client initialized for {}", api_url);
                Some(client)
            }
            Err(e) => {
                tracing::error!("Failed to initialize API client: {}", e);
                None
            }
        };

        Self {
            state: Arc::new(Mutex::new(state)),
            ui_state: UiState {
                auto_scroll: true,
                ..Default::default()
            },
            backend: None, // Deprecated - using API client now
            api_client,
            runtime,
            embedded_server,
            api_url,
            api_port,
            login_username: "admin".to_string(),
            login_password: "omnitak123".to_string(), // Default password for embedded server
            login_error: None,
            is_authenticated: false,
            status_message: None,
            status_message_expiry: None,
            last_refresh: std::time::Instant::now(),
            auto_start_done: false,
            command_palette: ui::command_palette::CommandPaletteState::default(),
            theme_initialized: false,
        }
    }

    /// Auto-starts connections if enabled
    fn auto_start_connections(&mut self) {
        let should_auto_start = {
            let state = self.state.lock().unwrap();
            state.settings.auto_start_connections
        };

        if !should_auto_start {
            return;
        }

        let servers = {
            let state = self.state.lock().unwrap();
            state.servers.clone()
        };

        let enabled_servers: Vec<_> = servers.iter().filter(|s| s.enabled).cloned().collect();

        if enabled_servers.is_empty() {
            return;
        }

        tracing::info!("Auto-starting {} connection(s)", enabled_servers.len());
        self.show_status(
            format!("Auto-starting {} connection(s)...", enabled_servers.len()),
            StatusLevel::Info,
            3,
        );

        for server in enabled_servers {
            self.connect_server(server);
        }
    }

    /// Shows a status message to the user
    pub fn show_status(&mut self, message: String, level: StatusLevel, duration_secs: u64) {
        self.status_message = Some((message, level));
        self.status_message_expiry =
            Some(std::time::Instant::now() + Duration::from_secs(duration_secs));
    }

    /// Clears the status message if expired
    fn check_status_expiry(&mut self) {
        if let Some(expiry) = self.status_message_expiry {
            if std::time::Instant::now() > expiry {
                self.status_message = None;
                self.status_message_expiry = None;
            }
        }
    }

    // Deprecated: process_backend_events - now using API refresh instead

    /// Connects to a server (via API)
    pub fn connect_server(&mut self, config: ServerConfig) {
        let api_client = match &self.api_client {
            Some(client) => client,
            None => {
                self.show_status(
                    "API client not available".to_string(),
                    StatusLevel::Error,
                    5,
                );
                return;
            }
        };

        let connection_type = match config.protocol {
            Protocol::Tcp => ConnectionType::TcpClient,
            Protocol::Udp => ConnectionType::Udp,
            Protocol::Tls => ConnectionType::TlsClient,
            Protocol::WebSocket => ConnectionType::TcpClient, // WebSocket uses TCP
        };

        let request = crate::api_client::CreateConnectionRequest {
            name: config.name.clone(),
            connection_type,
            address: config.host.clone(),
            port: config.port,
            priority: None,
        };

        match self.runtime.block_on(api_client.create_connection(request)) {
            Ok(id) => {
                tracing::info!("Created connection {} with ID: {}", config.name, id);
                self.show_status(
                    format!("Connected to {}", config.name),
                    StatusLevel::Success,
                    3,
                );
                self.refresh_from_api();
            }
            Err(e) => {
                tracing::error!("Failed to create connection: {}", e);
                self.show_status(
                    format!("Failed to connect to {}: {}", config.name, e),
                    StatusLevel::Error,
                    5,
                );
            }
        }
    }

    /// Disconnects from a server (via API)
    pub fn disconnect_server(&mut self, server_name: String) {
        let api_client = match &self.api_client {
            Some(client) => client,
            None => {
                self.show_status(
                    "API client not available".to_string(),
                    StatusLevel::Error,
                    5,
                );
                return;
            }
        };

        // Find the connection ID by name
        let connections = match self.runtime.block_on(api_client.list_connections()) {
            Ok(conns) => conns,
            Err(e) => {
                tracing::error!("Failed to list connections: {}", e);
                self.show_status(
                    format!("Failed to disconnect: {}", e),
                    StatusLevel::Error,
                    5,
                );
                return;
            }
        };

        let connection_id = connections
            .iter()
            .find(|c| c.name == server_name)
            .map(|c| c.id.clone());

        if let Some(id) = connection_id {
            match self.runtime.block_on(api_client.delete_connection(&id)) {
                Ok(()) => {
                    tracing::info!("Deleted connection: {}", server_name);
                    self.show_status(
                        format!("Disconnected from {}", server_name),
                        StatusLevel::Success,
                        3,
                    );
                    self.refresh_from_api();
                }
                Err(e) => {
                    tracing::error!("Failed to delete connection: {}", e);
                    self.show_status(
                        format!("Failed to disconnect from {}: {}", server_name, e),
                        StatusLevel::Error,
                        5,
                    );
                }
            }
        } else {
            self.show_status(
                format!("Connection {} not found", server_name),
                StatusLevel::Error,
                5,
            );
        }
    }

    /// Adds a new server configuration.
    pub fn add_server(&mut self, config: ServerConfig) {
        let mut state = self.state.lock().unwrap();
        state.servers.push(config);
    }

    /// Updates an existing server configuration.
    pub fn update_server(&mut self, index: usize, config: ServerConfig) {
        let mut state = self.state.lock().unwrap();
        if index < state.servers.len() {
            state.servers[index] = config;
        }
    }

    /// Removes a server configuration.
    pub fn remove_server(&mut self, index: usize) {
        let mut state = self.state.lock().unwrap();
        if index < state.servers.len() {
            state.servers.remove(index);
        }
    }

    /// Updates connection metadata.
    pub fn update_connection_metadata(
        &mut self,
        server_name: String,
        metadata: ConnectionMetadata,
    ) {
        let mut state = self.state.lock().unwrap();
        state.connections.insert(server_name, metadata);

        // Update metrics
        state.metrics.active_connections = state
            .connections
            .values()
            .filter(|m| m.status == ServerStatus::Connected)
            .count();
        state.metrics.failed_connections = state
            .connections
            .values()
            .filter(|m| m.status == ServerStatus::Failed)
            .count();

        state.metrics.total_messages_received = state
            .connections
            .values()
            .map(|m| m.messages_received)
            .sum();
        state.metrics.total_messages_sent =
            state.connections.values().map(|m| m.messages_sent).sum();
        state.metrics.total_bytes_received =
            state.connections.values().map(|m| m.bytes_received).sum();
        state.metrics.total_bytes_sent = state.connections.values().map(|m| m.bytes_sent).sum();
    }

    /// Adds a message to the log.
    pub fn add_message_log(&mut self, log: MessageLog) {
        let mut state = self.state.lock().unwrap();
        state.message_log.push(log);

        // Keep only last N messages based on retention policy
        let max_size = state.settings.max_message_log_size;
        let len = state.message_log.len();
        if len > max_size {
            state.message_log.drain(0..len - max_size);
        }
    }

    /// Exports configuration to a file
    pub fn export_config(&self, path: &str) -> anyhow::Result<()> {
        let state = self.state.lock().unwrap();
        let config = ConfigFile::new(state.servers.clone());

        // Validate before exporting
        if let Err(errors) = config.validate() {
            return Err(anyhow::anyhow!(
                "Configuration validation failed: {:?}",
                errors
            ));
        }

        export_config(&config, path)?;
        Ok(())
    }

    /// Imports configuration from a file
    pub fn import_config(&mut self, path: &str) -> anyhow::Result<usize> {
        let config = import_config(path)?;

        // Validate before importing
        if let Err(errors) = config.validate() {
            return Err(anyhow::anyhow!(
                "Configuration validation failed: {:?}",
                errors
            ));
        }

        let mut state = self.state.lock().unwrap();
        let imported_count = config.servers.len();
        state.servers.extend(config.servers);

        Ok(imported_count)
    }

    /// Replaces all server configurations with imported ones
    pub fn import_config_replace(&mut self, path: &str) -> anyhow::Result<usize> {
        let config = import_config(path)?;

        // Validate before importing
        if let Err(errors) = config.validate() {
            return Err(anyhow::anyhow!(
                "Configuration validation failed: {:?}",
                errors
            ));
        }

        let mut state = self.state.lock().unwrap();
        let imported_count = config.servers.len();
        state.servers = config.servers;

        Ok(imported_count)
    }

    /// Shows the login screen
    fn show_login_screen(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("🛰️ OmniTAK Login");
                ui.add_space(40.0);

                if let Some(error) = &self.login_error {
                    ui.colored_label(egui::Color32::RED, format!("Error: {}", error));
                    ui.add_space(10.0);
                }

                ui.horizontal(|ui| {
                    ui.label("API URL:");
                    ui.add_space(10.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.api_url)
                            .min_size(egui::vec2(300.0, 0.0)),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("Username:");
                    ui.add_space(10.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.login_username)
                            .min_size(egui::vec2(300.0, 0.0)),
                    );
                });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("Password:");
                    ui.add_space(10.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.login_password)
                            .password(true)
                            .min_size(egui::vec2(300.0, 0.0)),
                    );
                });
                ui.add_space(20.0);

                if ui.button("Login").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.perform_login();
                }
            });
        });
    }

    /// Performs login to the API
    fn perform_login(&mut self) {
        let api_client = match &mut self.api_client {
            Some(client) => client,
            None => {
                self.login_error = Some("API client not initialized".to_string());
                return;
            }
        };

        let username = self.login_username.clone();
        let password = self.login_password.clone();

        // Use shared tokio runtime to make the async call
        match self
            .runtime
            .block_on(api_client.login(&username, &password))
        {
            Ok(()) => {
                self.is_authenticated = true;
                self.login_error = None;
                self.login_password.clear(); // Clear password from memory
                tracing::info!("Successfully logged in as {}", username);

                // Initial data refresh
                self.refresh_from_api();
            }
            Err(e) => {
                self.login_error = Some(e.to_string());
                tracing::error!("Login failed: {}", e);
            }
        }
    }

    /// Refreshes data from the API
    pub fn refresh_from_api(&mut self) {
        let api_client = match &self.api_client {
            Some(client) => client,
            None => return,
        };

        // Get system status
        if let Ok(status) = self.runtime.block_on(api_client.get_status()) {
            let mut state = self.state.lock().unwrap();
            state.metrics.active_connections = status.active_connections;
            state.metrics.total_messages_received = status.messages_processed;
            // Update other metrics as needed
        }

        // Get connections
        if let Ok(connections) = self.runtime.block_on(api_client.list_connections()) {
            let mut state = self.state.lock().unwrap();
            state.connections.clear();

            for conn in connections {
                let metadata = ConnectionMetadata {
                    connection_id: ConnectionId::new(),
                    server_name: conn.name.clone(),
                    status: if conn.status == "connected" {
                        ServerStatus::Connected
                    } else {
                        ServerStatus::Disconnected
                    },
                    connected_at: None,
                    disconnected_at: None,
                    reconnect_attempts: 0,
                    messages_received: conn.messages_received,
                    messages_sent: conn.messages_sent,
                    bytes_received: 0,
                    bytes_sent: 0,
                    last_error: None,
                };
                state.connections.insert(conn.name.clone(), metadata);

                // Also add to servers list if not already there
                if !state.servers.iter().any(|s| s.name == conn.name) {
                    let protocol = match conn.connection_type {
                        ConnectionType::TcpClient | ConnectionType::TcpServer => Protocol::Tcp,
                        ConnectionType::Udp => Protocol::Udp,
                        ConnectionType::TlsClient | ConnectionType::TlsServer => Protocol::Tls,
                        ConnectionType::Multicast => Protocol::Udp,
                    };

                    let server_config = ServerConfig {
                        name: conn.name.clone(),
                        host: conn.address.clone(),
                        port: conn.port,
                        protocol,
                        tls: None,
                        reconnect: omnitak_core::types::ReconnectConfig::default(),
                        connect_timeout: Duration::from_secs(10),
                        read_timeout: Duration::from_secs(30),
                        enabled: true,
                        tags: vec![],
                    };
                    state.servers.push(server_config);
                }
            }
        }
    }
}

impl Drop for OmniTakApp {
    fn drop(&mut self) {
        // Gracefully shut down the embedded server
        if let Some(mut server) = self.embedded_server.take() {
            tracing::info!("Shutting down embedded API server (PID: {})", server.id());
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

impl eframe::App for OmniTakApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Initialize theme on first run
        if !self.theme_initialized {
            let dark_mode = {
                let state = self.state.lock().unwrap();
                state.settings.dark_mode
            };
            ui::command_palette::apply_theme(ctx, dark_mode);

            let ui_scale = {
                let state = self.state.lock().unwrap();
                state.settings.ui_scale
            };
            if (ui_scale - 1.0).abs() > 0.01 {
                ctx.set_pixels_per_point(ui_scale);
            }
            self.theme_initialized = true;
        }

        // Handle global keyboard shortcuts
        ui::command_palette::handle_keyboard_shortcuts(ctx, self);

        // Check status message expiry
        self.check_status_expiry();

        // Show login screen if not authenticated
        if !self.is_authenticated {
            self.show_login_screen(ctx);
            return;
        }

        // Auto-start connections on first update after authentication
        if !self.auto_start_done {
            self.auto_start_connections();
            self.auto_start_done = true;
        }

        // Refresh data from API every 5 seconds
        if self.last_refresh.elapsed() > Duration::from_secs(5) {
            self.refresh_from_api();
            self.last_refresh = std::time::Instant::now();
        }

        // Top panel with tabs
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.heading("🎯 OmniTAK");

                ui.separator();

                if ui
                    .selectable_label(self.ui_state.selected_tab == Tab::Dashboard, "📊 Dashboard")
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Dashboard;
                }

                if ui
                    .selectable_label(
                        self.ui_state.selected_tab == Tab::Connections,
                        "🔌 Connections",
                    )
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Connections;
                }

                if ui
                    .selectable_label(self.ui_state.selected_tab == Tab::Messages, "💬 Messages")
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Messages;
                }

                if ui
                    .selectable_label(self.ui_state.selected_tab == Tab::Map, "🗺 Map")
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Map;
                }

                if ui
                    .selectable_label(self.ui_state.selected_tab == Tab::Plugins, "🔌 Plugins")
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Plugins;
                }

                if ui
                    .selectable_label(
                        self.ui_state.selected_tab == Tab::DataPackages,
                        "📦 Packages",
                    )
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::DataPackages;
                }

                if ui
                    .selectable_label(self.ui_state.selected_tab == Tab::Settings, "⚙ Settings")
                    .clicked()
                {
                    self.ui_state.selected_tab = Tab::Settings;
                }

                // Spacer to push shortcuts help to the right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Ctrl+K: Command Palette")
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                });
            });
        });

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| match self.ui_state.selected_tab {
            Tab::Dashboard => ui::dashboard::show(ui, self),
            Tab::Connections => ui::connections::show(ui, self),
            Tab::Messages => ui::messages::show(ui, &self.state, &mut self.ui_state),
            Tab::Map => ui::map::show(ui, &self.state, &mut self.ui_state.map_panel),
            Tab::Plugins => {
                if let Some((message, level)) = ui::plugins::render_plugins_panel(
                    ui,
                    &self.state,
                    &mut self.ui_state.plugin_panel,
                    self.api_client.as_ref(),
                ) {
                    self.show_status(message, level, 5);
                }
            }
            Tab::DataPackages => {
                if let Some((message, level)) = ui::datapackage::render_datapackage_panel(
                    ui,
                    &self.state,
                    &mut self.ui_state.datapackage_panel,
                ) {
                    self.show_status(message, level, 5);
                }
            }
            Tab::Settings => ui::settings::show(ui, self),
        });

        // Bottom status bar
        if let Some((message, level)) = &self.status_message {
            egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (icon, color) = match level {
                        StatusLevel::Info => ("ℹ", egui::Color32::LIGHT_BLUE),
                        StatusLevel::Success => ("✓", egui::Color32::GREEN),
                        StatusLevel::Warning => ("⚠", egui::Color32::YELLOW),
                        StatusLevel::Error => ("✗", egui::Color32::RED),
                    };
                    ui.colored_label(color, icon);
                    ui.label(message);
                });
            });
        }

        // Render command palette overlay (on top of everything)
        if let Some(command_id) =
            ui::command_palette::render_command_palette(ctx, &mut self.command_palette)
        {
            ui::command_palette::execute_command(self, &command_id, ctx);
        }

        // Request repaint for real-time updates
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let state = self.state.lock().unwrap();
        eframe::set_value(storage, eframe::APP_KEY, &*state);
    }
}

/// Formats bytes into a human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// Formats a duration into a human-readable string.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}
