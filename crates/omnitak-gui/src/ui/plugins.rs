//! Plugin management UI panel

use crate::api_client::{LoadPluginRequest, PluginApiType, PluginMetricsResponse};
use crate::{ApiClient, AppState, StatusLevel};
use eframe::egui;
use omnitak_plugin_api::{PluginCapability, PluginInfo};
use poll_promise::Promise;
use std::sync::{Arc, Mutex};

pub struct PluginPanelState {
    /// Search/filter text
    pub filter_text: String,

    /// Show only enabled plugins
    pub show_enabled_only: bool,

    /// Plugin type filter
    pub type_filter: PluginTypeFilter,

    /// Plugin being viewed in detail
    pub selected_plugin: Option<String>,

    /// Load plugin dialog state
    pub load_dialog: Option<LoadPluginDialog>,

    /// Plugin configuration editor
    pub config_editor: Option<ConfigEditorDialog>,

    /// Cached plugin list from API
    pub cached_plugins: Vec<PluginInfo>,

    /// Promise for loading plugin list
    pub list_promise: Option<Promise<Result<Vec<PluginInfo>, String>>>,

    /// Promise for loading plugin operation (load, unload, reload)
    pub operation_promise: Option<Promise<Result<String, String>>>,

    /// Promise for loading plugin metrics
    pub metrics_promise: Option<Promise<Result<PluginMetricsResponse, String>>>,

    /// Cached metrics for selected plugin
    pub cached_metrics: Option<PluginMetricsResponse>,

    /// Last refresh time
    pub last_refresh: std::time::Instant,

    /// File picker promise for plugin path
    pub file_picker_promise: Option<Promise<Option<std::path::PathBuf>>>,
}

