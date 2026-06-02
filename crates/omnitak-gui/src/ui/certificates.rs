//! Certificate Management UI
//!
//! Provides visual certificate chain viewer, expiration warnings, and renewal workflows.

use eframe::egui;
use omnitak_cert::{CertificateChainInfo, CertificateInfo, ExpirationStatus};
use std::path::PathBuf;

/// State for certificate management panel
#[derive(Default)]
pub struct CertificateManagerState {
    /// Currently loaded certificate chain
    pub chain_info: Option<CertificateChainInfo>,
    /// Selected certificate for detailed view
    pub selected_cert: Option<usize>,
    /// File picker promise
    pub file_picker_promise: Option<poll_promise::Promise<Option<PathBuf>>>,
    /// Password for PKCS#12 files
    pub password: String,
    /// Show password input
    pub show_password_input: bool,
    /// Pending file to load (waiting for password)
    pub pending_file: Option<PathBuf>,
    /// Error message
    pub error_message: Option<String>,
}

/// Render the certificate manager panel
pub fn render_certificate_manager(
    ui: &mut egui::Ui,
    state: &mut CertificateManagerState,
) -> Option<(String, crate::StatusLevel)> {
    let mut status_message = None;

    // Handle file picker result
    if let Some(promise) = &state.file_picker_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                if ext == "p12" || ext == "pfx" {
                    // Need password for PKCS#12
                    state.pending_file = Some(path.clone());
                    state.show_password_input = true;
                } else {
                    // Try to load PEM directly
                    match load_certificate_from_file(&path, None) {
                        Ok(chain) => {
                            state.chain_info = Some(chain);
                            state.error_message = None;
                            status_message = Some((
                                format!("Loaded certificate from {}", path.display()),
                                crate::StatusLevel::Success,
                            ));
                        }
                        Err(e) => {
                            state.error_message = Some(e.to_string());
                        }
                    }
                }
            }
            state.file_picker_promise = None;
        }
    }

    // Password input dialog
    if state.show_password_input {
        egui::Window::new("PKCS#12 Password")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("Enter password for PKCS#12 file:");
                ui.add_space(10.0);
                ui.add(
                    egui::TextEdit::singleline(&mut state.password)
                        .password(true)
                        .hint_text("Password"),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("Load").clicked() {
                        if let Some(path) = &state.pending_file {
                            let password = if state.password.is_empty() {
                                None
                            } else {
                                Some(state.password.as_str())
                            };

                            match load_certificate_from_file(path, password) {
                                Ok(chain) => {
                                    state.chain_info = Some(chain);
                                    state.error_message = None;
                                    status_message = Some((
                                        format!("Loaded certificate from {}", path.display()),
                                        crate::StatusLevel::Success,
                                    ));
                                }
                                Err(e) => {
                                    state.error_message = Some(e.to_string());
                                }
                            }
                        }
                        state.show_password_input = false;
                        state.password.clear();
                        state.pending_file = None;
                    }

                    if ui.button("Cancel").clicked() {
                        state.show_password_input = false;
                        state.password.clear();
                        state.pending_file = None;
                    }
                });
            });
    }

    ui.heading("Certificate Manager");
    ui.add_space(10.0);

    // Load certificate button
    ui.horizontal(|ui| {
        if ui.button("Load Certificate File").clicked() && state.file_picker_promise.is_none() {
            state.file_picker_promise =
                Some(poll_promise::Promise::spawn_thread("cert_picker", || {
                    rfd::FileDialog::new()
                        .add_filter("Certificates", &["pem", "crt", "cer", "p12", "pfx"])
                        .pick_file()
                }));
        }

        if state.chain_info.is_some() && ui.button("Clear").clicked() {
            state.chain_info = None;
            state.selected_cert = None;
        }
    });

    // Error message
    if let Some(error) = &state.error_message {
        ui.add_space(5.0);
        ui.colored_label(egui::Color32::RED, format!("Error: {}", error));
    }

    ui.add_space(15.0);

    // Display certificate chain
    if let Some(chain) = &state.chain_info {
        render_certificate_chain(ui, chain, &mut state.selected_cert);
    } else {
        ui.label(
            "No certificate loaded. Click 'Load Certificate File' to view certificate details.",
        );
    }

    status_message
}

