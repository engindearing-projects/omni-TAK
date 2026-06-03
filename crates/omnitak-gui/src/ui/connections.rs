//! Connections view for managing server connections.

use crate::{format_bytes, OmniTakApp, ServerDialogState};
use eframe::egui;
use omnitak_core::types::{Protocol, ServerStatus};
use std::path::PathBuf;

/// Result of certificate scanning
#[derive(Debug)]
struct CertificateScanResult {
    ca_path: Option<String>,
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
}

/// Scans common locations for TAK certificates
fn scan_for_certificates() -> Option<CertificateScanResult> {
    let common_paths = vec![
        "./certs",
        "/Users/iesouskurios/omniTAK/certs",
        "~/Downloads",
        ".",
    ];

    let mut ca_path: Option<String> = None;
    let mut client_cert_path: Option<String> = None;
    let mut client_key_path: Option<String> = None;

    for base_path in common_paths {
        let expanded_path = if base_path.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                PathBuf::from(home).join(&base_path[2..])
            } else {
                continue;
            }
        } else {
            PathBuf::from(base_path)
        };

        if !expanded_path.exists() {
            continue;
        }

        // Look for CA certificate
        if ca_path.is_none() {
            for ca_name in &[
                "ca.pem",
                "ca.crt",
                "ca-cert.pem",
                "truststore.pem",
                "ca.p12",
            ] {
                let path = expanded_path.join(ca_name);
                if path.exists() {
                    ca_path = Some(path.to_string_lossy().to_string());
                    break;
                }
            }
        }

        // Look for client certificate
        if client_cert_path.is_none() {
            for cert_name in &[
                "admin.pem",
                "client.pem",
                "omnitak-desktop.pem",
                "client-cert.pem",
                "admin.p12",
                "client.p12",
            ] {
                let path = expanded_path.join(cert_name);
                if path.exists() {
                    client_cert_path = Some(path.to_string_lossy().to_string());
                    break;
                }
            }
        }

        // Look for client key
        if client_key_path.is_none() {
            for key_name in &[
                "admin-key.pem",
                "client-key.pem",
                "omnitak-desktop.key",
                "client.key",
                "admin.p12",
                "client.p12",
            ] {
                let path = expanded_path.join(key_name);
                if path.exists() {
                    client_key_path = Some(path.to_string_lossy().to_string());
                    break;
                }
            }
        }
    }

    // Only return result if we found at least one certificate
    if ca_path.is_some() || client_cert_path.is_some() || client_key_path.is_some() {
        Some(CertificateScanResult {
            ca_path,
            client_cert_path,
            client_key_path,
        })
    } else {
        None
    }
}

