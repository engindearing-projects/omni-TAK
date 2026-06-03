//! Dashboard view showing system overview and metrics.

use crate::{format_bytes, format_duration, OmniTakApp};
use eframe::egui;
use omnitak_core::types::ServerStatus;

/// Shows the dashboard view.
pub fn show(ui: &mut egui::Ui, app: &mut OmniTakApp) {
    ui.horizontal(|ui| {
        ui.heading("System Dashboard");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("🔄 Refresh").clicked() {
                app.refresh_from_api();
                app.show_status(
                    "Dashboard refreshed".to_string(),
                    crate::StatusLevel::Success,
                    2,
                );
            }
        });
    });
    ui.add_space(10.0);

    let state = app.state.lock().unwrap();

    // System Status Section
    egui::CollapsingHeader::new(egui::RichText::new("🖥️ System Status").size(16.0).strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                // API Server Status
                let api_status = if app.embedded_server.is_some() {
                    format!("Running on port {}", app.api_port)
                } else {
                    "External".to_string()
                };
                status_card(
                    ui,
                    "API Server",
                    &api_status,
                    if app.embedded_server.is_some() {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::YELLOW
                    },
                    "🌐",
                );

                // API Authentication
                status_card(
                    ui,
                    "API Auth",
                    if app.is_authenticated {
                        "Authenticated"
                    } else {
                        "Not logged in"
                    },
                    if app.is_authenticated {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::RED
                    },
                    "🔑",
                );

                // Embedded Server PID
                if let Some(ref server) = app.embedded_server {
                    let pid_text = format!("{}", server.id());
                    status_card(ui, "Server PID", &pid_text, egui::Color32::LIGHT_BLUE, "⚙️");
                }

                // Connection Pool
                let conn_text = format!(
                    "{} / {}",
                    state.metrics.active_connections,
                    state.servers.len()
                );
                status_card(
                    ui,
                    "Active Connections",
                    &conn_text,
                    if state.metrics.active_connections > 0 {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::GRAY
                    },
                    "🔌",
                );
            });
            ui.add_space(5.0);
        });

    ui.add_space(10.0);

    // Message Flow Metrics Section
    egui::CollapsingHeader::new(egui::RichText::new("📊 Message Flow").size(16.0).strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                metric_card(
                    ui,
                    "Messages Received",
                    &state.metrics.total_messages_received.to_string(),
                    egui::Color32::LIGHT_BLUE,
                );
                metric_card(
                    ui,
                    "Messages Sent",
                    &state.metrics.total_messages_sent.to_string(),
                    egui::Color32::LIGHT_GREEN,
                );
                metric_card(
                    ui,
                    "Bytes Received",
                    &format_bytes(state.metrics.total_bytes_received),
                    egui::Color32::from_rgb(100, 150, 255),
                );
                metric_card(
                    ui,
                    "Bytes Sent",
                    &format_bytes(state.metrics.total_bytes_sent),
                    egui::Color32::from_rgb(100, 255, 150),
                );
            });
            ui.add_space(5.0);
        });

    ui.add_space(10.0);

    // TAK Server Health Section
    egui::CollapsingHeader::new(
        egui::RichText::new("🛰️ TAK Server Health")
            .size(16.0)
            .strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            metric_card(
                ui,
                "Total Servers",
                &state.servers.len().to_string(),
                egui::Color32::BLUE,
            );
            metric_card(
                ui,
                "Active Connections",
                &state.metrics.active_connections.to_string(),
                if state.metrics.active_connections > 0 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::GRAY
                },
            );
            metric_card(
                ui,
                "Failed Connections",
                &state.metrics.failed_connections.to_string(),
                if state.metrics.failed_connections > 0 {
                    egui::Color32::RED
                } else {
                    egui::Color32::GRAY
                },
            );
            // Calculate healthy connection percentage
            let total = state.servers.len();
            let health_pct = if total > 0 {
                (state.metrics.active_connections as f32 / total as f32 * 100.0) as u32
            } else {
                0
            };
            metric_card(
                ui,
                "Health",
                &format!("{}%", health_pct),
                if health_pct >= 80 {
                    egui::Color32::GREEN
                } else if health_pct >= 50 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                },
            );
        });
        ui.add_space(5.0);
    });

    ui.add_space(10.0);

    // Connection Details Section
    egui::CollapsingHeader::new(
        egui::RichText::new("🔗 Connection Details")
            .size(16.0)
            .strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(10.0);

        if state.servers.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(30.0);
                ui.label(
                    egui::RichText::new("No servers configured")
                        .size(16.0)
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new("Go to the Connections tab to add servers")
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(30.0);
            });
        } else {
            egui::Grid::new("connection_status_grid")
                .num_columns(4)
                .spacing([20.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Server").strong());
                    ui.label(egui::RichText::new("Status").strong());
                    ui.label(egui::RichText::new("Messages").strong());
                    ui.label(egui::RichText::new("Uptime").strong());
                    ui.end_row();

                    for server in &state.servers {
                        if let Some(metadata) = state.connections.get(&server.name) {
                            ui.label(&server.name);

                            // Status with color and icon
                            let (status_text, status_color, icon) = match metadata.status {
                                ServerStatus::Connected => ("Connected", egui::Color32::GREEN, "✓"),
                                ServerStatus::Disconnected => {
                                    ("Disconnected", egui::Color32::GRAY, "○")
                                }
                                ServerStatus::Reconnecting => {
                                    ("Reconnecting", egui::Color32::YELLOW, "⟳")
                                }
                                ServerStatus::Failed => ("Failed", egui::Color32::RED, "✗"),
                            };
                            ui.horizontal(|ui| {
                                ui.colored_label(status_color, icon);
                                ui.colored_label(status_color, status_text);
                            });

                            ui.label(format!(
                                "{} ↓ / {} ↑",
                                metadata.messages_received, metadata.messages_sent
                            ));

                            if let Some(uptime) = metadata.uptime() {
                                ui.label(format_duration(uptime));
                            } else {
                                ui.label("-");
                            }

                            ui.end_row();
                        } else {
                            ui.label(&server.name);
                            ui.horizontal(|ui| {
                                ui.colored_label(egui::Color32::GRAY, "○");
                                ui.colored_label(egui::Color32::GRAY, "No connection");
                            });
                            ui.label("-");
                            ui.label("-");
                            ui.end_row();
                        }
                    }
                });
        }
        ui.add_space(5.0);
    });
}

/// Shows a metric card.
fn metric_card(ui: &mut egui::Ui, title: &str, value: &str, color: egui::Color32) {
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(40))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(12.0)
                        .color(egui::Color32::GRAY),
                );
                ui.label(egui::RichText::new(value).size(24.0).strong().color(color));
            });
        });
}

/// Shows a status card with icon.
fn status_card(ui: &mut egui::Ui, title: &str, value: &str, color: egui::Color32, icon: &str) {
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(35))
        .corner_radius(5.0)
        .inner_margin(12.0)
        .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.3)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icon).size(20.0).color(color));
                ui.add_space(5.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(11.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.label(egui::RichText::new(value).size(14.0).strong().color(color));
                });
            });
        });
}