/// Render the certificate chain visualization
fn render_certificate_chain(
    ui: &mut egui::Ui,
    chain: &CertificateChainInfo,
    selected: &mut Option<usize>,
) {
    // Chain summary
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.heading("Certificate Chain");
            ui.add_space(5.0);

            // Overall chain status
            let (status_icon, status_color, status_text) = if !chain.chain_valid {
                ("X", egui::Color32::RED, "INVALID")
            } else if let Some(days) = chain.days_until_chain_expiry {
                if days < 0 {
                    ("X", egui::Color32::RED, "EXPIRED")
                } else if days <= 30 {
                    ("!", egui::Color32::YELLOW, "EXPIRING SOON")
                } else {
                    ("OK", egui::Color32::GREEN, "VALID")
                }
            } else {
                ("?", egui::Color32::GRAY, "UNKNOWN")
            };

            ui.horizontal(|ui| {
                ui.colored_label(status_color, status_icon);
                ui.label(format!("Chain Status: {}", status_text));
            });

            if let Some(expiry) = &chain.earliest_expiry {
                ui.label(format!("Earliest Expiry: {}", expiry));
            }

            if let Some(days) = chain.days_until_chain_expiry {
                let days_text = if days < 0 {
                    format!("Expired {} days ago", -days)
                } else if days == 0 {
                    "Expires today".to_string()
                } else if days == 1 {
                    "Expires tomorrow".to_string()
                } else {
                    format!("{} days until expiry", days)
                };

                let color = if days < 0 {
                    egui::Color32::RED
                } else if days <= 7 {
                    egui::Color32::from_rgb(255, 100, 100)
                } else if days <= 30 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::GREEN
                };

                ui.colored_label(color, days_text);
            }
        });

    ui.add_space(10.0);

    // Certificate chain visualization (tree view)
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label("Chain Hierarchy:");
            ui.add_space(5.0);

            let mut cert_index = 0;

            // Client certificate (leaf)
            if let Some(client_cert) = &chain.client_cert {
                render_cert_node(
                    ui,
                    client_cert,
                    "Client Certificate",
                    0,
                    cert_index,
                    selected,
                );
                cert_index += 1;
            }

            // Intermediate certificates
            for (i, intermediate) in chain.intermediates.iter().enumerate() {
                let label = if i == chain.intermediates.len() - 1 && chain.root_ca.is_none() {
                    "Root CA"
                } else {
                    "Intermediate CA"
                };
                render_cert_node(ui, intermediate, label, 1 + i, cert_index, selected);
                cert_index += 1;
            }

            // Root CA (if separate)
            if let Some(root_ca) = &chain.root_ca {
                render_cert_node(
                    ui,
                    root_ca,
                    "Root CA",
                    chain.intermediates.len() + 1,
                    cert_index,
                    selected,
                );
            }
        });

    ui.add_space(10.0);

    // Detailed view of selected certificate
    if let Some(idx) = selected {
        let cert_opt = if *idx == 0 {
            chain.client_cert.as_ref()
        } else if *idx <= chain.intermediates.len() {
            chain.intermediates.get(*idx - 1)
        } else {
            chain.root_ca.as_ref()
        };

        if let Some(cert) = cert_opt {
            render_certificate_details(ui, cert);
        }
    }
}

/// Render a single certificate node in the chain tree
fn render_cert_node(
    ui: &mut egui::Ui,
    cert: &CertificateInfo,
    label: &str,
    depth: usize,
    index: usize,
    selected: &mut Option<usize>,
) {
    let indent = "  ".repeat(depth);
    let connector = if depth > 0 { "└─ " } else { "" };

    let status = ExpirationStatus::from_cert_info(cert);
    let (status_icon, status_color) = match status {
        ExpirationStatus::Valid => ("OK", egui::Color32::GREEN),
        ExpirationStatus::ExpiringSoon => ("!", egui::Color32::YELLOW),
        ExpirationStatus::Expired => ("X", egui::Color32::RED),
        ExpirationStatus::NotYetValid => ("?", egui::Color32::GRAY),
    };

    ui.horizontal(|ui| {
        ui.label(&indent);
        ui.label(connector);
        ui.colored_label(status_color, status_icon);

        let is_selected = *selected == Some(index);
        if ui
            .selectable_label(is_selected, format!("{}: {}", label, cert.subject_cn))
            .clicked()
        {
            *selected = Some(index);
        }

        ui.label(
            egui::RichText::new(format!("({} days)", cert.days_until_expiry))
                .small()
                .color(status_color),
        );
    });
}