/// Shows the connections view.
pub fn show(ui: &mut egui::Ui, app: &mut OmniTakApp) {
    // Check if Quick Connect wizard is open
    if app.ui_state.quick_connect.is_some() {
        // Take ownership temporarily to avoid borrow conflicts
        let mut quick_connect_state = app.ui_state.quick_connect.take().unwrap();
        let mut close_wizard = false;

        if let Some((msg, level)) =
            super::quick_connect::render_quick_connect(ui, app, &mut quick_connect_state)
        {
            app.show_status(msg, level, 3);
        }

        ui.add_space(10.0);
        if ui.button("❌ Close Wizard").clicked() {
            close_wizard = true;
        }

        if !close_wizard {
            // Restore the state
            app.ui_state.quick_connect = Some(quick_connect_state);
        }
        return;
    }

    ui.horizontal(|ui| {
        ui.heading("Server Connections");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("🔄 Refresh").clicked() {
                app.refresh_from_api();
                app.show_status(
                    "Connections refreshed".to_string(),
                    crate::StatusLevel::Success,
                    2,
                );
            }

            // Quick Connect button - prominent placement
            if ui.button("⚡ Quick Connect").clicked() {
                app.ui_state.quick_connect = Some(super::quick_connect::QuickConnectState::new());
            }
        });
    });
    ui.add_space(10.0);

    // Handle certificate file picker promises
    if let Some(promise) = &app.ui_state.cert_ca_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                if let Some(dialog_state) = &mut app.ui_state.inline_server_form {
                    dialog_state.ca_cert_path = path.to_string_lossy().to_string();
                }
            }
            app.ui_state.cert_ca_promise = None;
        }
    }

    if let Some(promise) = &app.ui_state.cert_client_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                if let Some(dialog_state) = &mut app.ui_state.inline_server_form {
                    dialog_state.client_cert_path = path.to_string_lossy().to_string();
                }
            }
            app.ui_state.cert_client_promise = None;
        }
    }

    if let Some(promise) = &app.ui_state.cert_key_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                if let Some(dialog_state) = &mut app.ui_state.inline_server_form {
                    dialog_state.client_key_path = path.to_string_lossy().to_string();
                }
            }
            app.ui_state.cert_key_promise = None;
        }
    }

    // Inline Add/Edit Form
    let mut form_closed = false;
    let mut form_saved = false;

    if let Some(dialog_state) = &mut app.ui_state.inline_server_form {
        egui::CollapsingHeader::new(if dialog_state.editing_index.is_some() {
            "✏ Edit Server"
        } else {
            "➕ Add New Server"
        })
        .default_open(true)
        .show(ui, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(40))
                .corner_radius(5.0)
                .inner_margin(15.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        // Server Name
                        ui.horizontal(|ui| {
                            ui.label("Server Name:");
                            ui.text_edit_singleline(&mut dialog_state.config.name);
                        });
                        ui.add_space(5.0);

                        // Host and Port
                        ui.horizontal(|ui| {
                            ui.label("Host:");
                            ui.add(
                                egui::TextEdit::singleline(&mut dialog_state.config.host)
                                    .desired_width(200.0),
                            );
                            ui.add_space(10.0);
                            ui.label("Port:");
                            let mut port_str = dialog_state.config.port.to_string();
                            if ui
                                .add(egui::TextEdit::singleline(&mut port_str).desired_width(80.0))
                                .changed()
                            {
                                if let Ok(port) = port_str.parse::<u16>() {
                                    dialog_state.config.port = port;
                                }
                            }
                        });
                        ui.add_space(5.0);

                        // Protocol
                        ui.horizontal(|ui| {
                            ui.label("Protocol:");
                            egui::ComboBox::from_id_salt("protocol_combo")
                                .selected_text(format!("{}", dialog_state.config.protocol))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut dialog_state.config.protocol,
                                        Protocol::Tcp,
                                        "TCP",
                                    );
                                    ui.selectable_value(
                                        &mut dialog_state.config.protocol,
                                        Protocol::Udp,
                                        "UDP",
                                    );
                                    ui.selectable_value(
                                        &mut dialog_state.config.protocol,
                                        Protocol::Tls,
                                        "TLS",
                                    );
                                    ui.selectable_value(
                                        &mut dialog_state.config.protocol,
                                        Protocol::WebSocket,
                                        "WebSocket",
                                    );
                                });
                        });
                        ui.add_space(5.0);

                        // TLS Configuration (if TLS protocol selected)
                        if dialog_state.config.protocol == Protocol::Tls {
                            ui.separator();
                            ui.label(egui::RichText::new("TLS Configuration").strong());
                            ui.add_space(5.0);

                            ui.checkbox(&mut dialog_state.tls_enabled, "Enable TLS");

                            if dialog_state.tls_enabled {
                                // Auto-detect button
                                if ui.button("🔍 Auto-detect Certificates").clicked() {
                                    if let Some(certs) = scan_for_certificates() {
                                        dialog_state.ca_cert_path =
                                            certs.ca_path.unwrap_or_default();
                                        dialog_state.client_cert_path =
                                            certs.client_cert_path.unwrap_or_default();
                                        dialog_state.client_key_path =
                                            certs.client_key_path.unwrap_or_default();
                                    }
                                }
                                ui.add_space(5.0);

                                ui.horizontal(|ui| {
                                    ui.label("CA Certificate:");
                                    ui.text_edit_singleline(&mut dialog_state.ca_cert_path);
                                    if ui.button("📁").clicked()
                                        && app.ui_state.cert_ca_promise.is_none()
                                    {
                                        let promise = poll_promise::Promise::spawn_thread(
                                            "ca_cert_dialog",
                                            || {
                                                rfd::FileDialog::new()
                                                    .add_filter(
                                                        "Certificate Files",
                                                        &["pem", "crt", "cer", "p12", "pfx"],
                                                    )
                                                    .add_filter("All Files", &["*"])
                                                    .pick_file()
                                            },
                                        );
                                        app.ui_state.cert_ca_promise = Some(promise);
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Client Certificate:");
                                    ui.text_edit_singleline(&mut dialog_state.client_cert_path);
                                    if ui.button("📁").clicked()
                                        && app.ui_state.cert_client_promise.is_none()
                                    {
                                        let promise = poll_promise::Promise::spawn_thread(
                                            "client_cert_dialog",
                                            || {
                                                rfd::FileDialog::new()
                                                    .add_filter(
                                                        "Certificate Files",
                                                        &["pem", "crt", "cer", "p12", "pfx"],
                                                    )
                                                    .add_filter("All Files", &["*"])
                                                    .pick_file()
                                            },
                                        );
                                        app.ui_state.cert_client_promise = Some(promise);
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Client Key:");
                                    ui.text_edit_singleline(&mut dialog_state.client_key_path);
                                    if ui.button("📁").clicked()
                                        && app.ui_state.cert_key_promise.is_none()
                                    {
                                        let promise = poll_promise::Promise::spawn_thread(
                                            "client_key_dialog",
                                            || {
                                                rfd::FileDialog::new()
                                                    .add_filter(
                                                        "Key Files",
                                                        &["pem", "key", "p12", "pfx"],
                                                    )
                                                    .add_filter("All Files", &["*"])
                                                    .pick_file()
                                            },
                                        );
                                        app.ui_state.cert_key_promise = Some(promise);
                                    }
                                });

                                ui.checkbox(
                                    &mut dialog_state.verify_cert,
                                    "Verify Server Certificate",
                                );

                                ui.horizontal(|ui| {
                                    ui.label("Server Name (optional):");
                                    ui.text_edit_singleline(&mut dialog_state.server_name);
                                });
                            }
                        }

                        ui.add_space(10.0);

                        // Enabled checkbox
                        ui.checkbox(&mut dialog_state.config.enabled, "Enable auto-connect");

                        ui.add_space(15.0);

                        // Action buttons
                        ui.horizontal(|ui| {
                            if ui.button("💾 Save").clicked() {
                                form_saved = true;
                            }

                            if ui.button("✖ Cancel").clicked() {
                                form_closed = true;
                            }
                        });
                    });
                });
        });
        ui.add_space(10.0);
    } else {
        // Add server button (only show when form is closed)
        if ui.button("➕ Add Server").clicked() {
            app.ui_state.inline_server_form = Some(ServerDialogState::new());
        }
        ui.add_space(10.0);
    }

    // Handle form actions
    if form_saved {
        if let Some(dialog_state) = &app.ui_state.inline_server_form {
            let config = dialog_state.build();
            if let Some(idx) = dialog_state.editing_index {
                app.update_server(idx, config);
                app.show_status("Server updated".to_string(), crate::StatusLevel::Success, 3);
            } else {
                app.add_server(config);
                app.show_status("Server added".to_string(), crate::StatusLevel::Success, 3);
            }
        }
        app.ui_state.inline_server_form = None;
    }

    if form_closed {
        app.ui_state.inline_server_form = None;
    }

    // Server list
    let state = app.state.lock().unwrap();
    let servers_clone = state.servers.clone();
    let connections_clone = state.connections.clone();
    drop(state);

    if servers_clone.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(50.0);
            ui.label(
                egui::RichText::new("No servers configured")
                    .size(16.0)
                    .color(egui::Color32::GRAY),
            );
            ui.label("Click 'Add Server' to configure your first TAK server connection");
        });
        return;
    }

    let mut server_to_remove: Option<usize> = None;
    let mut server_to_edit: Option<usize> = None;
    let mut server_to_connect: Option<usize> = None;
    let mut server_to_disconnect: Option<String> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (idx, server) in servers_clone.iter().enumerate() {
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(35))
                .corner_radius(5.0)
                .inner_margin(15.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            // Server name and host
                            ui.label(egui::RichText::new(&server.name).size(16.0).strong());
                            ui.label(format!(
                                "{}:{} ({})",
                                server.host, server.port, server.protocol
                            ));

                            // Connection status
                            if let Some(metadata) = connections_clone.get(&server.name) {
                                ui.horizontal(|ui| {
                                    let (status_text, status_color) = match metadata.status {
                                        ServerStatus::Connected => {
                                            ("● Connected", egui::Color32::GREEN)
                                        }
                                        ServerStatus::Disconnected => {
                                            ("● Disconnected", egui::Color32::GRAY)
                                        }
                                        ServerStatus::Reconnecting => {
                                            ("● Reconnecting", egui::Color32::YELLOW)
                                        }
                                        ServerStatus::Failed => ("● Failed", egui::Color32::RED),
                                    };
                                    ui.colored_label(status_color, status_text);

                                    if let Some(error) = &metadata.last_error {
                                        ui.label(
                                            egui::RichText::new(format!("({})", error))
                                                .color(egui::Color32::LIGHT_RED),
                                        );
                                    }
                                });

                                // Metrics
                                ui.horizontal(|ui| {
                                    ui.label(format!(
                                        "↓ {} msgs ({}) | ↑ {} msgs ({})",
                                        metadata.messages_received,
                                        format_bytes(metadata.bytes_received),
                                        metadata.messages_sent,
                                        format_bytes(metadata.bytes_sent)
                                    ));
                                });

                                if metadata.reconnect_attempts > 0 {
                                    ui.label(format!(
                                        "Reconnect attempts: {}",
                                        metadata.reconnect_attempts
                                    ));
                                }
                            } else {
                                ui.colored_label(egui::Color32::GRAY, "● Not connected");
                            }

                            // TLS indicator
                            if server.tls.is_some() {
                                ui.label(
                                    egui::RichText::new("🔒 TLS Enabled")
                                        .color(egui::Color32::LIGHT_GREEN),
                                );
                            }

                            // Tags
                            if !server.tags.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.label("Tags:");
                                    for tag in &server.tags {
                                        ui.label(
                                            egui::RichText::new(tag)
                                                .background_color(egui::Color32::from_gray(60)),
                                        );
                                    }
                                });
                            }
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("🗑 Delete").clicked() {
                                server_to_remove = Some(idx);
                            }

                            if ui.button("✏ Edit").clicked() {
                                server_to_edit = Some(idx);
                            }

                            // Connect/Disconnect button
                            if let Some(metadata) = connections_clone.get(&server.name) {
                                if metadata.status == ServerStatus::Connected {
                                    if ui.button("⏸ Disconnect").clicked() {
                                        server_to_disconnect = Some(server.name.clone());
                                    }
                                } else if metadata.status != ServerStatus::Reconnecting {
                                    if ui.button("▶ Connect").clicked() {
                                        server_to_connect = Some(idx);
                                    }
                                }
                            } else if server.enabled {
                                if ui.button("▶ Connect").clicked() {
                                    server_to_connect = Some(idx);
                                }
                            }

                            // Enable/disable toggle
                            let enabled_text = if server.enabled {
                                "Enabled"
                            } else {
                                "Disabled"
                            };
                            let enabled_color = if server.enabled {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::GRAY
                            };
                            ui.colored_label(enabled_color, enabled_text);
                        });
                    });
                });

            ui.add_space(10.0);
        }
    });

    // Handle actions
    if let Some(idx) = server_to_remove {
        // Disconnect before removing
        let server_name = {
            let state = app.state.lock().unwrap();
            state.servers.get(idx).map(|s| s.name.clone())
        };
        if let Some(name) = server_name {
            app.disconnect_server(name);
        }
        app.remove_server(idx);
    }

    if let Some(idx) = server_to_edit {
        let server_opt = {
            let state = app.state.lock().unwrap();
            state.servers.get(idx).cloned()
        };
        if let Some(server) = server_opt {
            app.ui_state.inline_server_form = Some(ServerDialogState::edit(idx, server));
        }
    }

    if let Some(idx) = server_to_connect {
        let server_opt = {
            let state = app.state.lock().unwrap();
            state.servers.get(idx).cloned()
        };
        if let Some(server) = server_opt {
            app.connect_server(server);
        }
    }

    if let Some(server_name) = server_to_disconnect {
        app.disconnect_server(server_name);
    }
}