impl Default for PluginPanelState {
    fn default() -> Self {
        Self {
            filter_text: String::new(),
            show_enabled_only: false,
            type_filter: PluginTypeFilter::All,
            selected_plugin: None,
            load_dialog: None,
            config_editor: None,
            cached_plugins: vec![],
            list_promise: None,
            operation_promise: None,
            metrics_promise: None,
            cached_metrics: None,
            last_refresh: std::time::Instant::now(),
            file_picker_promise: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTypeFilter {
    All,
    Filter,
    Transformer,
}

pub struct LoadPluginDialog {
    pub plugin_id: String,
    pub plugin_path: String,
    pub plugin_type: String,
    pub enabled: bool,
    pub error_message: Option<String>,
}

impl Default for LoadPluginDialog {
    fn default() -> Self {
        Self {
            plugin_id: String::new(),
            plugin_path: String::new(),
            plugin_type: "filter".to_string(),
            enabled: true,
            error_message: None,
        }
    }
}

pub struct ConfigEditorDialog {
    pub plugin_id: String,
    pub config_json: String,
    pub error_message: Option<String>,
}

/// Render the plugins panel
pub fn render_plugins_panel(
    ui: &mut egui::Ui,
    _state: &Arc<Mutex<AppState>>,
    panel_state: &mut PluginPanelState,
    api_client: Option<&ApiClient>,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    // Handle async promise results
    if let Some(promise) = &panel_state.list_promise {
        if let Some(result) = promise.ready() {
            match result {
                Ok(plugins) => {
                    panel_state.cached_plugins = plugins.clone();
                    panel_state.last_refresh = std::time::Instant::now();
                }
                Err(e) => {
                    status_message =
                        Some((format!("Failed to load plugins: {}", e), StatusLevel::Error));
                }
            }
            panel_state.list_promise = None;
        }
    }

    // Handle operation promise results
    if let Some(promise) = &panel_state.operation_promise {
        if let Some(result) = promise.ready() {
            match result {
                Ok(msg) => {
                    status_message = Some((msg.clone(), StatusLevel::Success));
                    // Trigger refresh after operation
                    if let Some(client) = api_client {
                        panel_state.list_promise = Some(spawn_list_plugins(client.clone()));
                    }
                }
                Err(e) => {
                    status_message = Some((e.clone(), StatusLevel::Error));
                }
            }
            panel_state.operation_promise = None;
        }
    }

    // Handle metrics promise results
    if let Some(promise) = &panel_state.metrics_promise {
        if let Some(result) = promise.ready() {
            match result {
                Ok(metrics) => {
                    panel_state.cached_metrics = Some(metrics.clone());
                }
                Err(e) => {
                    status_message =
                        Some((format!("Failed to load metrics: {}", e), StatusLevel::Error));
                }
            }
            panel_state.metrics_promise = None;
        }
    }

    // Auto-refresh on first load or every 30 seconds
    let should_refresh =
        panel_state.cached_plugins.is_empty() || panel_state.last_refresh.elapsed().as_secs() > 30;

    if should_refresh && panel_state.list_promise.is_none() {
        if let Some(client) = api_client {
            panel_state.list_promise = Some(spawn_list_plugins(client.clone()));
        }
    }

    ui.heading("Plugin Management");
    ui.add_space(10.0);

    // Connection status indicator
    if api_client.is_none() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Not connected to server. Connect to manage plugins.",
        );
        ui.add_space(10.0);
    }

    // Toolbar
    ui.horizontal(|ui| {
        let can_interact = api_client.is_some() && panel_state.operation_promise.is_none();

        ui.add_enabled_ui(can_interact, |ui| {
            if ui.button("Load Plugin").clicked() {
                panel_state.load_dialog = Some(LoadPluginDialog::default());
            }

            ui.add_space(10.0);

            if ui.button("Reload All").clicked() {
                if let Some(client) = api_client {
                    panel_state.operation_promise = Some(spawn_reload_all(client.clone()));
                    status_message =
                        Some(("Reloading all plugins...".to_string(), StatusLevel::Info));
                }
            }

            ui.add_space(10.0);

            if ui.button("Refresh").clicked() {
                if let Some(client) = api_client {
                    panel_state.list_promise = Some(spawn_list_plugins(client.clone()));
                }
            }
        });

        ui.add_space(10.0);

        ui.label("Filter:");
        ui.text_edit_singleline(&mut panel_state.filter_text);

        ui.add_space(10.0);

        ui.checkbox(&mut panel_state.show_enabled_only, "Enabled only");

        ui.add_space(10.0);

        egui::ComboBox::from_label("Type")
            .selected_text(format!("{:?}", panel_state.type_filter))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut panel_state.type_filter, PluginTypeFilter::All, "All");
                ui.selectable_value(
                    &mut panel_state.type_filter,
                    PluginTypeFilter::Filter,
                    "Filter",
                );
                ui.selectable_value(
                    &mut panel_state.type_filter,
                    PluginTypeFilter::Transformer,
                    "Transformer",
                );
            });
    });

    ui.add_space(10.0);

    // Show loading indicator
    if panel_state.list_promise.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading plugins...");
        });
    }

    ui.separator();
    ui.add_space(10.0);

    // Plugin list
    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let plugins = filter_plugins(&panel_state.cached_plugins, panel_state);

            if plugins.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    if panel_state.cached_plugins.is_empty() {
                        ui.label("No plugins loaded");
                        ui.add_space(10.0);
                        ui.label("Click 'Load Plugin' to add one");
                    } else {
                        ui.label("No plugins match filters");
                    }
                });
            } else {
                for plugin in plugins {
                    if let Some(msg) = render_plugin_card(ui, &plugin, panel_state, api_client) {
                        status_message = Some(msg);
                    }
                    ui.add_space(5.0);
                }
            }
        });

    // Load plugin dialog
    let mut close_load_dialog = false;
    if let Some(ref mut dialog) = panel_state.load_dialog {
        let mut open = true;
        egui::Window::new("Load Plugin")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                if let Some(msg) = render_load_plugin_dialog(ui, dialog) {
                    status_message = Some(msg);
                    close_load_dialog = true;
                }
            });

        if !open {
            close_load_dialog = true;
        }
    }
    if close_load_dialog {
        panel_state.load_dialog = None;
    }

    // Config editor dialog
    let mut close_config_editor = false;
    if let Some(ref mut dialog) = panel_state.config_editor {
        let mut open = true;
        egui::Window::new(format!("Configure Plugin: {}", dialog.plugin_id))
            .open(&mut open)
            .resizable(true)
            .default_width(600.0)
            .default_height(400.0)
            .show(ui.ctx(), |ui| {
                if let Some(msg) = render_config_editor_dialog(ui, dialog) {
                    status_message = Some(msg);
                    close_config_editor = true;
                }
            });

        if !open {
            close_config_editor = true;
        }
    }
    if close_config_editor {
        panel_state.config_editor = None;
    }

    status_message
}

