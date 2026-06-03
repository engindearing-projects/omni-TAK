//! Settings view for application configuration.

use crate::OmniTakApp;
use eframe::egui;

/// Shows the settings view.
pub fn show(ui: &mut egui::Ui, app: &mut OmniTakApp) {
    let state = &app.state;
    ui.heading("Settings");
    ui.add_space(10.0);

    ui.label("Application configuration and preferences");
    ui.add_space(20.0);

    // About section
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(35))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("About OmniTAK");
            ui.add_space(5.0);
            ui.label("Version: 0.2.0");
            ui.label("License: MIT OR Apache-2.0");
            ui.add_space(5.0);
            ui.label("OmniTAK is a military-grade TAK server aggregator that connects to");
            ui.label("multiple TAK servers simultaneously and intelligently routes CoT messages.");
        });

    ui.add_space(20.0);

    // Configuration info
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(35))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("Configuration");
            ui.add_space(5.0);

            let state = state.lock().unwrap();

            ui.label(format!("Total Servers: {}", state.servers.len()));
            ui.label(format!(
                "Active Connections: {}",
                state.metrics.active_connections
            ));
            ui.label(format!(
                "Message Log Size: {} entries",
                state.message_log.len()
            ));
        });

    ui.add_space(20.0);

    // Future settings
    ui.label(
        egui::RichText::new("Additional settings will be added in future releases")
            .color(egui::Color32::GRAY),
    );
    ui.add_space(10.0);

    // Import/Export Configuration
    egui::Frame::NONE
        .fill(egui::Color32::from_gray(35))
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("Import/Export Configuration");
            ui.add_space(5.0);

            ui.label("Backup or restore your server configurations");
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                // Handle ongoing export promise
                if let Some(promise) = &app.ui_state.export_promise {
                    if let Some(result) = promise.ready() {
                        if let Some(path) = result {
                            let state = app.state.lock().unwrap();
                            let config = crate::ConfigFile::new(state.servers.clone());
                            drop(state); // Release lock before I/O

                            match crate::export_config(&config, path.as_path()) {
                                Ok(()) => {
                                    app.show_status(
                                        format!("Configuration exported to {}", path.display()),
                                        crate::StatusLevel::Success,
                                        5,
                                    );
                                }
                                Err(e) => {
                                    app.show_status(
                                        format!("Export failed: {}", e),
                                        crate::StatusLevel::Error,
                                        10,
                                    );
                                }
                            }
                        }
                        app.ui_state.export_promise = None;
                    }
                }

                if ui.button("📤 Export Configuration").clicked()
                    && app.ui_state.export_promise.is_none()
                {
                    let promise = poll_promise::Promise::spawn_thread("export_dialog", || {
                        rfd::FileDialog::new()
                            .add_filter("YAML", &["yaml", "yml"])
                            .add_filter("JSON", &["json"])
                            .set_file_name("omnitak_config.yaml")
                            .save_file()
                    });
                    app.ui_state.export_promise = Some(promise);
                }
            });

            ui.add_space(10.0);

            ui.horizontal(|ui| {
                // Handle ongoing import promise
                if let Some(promise) = &app.ui_state.import_promise {
                    if let Some(result) = promise.ready() {
                        if let Some(path) = result {
                            match crate::import_config(&path) {
                                Ok(config) => {
                                    let mut state = app.state.lock().unwrap();
                                    let count = config.servers.len();
                                    state.servers.extend(config.servers);
                                    drop(state); // Release lock

                                    app.show_status(
                                        format!(
                                            "Imported {} server(s) from {}",
                                            count,
                                            path.display()
                                        ),
                                        crate::StatusLevel::Success,
                                        5,
                                    );
                                }
                                Err(e) => {
                                    app.show_status(
                                        format!("Import failed: {}", e),
                                        crate::StatusLevel::Error,
                                        10,
                                    );
                                }
                            }
                        }
                        app.ui_state.import_promise = None;
                    }
                }

                if ui.button("📥 Import Configuration").clicked()
                    && app.ui_state.import_promise.is_none()
                {
                    let promise = poll_promise::Promise::spawn_thread("import_dialog", || {
                        rfd::FileDialog::new()
                            .add_filter("YAML", &["yaml", "yml"])
                            .add_filter("JSON", &["json"])
                            .pick_file()
                    });
                    app.ui_state.import_promise = Some(promise);
                }
            });

            ui.add_space(5.0);
            ui.label(
                egui::RichText::new("⚠ Importing will add to existing servers")
                    .color(egui::Color32::YELLOW),
            );
        });

    ui.add_space(20.0);

    // Appearance Settings
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("Appearance");
            ui.add_space(10.0);

            // Dark Mode Toggle
            ui.horizontal(|ui| {
                let mut dark_mode = {
                    let state = app.state.lock().unwrap();
                    state.settings.dark_mode
                };

                let label = if dark_mode {
                    "🌙 Dark Mode"
                } else {
                    "☀️ Light Mode"
                };
                if ui.checkbox(&mut dark_mode, label).changed() {
                    let mut state = app.state.lock().unwrap();
                    state.settings.dark_mode = dark_mode;
                    drop(state);
                    crate::ui::command_palette::apply_theme(ui.ctx(), dark_mode);
                    app.show_status(
                        if dark_mode {
                            "Dark mode enabled".to_string()
                        } else {
                            "Light mode enabled".to_string()
                        },
                        crate::StatusLevel::Info,
                        2,
                    );
                }

                ui.label(
                    egui::RichText::new("Ctrl+Shift+D")
                        .small()
                        .color(egui::Color32::GRAY),
                );
            });

            ui.add_space(10.0);

            // UI Scale
            ui.horizontal(|ui| {
                ui.label("UI Scale:");

                let mut ui_scale = {
                    let state = app.state.lock().unwrap();
                    state.settings.ui_scale
                };

                let slider = egui::Slider::new(&mut ui_scale, 0.5..=2.0)
                    .step_by(0.1)
                    .suffix("x")
                    .show_value(true);

                if ui.add(slider).changed() {
                    let mut state = app.state.lock().unwrap();
                    state.settings.ui_scale = ui_scale;
                    drop(state);
                    ui.ctx().set_pixels_per_point(ui_scale);
                }

                if ui.button("Reset").clicked() {
                    let mut state = app.state.lock().unwrap();
                    state.settings.ui_scale = 1.0;
                    drop(state);
                    ui.ctx().set_pixels_per_point(1.0);
                }
            });

            ui.add_space(5.0);
            ui.label(
                egui::RichText::new("Ctrl++ / Ctrl+- to adjust, Ctrl+0 to reset")
                    .small()
                    .color(egui::Color32::GRAY),
            );
        });

    ui.add_space(20.0);

    // Keyboard Shortcuts Reference
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("Keyboard Shortcuts");
            ui.add_space(10.0);

            egui::Grid::new("shortcuts_grid")
                .striped(true)
                .spacing([20.0, 5.0])
                .show(ui, |ui| {
                    // General
                    ui.label(egui::RichText::new("General").strong());
                    ui.label("");
                    ui.end_row();

                    ui.label("Ctrl+K");
                    ui.label("Open Command Palette");
                    ui.end_row();

                    ui.label("Ctrl+,");
                    ui.label("Open Settings");
                    ui.end_row();

                    ui.label("Ctrl+R");
                    ui.label("Refresh from API");
                    ui.end_row();

                    // Navigation
                    ui.label(egui::RichText::new("Navigation").strong());
                    ui.label("");
                    ui.end_row();

                    ui.label("Ctrl+1");
                    ui.label("Dashboard");
                    ui.end_row();

                    ui.label("Ctrl+2");
                    ui.label("Connections");
                    ui.end_row();

                    ui.label("Ctrl+3");
                    ui.label("Messages");
                    ui.end_row();

                    ui.label("Ctrl+4");
                    ui.label("Map");
                    ui.end_row();

                    ui.label("Ctrl+5");
                    ui.label("Plugins");
                    ui.end_row();

                    // Connections
                    ui.label(egui::RichText::new("Connections").strong());
                    ui.label("");
                    ui.end_row();

                    ui.label("Ctrl+N");
                    ui.label("Add New Connection");
                    ui.end_row();

                    ui.label("Ctrl+Shift+N");
                    ui.label("Quick Connect Wizard");
                    ui.end_row();

                    // View
                    ui.label(egui::RichText::new("View").strong());
                    ui.label("");
                    ui.end_row();

                    ui.label("Ctrl+Shift+D");
                    ui.label("Toggle Dark Mode");
                    ui.end_row();

                    ui.label("Ctrl++");
                    ui.label("Zoom In");
                    ui.end_row();

                    ui.label("Ctrl+-");
                    ui.label("Zoom Out");
                    ui.end_row();

                    ui.label("Ctrl+0");
                    ui.label("Reset Zoom");
                    ui.end_row();

                    // Tools
                    ui.label(egui::RichText::new("Tools").strong());
                    ui.label("");
                    ui.end_row();

                    ui.label("Ctrl+E");
                    ui.label("Export Configuration");
                    ui.end_row();

                    ui.label("Ctrl+I");
                    ui.label("Import Configuration");
                    ui.end_row();
                });
        });

    ui.add_space(20.0);

    // Application Settings
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(5.0)
        .inner_margin(15.0)
        .show(ui, |ui| {
            ui.heading("Application Settings");
            ui.add_space(10.0);

            let mut settings_changed = false;

            // Auto-start connections
            ui.horizontal(|ui| {
                let mut auto_start = {
                    let state = app.state.lock().unwrap();
                    state.settings.auto_start_connections
                };

                if ui
                    .checkbox(&mut auto_start, "Auto-start connections on launch")
                    .changed()
                {
                    let mut state = app.state.lock().unwrap();
                    state.settings.auto_start_connections = auto_start;
                    settings_changed = true;
                }

                ui.label(egui::RichText::new("ℹ").color(egui::Color32::LIGHT_BLUE))
                    .on_hover_text(
                        "Automatically connect to all enabled servers when the application starts",
                    );
            });

            ui.add_space(10.0);

            // Message retention policy
            ui.horizontal(|ui| {
                ui.label("Message log retention:");

                let max_messages = {
                    let state = app.state.lock().unwrap();
                    state.settings.max_message_log_size
                };

                let mut temp_value = max_messages.to_string();
                if ui
                    .add(egui::TextEdit::singleline(&mut temp_value).desired_width(100.0))
                    .changed()
                {
                    if let Ok(value) = temp_value.parse::<usize>() {
                        if value >= 100 && value <= 100000 {
                            let mut state = app.state.lock().unwrap();
                            state.settings.max_message_log_size = value;
                            settings_changed = true;
                        }
                    }
                }

                ui.label("messages");
                ui.label(egui::RichText::new("ℹ").color(egui::Color32::LIGHT_BLUE))
                    .on_hover_text("Maximum number of messages to keep in the log (100-100000)");
            });

            if settings_changed {
                app.show_status(
                    "Settings updated".to_string(),
                    crate::StatusLevel::Success,
                    3,
                );
            }
        });

    ui.add_space(20.0);

    // Certificate Management
    egui::CollapsingHeader::new("Certificate Management")
        .default_open(false)
        .show(ui, |ui| {
            if let Some((message, level)) = crate::ui::certificates::render_certificate_manager(
                ui,
                &mut app.ui_state.certificate_manager,
            ) {
                app.show_status(message, level, 5);
            }
        });

    ui.add_space(20.0);
}
