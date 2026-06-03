//! Quick Connect wizard for easy server setup
//!
//! Provides a "no wrong door" approach to connecting to TAK servers by:
//! - Auto-detecting certificate formats (ZIP, P12, PEM)
//! - Discovering TAK servers on the local network
//! - Extracting server info from certificate bundles
//! - Minimal configuration required from user

use crate::{OmniTakApp, StatusLevel};
use eframe::egui;
use omnitak_cert::{
    extract_zip_certificates, scan_directory_for_certificates, ExtractedCertificates,
};
use omnitak_core::types::{Protocol, ReconnectConfig, ServerConfig, TlsConfig};
use std::path::PathBuf;
use std::time::Duration;

/// State for the Quick Connect wizard
pub struct QuickConnectState {
    /// Current step in the wizard
    pub step: WizardStep,

    /// Certificate file path (user selected)
    pub cert_file_path: String,

    /// Password for P12 files
    pub p12_password: String,

    /// Show password in plain text
    pub show_password: bool,

    /// Extracted certificate information
    pub extracted_certs: Option<ExtractedCertificates>,

    /// Output directory for extracted certs
    pub cert_output_dir: PathBuf,

    /// Server configuration being built
    pub server_config: ServerConfig,

    /// Error message to display
    pub error_message: Option<String>,

    /// Success message
    pub success_message: Option<String>,

    /// File picker promise
    pub file_picker_promise: Option<poll_promise::Promise<Option<PathBuf>>>,

    /// Discovered servers from mDNS
    pub discovered_servers: Vec<DiscoveredServer>,

    /// Is currently discovering?
    pub is_discovering: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WizardStep {
    #[default]
    SelectSource,
    ExtractCerts,
    ConfigureServer,
    Review,
}

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub service_type: String,
}

impl QuickConnectState {
    pub fn new() -> Self {
        let cert_output_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("certs")
            .join("extracted");

        Self {
            step: WizardStep::default(),
            cert_file_path: String::new(),
            p12_password: String::new(),
            show_password: false,
            extracted_certs: None,
            cert_output_dir,
            server_config: ServerConfig {
                name: "My TAK Server".to_string(),
                host: String::new(),
                port: 8089,
                protocol: Protocol::Tls,
                enabled: true,
                tags: vec![],
                tls: None,
                connect_timeout: Duration::from_secs(10),
                read_timeout: Duration::from_secs(30),
                reconnect: ReconnectConfig::default(),
            },
            error_message: None,
            success_message: None,
            file_picker_promise: None,
            discovered_servers: vec![],
            is_discovering: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for QuickConnectState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the Quick Connect wizard
pub fn render_quick_connect(
    ui: &mut egui::Ui,
    app: &mut OmniTakApp,
    state: &mut QuickConnectState,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    ui.heading("Quick Connect Wizard");
    ui.add_space(10.0);

    // Progress indicator
    render_progress_indicator(ui, state.step);
    ui.add_space(15.0);

    // Handle file picker promise
    if let Some(promise) = &state.file_picker_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                state.cert_file_path = path.to_string_lossy().to_string();
                // Auto-advance to extraction if we got a file
                if !state.cert_file_path.is_empty() {
                    state.step = WizardStep::ExtractCerts;
                }
            }
            state.file_picker_promise = None;
        }
    }

    // Render current step
    match state.step {
        WizardStep::SelectSource => {
            render_select_source_step(ui, state);
        }
        WizardStep::ExtractCerts => {
            if let Some(msg) = render_extract_certs_step(ui, state) {
                status_message = Some(msg);
            }
        }
        WizardStep::ConfigureServer => {
            render_configure_server_step(ui, state);
        }
        WizardStep::Review => {
            if let Some(msg) = render_review_step(ui, app, state) {
                status_message = Some(msg);
            }
        }
    }

    // Error display
    if let Some(error) = &state.error_message {
        ui.add_space(10.0);
        ui.colored_label(egui::Color32::RED, format!("⚠️ {}", error));
    }

    // Success display
    if let Some(success) = &state.success_message {
        ui.add_space(10.0);
        ui.colored_label(egui::Color32::GREEN, format!("✓ {}", success));
    }

    status_message
}

fn render_progress_indicator(ui: &mut egui::Ui, current_step: WizardStep) {
    ui.horizontal(|ui| {
        let steps = [
            ("1. Source", WizardStep::SelectSource),
            ("2. Extract", WizardStep::ExtractCerts),
            ("3. Configure", WizardStep::ConfigureServer),
            ("4. Review", WizardStep::Review),
        ];

        for (i, (label, step)) in steps.iter().enumerate() {
            if i > 0 {
                ui.label(" → ");
            }

            let color = if *step == current_step {
                egui::Color32::from_rgb(100, 200, 100)
            } else if (*step as u8) < (current_step as u8) {
                egui::Color32::GRAY
            } else {
                egui::Color32::DARK_GRAY
            };

            ui.colored_label(color, *label);
        }
    });
}