/// Render detailed certificate information
fn render_certificate_details(ui: &mut egui::Ui, cert: &CertificateInfo) {
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.heading("Certificate Details");
            ui.add_space(10.0);

            egui::Grid::new("cert_details")
                .striped(true)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Subject CN:");
                    ui.label(&cert.subject_cn);
                    ui.end_row();

                    ui.label("Subject DN:");
                    ui.label(&cert.subject_dn);
                    ui.end_row();

                    ui.label("Issuer CN:");
                    ui.label(&cert.issuer_cn);
                    ui.end_row();

                    ui.label("Issuer DN:");
                    ui.label(&cert.issuer_dn);
                    ui.end_row();

                    ui.label("Serial Number:");
                    ui.label(&cert.serial_number);
                    ui.end_row();

                    ui.label("Not Before:");
                    ui.label(&cert.not_before);
                    ui.end_row();

                    ui.label("Not After:");
                    let expiry_color = if cert.is_expired {
                        egui::Color32::RED
                    } else if cert.expiring_soon {
                        egui::Color32::YELLOW
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.colored_label(expiry_color, &cert.not_after);
                    ui.end_row();

                    ui.label("Days Until Expiry:");
                    let days_text = if cert.days_until_expiry < 0 {
                        format!("{} (EXPIRED)", cert.days_until_expiry)
                    } else {
                        cert.days_until_expiry.to_string()
                    };
                    ui.colored_label(expiry_color, days_text);
                    ui.end_row();

                    ui.label("Fingerprint:");
                    ui.label(&cert.fingerprint);
                    ui.end_row();

                    ui.label("Is CA:");
                    ui.label(if cert.is_ca { "Yes" } else { "No" });
                    ui.end_row();

                    if !cert.key_usage.is_empty() {
                        ui.label("Key Usage:");
                        ui.label(cert.key_usage.join(", "));
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);

            // Renewal suggestions
            if cert.is_expired || cert.expiring_soon {
                ui.separator();
                ui.add_space(5.0);

                let warning_text = if cert.is_expired {
                    "This certificate has EXPIRED and must be renewed immediately."
                } else {
                    "This certificate expires within 30 days. Consider renewing soon."
                };

                ui.colored_label(
                    if cert.is_expired {
                        egui::Color32::RED
                    } else {
                        egui::Color32::YELLOW
                    },
                    warning_text,
                );

                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    if ui.button("Generate CSR").clicked() {
                        // TODO: Implement CSR generation workflow
                    }

                    if ui.button("Export Info").clicked() {
                        // TODO: Export certificate info for renewal request
                    }
                });
            }
        });
}

/// Load certificate chain from file
fn load_certificate_from_file(
    path: &PathBuf,
    password: Option<&str>,
) -> anyhow::Result<CertificateChainInfo> {
    let bundle = omnitak_cert::auto_load_certificate_bundle(path, password)?;
    Ok(CertificateChainInfo::from_bundle(&bundle))
}

/// Render expiration warning badge for a server connection
pub fn render_expiration_badge(ui: &mut egui::Ui, days_until_expiry: i64) {
    let (text, color) = if days_until_expiry < 0 {
        ("EXPIRED", egui::Color32::RED)
    } else if days_until_expiry <= 7 {
        ("CRITICAL", egui::Color32::from_rgb(255, 100, 100))
    } else if days_until_expiry <= 30 {
        ("WARNING", egui::Color32::YELLOW)
    } else {
        return; // No badge needed for valid certs
    };

    ui.colored_label(color, format!("[{}]", text));
}