fn render_plugin_card(
    ui: &mut egui::Ui,
    plugin: &PluginInfo,
    panel_state: &mut PluginPanelState,
    api_client: Option<&ApiClient>,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;
    let can_interact = api_client.is_some() && panel_state.operation_promise.is_none();

    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Plugin icon based on type
                let icon = if plugin.capabilities.contains(&PluginCapability::Filter) {
                    "🔍"
                } else if plugin.capabilities.contains(&PluginCapability::Transform) {
                    "⚙️"
                } else {
                    "📦"
                };

                ui.label(egui::RichText::new(icon).size(24.0));

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading(&plugin.name);
                        ui.label(egui::RichText::new(&plugin.version).weak());

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Enabled toggle
                            ui.add_enabled_ui(can_interact, |ui| {
                                let mut enabled = true; // TODO: Track actual state from API
                                if ui.checkbox(&mut enabled, "Enabled").changed() {
                                    if let Some(client) = api_client {
                                        let plugin_id = plugin.id.clone();
                                        panel_state.operation_promise = Some(spawn_toggle_plugin(
                                            client.clone(),
                                            plugin_id.clone(),
                                            enabled,
                                        ));
                                        status_message = Some((
                                            format!(
                                                "Plugin {} {}",
                                                plugin_id,
                                                if enabled { "enabled" } else { "disabled" }
                                            ),
                                            StatusLevel::Info,
                                        ));
                                    }
                                }
                            });
                        });
                    });

                    ui.label(&plugin.description);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("by {}", plugin.author)).weak());
                        ui.add_space(10.0);

                        // Capability badges
                        for cap in &plugin.capabilities {
                            let (text, color) = match cap {
                                PluginCapability::Filter => {
                                    ("Filter", egui::Color32::from_rgb(100, 149, 237))
                                }
                                PluginCapability::Transform => {
                                    ("Transform", egui::Color32::from_rgb(144, 238, 144))
                                }
                                PluginCapability::NetworkAccess => {
                                    ("Network", egui::Color32::from_rgb(255, 165, 0))
                                }
                                PluginCapability::FilesystemAccess => {
                                    ("Filesystem", egui::Color32::from_rgb(255, 99, 71))
                                }
                            };

                            ui.label(
                                egui::RichText::new(text)
                                    .small()
                                    .background_color(color)
                                    .color(egui::Color32::WHITE),
                            );
                        }
                    });

                    ui.add_space(5.0);

                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(can_interact, |ui| {
                            if ui.button("Configure").clicked() {
                                panel_state.config_editor = Some(ConfigEditorDialog {
                                    plugin_id: plugin.id.clone(),
                                    config_json: "{}".to_string(),
                                    error_message: None,
                                });
                            }

                            if ui.button("Metrics").clicked() {
                                panel_state.selected_plugin = Some(plugin.id.clone());
                                if let Some(client) = api_client {
                                    panel_state.metrics_promise =
                                        Some(spawn_get_metrics(client.clone(), plugin.id.clone()));
                                }
                            }

                            if ui.button("Reload").clicked() {
                                if let Some(client) = api_client {
                                    let plugin_id = plugin.id.clone();
                                    panel_state.operation_promise = Some(spawn_reload_plugin(
                                        client.clone(),
                                        plugin_id.clone(),
                                    ));
                                    status_message = Some((
                                        format!("Reloading plugin: {}", plugin_id),
                                        StatusLevel::Info,
                                    ));
                                }
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Unload").clicked() {
                                        if let Some(client) = api_client {
                                            let plugin_id = plugin.id.clone();
                                            panel_state.operation_promise =
                                                Some(spawn_unload_plugin(
                                                    client.clone(),
                                                    plugin_id.clone(),
                                                ));
                                            status_message = Some((
                                                format!("Unloading plugin: {}", plugin_id),
                                                StatusLevel::Warning,
                                            ));
                                        }
                                    }
                                },
                            );
                        });
                    });
                });
            });

            // Expandable metrics section
            if panel_state.selected_plugin.as_ref() == Some(&plugin.id) {
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                render_plugin_metrics(ui, plugin, panel_state);

                if ui.button("Close Metrics").clicked() {
                    panel_state.selected_plugin = None;
                    panel_state.cached_metrics = None;
                }
            }
        });

    status_message
}