fn render_select_source_step(ui: &mut egui::Ui, state: &mut QuickConnectState) {
    ui.label(
        egui::RichText::new("How do you want to connect?")
            .size(16.0)
            .strong(),
    );
    ui.add_space(10.0);

    // Option 1: Certificate file
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(40))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("📁 Import Certificates")
                    .size(14.0)
                    .strong(),
            );
            ui.add_space(5.0);
            ui.label("Upload a ZIP file, P12/PFX file, or select a certificate folder");
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut state.cert_file_path);

                if ui.button("Browse...").clicked() && state.file_picker_promise.is_none() {
                    state.file_picker_promise = Some(poll_promise::Promise::spawn_thread(
                        "cert_file_picker",
                        || {
                            rfd::FileDialog::new()
                                .add_filter(
                                    "Certificate Files",
                                    &["zip", "p12", "pfx", "pem", "crt"],
                                )
                                .add_filter("All Files", &["*"])
                                .pick_file()
                        },
                    ));
                }
            });

            ui.add_space(10.0);

            if !state.cert_file_path.is_empty() {
                if ui.button("➡️ Extract & Continue").clicked() {
                    state.step = WizardStep::ExtractCerts;
                    state.error_message = None;
                }
            }
        });

    ui.add_space(15.0);

    // Option 2: Discover servers
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(40))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("🔍 Discover TAK Servers")
                    .size(14.0)
                    .strong(),
            );
            ui.add_space(5.0);
            ui.label("Find TAK servers on your local network via mDNS");
            ui.add_space(10.0);

            if state.is_discovering {
                ui.label("Scanning network...");
                ui.spinner();
            } else {
                if ui.button("🔄 Scan Network").clicked() {
                    state.is_discovering = true;
                    // TODO: Integrate with discovery service
                    // For now, show placeholder
                    state.discovered_servers = vec![DiscoveredServer {
                        name: "Local TAK Server".to_string(),
                        host: "127.0.0.1".to_string(),
                        port: 8089,
                        service_type: "TLS".to_string(),
                    }];
                    state.is_discovering = false;
                }
            }

            if !state.discovered_servers.is_empty() {
                ui.add_space(10.0);
                ui.label("Found servers:");

                for server in &state.discovered_servers {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "• {} ({}:{})",
                            server.name, server.host, server.port
                        ));
                        if ui.small_button("Select").clicked() {
                            state.server_config.name = server.name.clone();
                            state.server_config.host = server.host.clone();
                            state.server_config.port = server.port;
                            state.step = WizardStep::ConfigureServer;
                        }
                    });
                }
            }
        });

    ui.add_space(15.0);

    // Option 3: Manual entry
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(40))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("⌨️ Manual Configuration")
                    .size(14.0)
                    .strong(),
            );
            ui.add_space(5.0);
            ui.label("Enter server details manually (advanced)");
            ui.add_space(10.0);

            if ui.button("➡️ Configure Manually").clicked() {
                state.step = WizardStep::ConfigureServer;
            }
        });
}

fn render_extract_certs_step(
    ui: &mut egui::Ui,
    state: &mut QuickConnectState,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    ui.label(
        egui::RichText::new("Extracting Certificates")
            .size(16.0)
            .strong(),
    );
    ui.add_space(10.0);

    ui.label(format!("File: {}", state.cert_file_path));
    ui.add_space(10.0);

    // Check if we need to extract
    if state.extracted_certs.is_none() {
        let path = PathBuf::from(&state.cert_file_path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Ensure output directory exists
        if let Err(e) = std::fs::create_dir_all(&state.cert_output_dir) {
            state.error_message = Some(format!("Failed to create output directory: {}", e));
        } else {
            match ext.as_str() {
                "zip" => match extract_zip_certificates(&path, &state.cert_output_dir) {
                    Ok(extracted) => {
                        state.extracted_certs = Some(extracted);
                        state.error_message = None;
                        status_message = Some((
                            "Certificates extracted successfully".to_string(),
                            StatusLevel::Success,
                        ));
                    }
                    Err(e) => {
                        state.error_message = Some(format!("Failed to extract ZIP: {}", e));
                    }
                },
                "p12" | "pfx" => {
                    // P12 file - we'll handle it directly in the next step
                    state.extracted_certs = Some(ExtractedCertificates {
                        ca_cert_path: None,
                        client_cert_path: None,
                        client_key_path: None,
                        p12_path: Some(path.clone()),
                        server_info: None,
                        all_files: vec![path],
                    });
                }
                "pem" | "crt" | "cer" => {
                    // Single cert file - scan the directory it's in
                    if let Some(parent) = path.parent() {
                        match scan_directory_for_certificates(parent) {
                            Ok(extracted) => {
                                state.extracted_certs = Some(extracted);
                            }
                            Err(e) => {
                                state.error_message =
                                    Some(format!("Failed to scan directory: {}", e));
                            }
                        }
                    }
                }
                _ => {
                    state.error_message = Some(format!("Unsupported file type: {}", ext));
                }
            }
        }
    }

    // Show extraction results
    if let Some(extracted) = &state.extracted_certs {
        // Clone extracted data for display to avoid borrow conflicts
        let p12_name = extracted.p12_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let ca_name = extracted.ca_cert_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let cert_name = extracted.client_cert_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let key_name = extracted.client_key_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let server_info = extracted.server_info.clone();
        let has_p12 = extracted.p12_path.is_some();

        // Clone paths for TLS config construction
        let p12_path = extracted.p12_path.clone();
        let ca_cert_path = extracted.ca_cert_path.clone();
        let client_cert_path = extracted.client_cert_path.clone();
        let client_key_path = extracted.client_key_path.clone();

        egui::Frame::NONE
            .fill(egui::Color32::from_gray(35))
            .corner_radius(5.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Found Certificates:").strong());
                ui.add_space(5.0);

                if let Some(name) = &p12_name {
                    ui.colored_label(egui::Color32::GREEN, format!("✓ P12/PFX Bundle: {}", name));
                }

                if let Some(name) = &ca_name {
                    ui.colored_label(egui::Color32::GREEN, format!("✓ CA Certificate: {}", name));
                }

                if let Some(name) = &cert_name {
                    ui.colored_label(
                        egui::Color32::GREEN,
                        format!("✓ Client Certificate: {}", name),
                    );
                }

                if let Some(name) = &key_name {
                    ui.colored_label(egui::Color32::GREEN, format!("✓ Client Key: {}", name));
                }

                if let Some(info) = &server_info {
                    ui.add_space(5.0);
                    ui.label(egui::RichText::new("Server Info Found:").strong());
                    if let Some(host) = &info.host {
                        ui.label(format!("  Host: {}", host));
                    }
                    if let Some(port) = info.port {
                        ui.label(format!("  Port: {}", port));
                    }
                }
            });

        // Update server config from extracted info
        if let Some(info) = &server_info {
            if let Some(host) = &info.host {
                state.server_config.host = host.clone();
            }
            if let Some(port) = info.port {
                state.server_config.port = port;
            }
        }

        ui.add_space(10.0);

        // Password input for P12
        if has_p12 {
            ui.label("P12 Password (if required):");
            ui.horizontal(|ui| {
                if state.show_password {
                    ui.text_edit_singleline(&mut state.p12_password);
                } else {
                    ui.add(egui::TextEdit::singleline(&mut state.p12_password).password(true));
                }
                ui.checkbox(&mut state.show_password, "Show");
            });
            ui.add_space(10.0);
        }

        // Navigation
        let mut go_back = false;
        let mut go_forward = false;

        ui.horizontal(|ui| {
            if ui.button("⬅️ Back").clicked() {
                go_back = true;
            }

            if ui.button("➡️ Configure Server").clicked() {
                go_forward = true;
            }
        });

        if go_back {
            state.step = WizardStep::SelectSource;
            state.extracted_certs = None;
        }

        if go_forward {
            // Build TLS config from extracted certs
            if let Some(p12) = &p12_path {
                state.server_config.tls = Some(TlsConfig {
                    ca_cert_path: ca_cert_path.clone().unwrap_or_else(|| PathBuf::new()),
                    client_cert_path: Some(p12.clone()),
                    client_key_path: None, // Embedded in P12
                    verify_cert: true,
                    server_name: None,
                });
            } else if let (Some(cert), Some(key)) = (&client_cert_path, &client_key_path) {
                state.server_config.tls = Some(TlsConfig {
                    ca_cert_path: ca_cert_path.clone().unwrap_or_else(|| PathBuf::new()),
                    client_cert_path: Some(cert.clone()),
                    client_key_path: Some(key.clone()),
                    verify_cert: true,
                    server_name: None,
                });
            }

            state.step = WizardStep::ConfigureServer;
        }
    }

    status_message
}