fn render_plugin_metrics(ui: &mut egui::Ui, plugin: &PluginInfo, panel_state: &PluginPanelState) {
    ui.heading("Plugin Metrics");
    ui.add_space(5.0);

    if panel_state.metrics_promise.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading metrics...");
        });
        return;
    }

    let metrics = &panel_state.cached_metrics;

    egui::Grid::new(format!("metrics_{}", plugin.id))
        .num_columns(2)
        .spacing([40.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label("Executions:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| m.execution_count.to_string())
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("Errors:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| m.error_count.to_string())
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("Timeouts:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| m.timeout_count.to_string())
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("Avg Execution Time:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| format!("{:.2} ms", m.avg_execution_time_ms))
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("P95 Execution Time:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| format!("{:.2} ms", m.p95_execution_time_ms))
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("P99 Execution Time:");
            ui.label(
                metrics
                    .as_ref()
                    .map(|m| format!("{:.2} ms", m.p99_execution_time_ms))
                    .unwrap_or_else(|| "N/A".to_string()),
            );
            ui.end_row();

            ui.label("Last Execution:");
            ui.label(
                metrics
                    .as_ref()
                    .and_then(|m| m.last_execution.clone())
                    .unwrap_or_else(|| "Never".to_string()),
            );
            ui.end_row();

            if let Some(ref m) = metrics {
                if let Some(ref err) = m.last_error {
                    ui.label("Last Error:");
                    ui.colored_label(egui::Color32::RED, err);
                    ui.end_row();
                }
            }
        });

    ui.add_space(10.0);

    // Health status based on metrics
    ui.horizontal(|ui| {
        ui.label("Health:");
        if let Some(ref m) = metrics {
            let (status_text, status_color) = if m.error_count > 0 && m.execution_count > 0 {
                let error_rate = m.error_count as f64 / m.execution_count as f64;
                if error_rate > 0.1 {
                    ("Unhealthy", egui::Color32::RED)
                } else if error_rate > 0.01 {
                    ("Degraded", egui::Color32::YELLOW)
                } else {
                    ("Healthy", egui::Color32::GREEN)
                }
            } else {
                ("Healthy", egui::Color32::GREEN)
            };
            ui.label(egui::RichText::new(status_text).color(status_color));
        } else {
            ui.label(egui::RichText::new("Unknown").color(egui::Color32::GRAY));
        }
    });
}

fn render_load_plugin_dialog(
    ui: &mut egui::Ui,
    dialog: &mut LoadPluginDialog,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    ui.add_space(10.0);

    egui::Grid::new("load_plugin_grid")
        .num_columns(2)
        .spacing([10.0, 10.0])
        .show(ui, |ui| {
            ui.label("Plugin ID:");
            ui.text_edit_singleline(&mut dialog.plugin_id);
            ui.end_row();

            ui.label("Plugin Path:");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut dialog.plugin_path);
                if ui.button("Browse...").clicked() {
                    // TODO: File picker
                }
            });
            ui.end_row();

            ui.label("Plugin Type:");
            egui::ComboBox::from_id_salt("plugin_type")
                .selected_text(&dialog.plugin_type)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dialog.plugin_type, "filter".to_string(), "Filter");
                    ui.selectable_value(
                        &mut dialog.plugin_type,
                        "transformer".to_string(),
                        "Transformer",
                    );
                });
            ui.end_row();

            ui.label("Enabled:");
            ui.checkbox(&mut dialog.enabled, "");
            ui.end_row();
        });

    ui.add_space(10.0);

    if let Some(ref error) = dialog.error_message {
        ui.label(egui::RichText::new(error).color(egui::Color32::RED));
        ui.add_space(5.0);
    }

    ui.horizontal(|ui| {
        if ui.button("Load").clicked() {
            if dialog.plugin_id.is_empty() {
                dialog.error_message = Some("Plugin ID is required".to_string());
            } else if dialog.plugin_path.is_empty() {
                dialog.error_message = Some("Plugin path is required".to_string());
            } else {
                // TODO: Call API to load plugin
                status_message = Some((
                    format!("Loading plugin: {}", dialog.plugin_id),
                    StatusLevel::Success,
                ));
            }
        }

        if ui.button("Cancel").clicked() {
            status_message = Some(("Cancelled".to_string(), StatusLevel::Info));
        }
    });

    status_message
}

fn render_config_editor_dialog(
    ui: &mut egui::Ui,
    dialog: &mut ConfigEditorDialog,
) -> Option<(String, StatusLevel)> {
    let mut status_message = None;

    ui.add_space(10.0);

    ui.label("Plugin Configuration (JSON):");
    ui.add_space(5.0);

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut dialog.config_json)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(15),
            );
        });

    ui.add_space(10.0);

    if let Some(ref error) = dialog.error_message {
        ui.label(egui::RichText::new(error).color(egui::Color32::RED));
        ui.add_space(5.0);
    }

    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            // Validate JSON
            match serde_json::from_str::<serde_json::Value>(&dialog.config_json) {
                Ok(_) => {
                    // TODO: Call API to update config
                    status_message = Some((
                        format!("Configuration updated for plugin: {}", dialog.plugin_id),
                        StatusLevel::Success,
                    ));
                }
                Err(e) => {
                    dialog.error_message = Some(format!("Invalid JSON: {}", e));
                }
            }
        }

        if ui.button("Cancel").clicked() {
            status_message = Some(("Cancelled".to_string(), StatusLevel::Info));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Load Example").clicked() {
                dialog.config_json = serde_json::to_string_pretty(&serde_json::json!({
                    "center_lat": 35.0,
                    "center_lon": -79.0,
                    "radius_degrees": 2.0,
                    "enabled": true
                }))
                .unwrap();
            }
        });
    });

    status_message
}