fn render_configure_server_step(ui: &mut egui::Ui, state: &mut QuickConnectState) {
    ui.label(egui::RichText::new("Configure Server").size(16.0).strong());
    ui.add_space(10.0);

    egui::Grid::new("server_config_grid")
        .num_columns(2)
        .spacing([10.0, 8.0])
        .show(ui, |ui| {
            ui.label("Server Name:");
            ui.text_edit_singleline(&mut state.server_config.name);
            ui.end_row();

            ui.label("Host/IP:");
            ui.text_edit_singleline(&mut state.server_config.host);
            ui.end_row();

            ui.label("Port:");
            let mut port_str = state.server_config.port.to_string();
            if ui.text_edit_singleline(&mut port_str).changed() {
                if let Ok(port) = port_str.parse::<u16>() {
                    state.server_config.port = port;
                }
            }
            ui.end_row();

            ui.label("Protocol:");
            egui::ComboBox::from_id_salt("quick_connect_protocol")
                .selected_text(format!("{:?}", state.server_config.protocol))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.server_config.protocol, Protocol::Tcp, "TCP");
                    ui.selectable_value(&mut state.server_config.protocol, Protocol::Tls, "TLS");
                    ui.selectable_value(&mut state.server_config.protocol, Protocol::Udp, "UDP");
                });
            ui.end_row();

            ui.label("Auto-connect:");
            ui.checkbox(&mut state.server_config.enabled, "Enable on startup");
            ui.end_row();
        });

    ui.add_space(15.0);

    // TLS configuration summary
    if let Some(tls) = &state.server_config.tls {
        // Clone display info to avoid borrow conflicts
        let ca_display = if tls.ca_cert_path.as_os_str().len() > 0 {
            Some(
                tls.ca_cert_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            )
        } else {
            None
        };
        let cert_display = tls.client_cert_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let key_display = tls.client_key_path.as_ref().map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let current_verify = tls.verify_cert;

        let mut new_verify = current_verify;

        egui::Frame::NONE
            .fill(egui::Color32::from_gray(35))
            .corner_radius(5.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("🔒 TLS Configuration").strong());
                ui.add_space(5.0);

                if let Some(ca) = &ca_display {
                    ui.label(format!("CA: {}", ca));
                }
                if let Some(cert) = &cert_display {
                    ui.label(format!("Client Cert: {}", cert));
                }
                if let Some(key) = &key_display {
                    ui.label(format!("Client Key: {}", key));
                }
                ui.checkbox(&mut new_verify, "Verify server certificate");
            });

        // Update verify_cert if changed
        if new_verify != current_verify {
            if let Some(tls) = &mut state.server_config.tls {
                tls.verify_cert = new_verify;
            }
        }
    }

    ui.add_space(15.0);

    // Navigation
    ui.horizontal(|ui| {
        if ui.button("⬅️ Back").clicked() {
            if state.extracted_certs.is_some() {
                state.step = WizardStep::ExtractCerts;
            } else {
                state.step = WizardStep::SelectSource;
            }
        }

        let can_proceed = !state.server_config.host.is_empty() && state.server_config.port > 0;
        ui.add_enabled_ui(can_proceed, |ui| {
            if ui.button("➡️ Review & Connect").clicked() {
                state.step = WizardStep::Review;
            }
        });

        if !can_proceed {
            ui.colored_label(egui::Color32::YELLOW, "Please enter host and port");
        }
    });
}

fn render_review_step(
    ui: &mut egui::Ui,
    app: &mut OmniTakApp,
    state: &mut QuickConnectState,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    ui.label(egui::RichText::new("Review & Connect").size(16.0).strong());
    ui.add_space(10.0);

    egui::Frame::NONE
        .fill(egui::Color32::from_gray(35))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Server Configuration:").strong());
            ui.add_space(10.0);

            ui.label(format!("Name: {}", state.server_config.name));
            ui.label(format!("Host: {}", state.server_config.host));
            ui.label(format!("Port: {}", state.server_config.port));
            ui.label(format!("Protocol: {:?}", state.server_config.protocol));
            ui.label(format!(
                "Auto-connect: {}",
                if state.server_config.enabled {
                    "Yes"
                } else {
                    "No"
                }
            ));

            if state.server_config.tls.is_some() {
                ui.add_space(5.0);
                ui.colored_label(egui::Color32::GREEN, "🔒 TLS Enabled");
            }
        });

    ui.add_space(15.0);

    // Navigation
    ui.horizontal(|ui| {
        if ui.button("⬅️ Back").clicked() {
            state.step = WizardStep::ConfigureServer;
        }

        if ui.button("✅ Add Server & Connect").clicked() {
            // Add the server
            app.add_server(state.server_config.clone());

            // Connect to it
            app.connect_server(state.server_config.clone());

            status_message = Some((
                format!(
                    "Server '{}' added and connecting...",
                    state.server_config.name
                ),
                StatusLevel::Success,
            ));

            state.success_message = Some("Server added successfully!".to_string());

            // Reset wizard for next use
            state.reset();
        }

        if ui.button("💾 Add Server Only").clicked() {
            // Just add without connecting
            app.add_server(state.server_config.clone());

            status_message = Some((
                format!("Server '{}' added", state.server_config.name),
                StatusLevel::Success,
            ));

            state.success_message = Some("Server added!".to_string());
            state.reset();
        }
    });

    status_message
}