// ============================================================================
// Async Helper Functions
// ============================================================================

/// Filter plugins based on panel state filters
fn filter_plugins(plugins: &[PluginInfo], panel_state: &PluginPanelState) -> Vec<PluginInfo> {
    plugins
        .iter()
        .filter(|p| {
            // Text filter
            if !panel_state.filter_text.is_empty() {
                let search = panel_state.filter_text.to_lowercase();
                let matches = p.name.to_lowercase().contains(&search)
                    || p.id.to_lowercase().contains(&search)
                    || p.description.to_lowercase().contains(&search)
                    || p.author.to_lowercase().contains(&search);
                if !matches {
                    return false;
                }
            }

            // Type filter
            match panel_state.type_filter {
                PluginTypeFilter::All => true,
                PluginTypeFilter::Filter => p.capabilities.contains(&PluginCapability::Filter),
                PluginTypeFilter::Transformer => {
                    p.capabilities.contains(&PluginCapability::Transform)
                }
            }
        })
        .cloned()
        .collect()
}

/// Spawn async task to list plugins
fn spawn_list_plugins(client: ApiClient) -> Promise<Result<Vec<PluginInfo>, String>> {
    Promise::spawn_thread("list_plugins", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async { client.list_plugins().await.map_err(|e| e.to_string()) })
    })
}

/// Spawn async task to load a plugin
fn spawn_load_plugin(
    client: ApiClient,
    request: LoadPluginRequest,
) -> Promise<Result<String, String>> {
    let plugin_id = request.id.clone();
    Promise::spawn_thread("load_plugin", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .load_plugin(request)
                .await
                .map(|_| format!("Plugin {} loaded successfully", plugin_id))
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to unload a plugin
fn spawn_unload_plugin(client: ApiClient, plugin_id: String) -> Promise<Result<String, String>> {
    let id = plugin_id.clone();
    Promise::spawn_thread("unload_plugin", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .unload_plugin(&id)
                .await
                .map(|_| format!("Plugin {} unloaded successfully", id))
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to reload a plugin
fn spawn_reload_plugin(client: ApiClient, plugin_id: String) -> Promise<Result<String, String>> {
    let id = plugin_id.clone();
    Promise::spawn_thread("reload_plugin", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .reload_plugin(&id)
                .await
                .map(|_| format!("Plugin {} reloaded successfully", id))
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to reload all plugins
fn spawn_reload_all(client: ApiClient) -> Promise<Result<String, String>> {
    Promise::spawn_thread("reload_all_plugins", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .reload_all_plugins()
                .await
                .map(|_| "All plugins reloaded successfully".to_string())
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to toggle plugin enabled state
fn spawn_toggle_plugin(
    client: ApiClient,
    plugin_id: String,
    enabled: bool,
) -> Promise<Result<String, String>> {
    let id = plugin_id.clone();
    Promise::spawn_thread("toggle_plugin", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .toggle_plugin(&id, enabled)
                .await
                .map(|_| {
                    format!(
                        "Plugin {} {}",
                        id,
                        if enabled { "enabled" } else { "disabled" }
                    )
                })
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to get plugin metrics
fn spawn_get_metrics(
    client: ApiClient,
    plugin_id: String,
) -> Promise<Result<PluginMetricsResponse, String>> {
    Promise::spawn_thread("get_plugin_metrics", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .get_plugin_metrics(&plugin_id)
                .await
                .map_err(|e| e.to_string())
        })
    })
}

/// Spawn async task to update plugin config
fn spawn_update_config(
    client: ApiClient,
    plugin_id: String,
    config: serde_json::Value,
) -> Promise<Result<String, String>> {
    let id = plugin_id.clone();
    Promise::spawn_thread("update_plugin_config", move || {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        rt.block_on(async {
            client
                .update_plugin_config(&id, config)
                .await
                .map(|_| format!("Configuration updated for plugin {}", id))
                .map_err(|e| e.to_string())
        })
    })
}
