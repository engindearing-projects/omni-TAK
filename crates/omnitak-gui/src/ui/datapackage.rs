//! TAK Data Package UI - Import/Export .dpk/.zip files

use egui::{Button, Color32, ScrollArea, Ui};
use omnitak_datapackage::{
    ContentType, DataPackageBuilder, DataPackageReader, PackageContent, PackageSummary,
};
use poll_promise::Promise;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{AppState, StatusLevel as CrateStatusLevel};

/// State for the Data Package panel
#[derive(Default)]
pub struct DataPackagePanelState {
    /// Currently loaded package
    pub loaded_package: Option<LoadedPackage>,
    /// File picker promise for import
    pub import_promise: Option<Promise<Option<PathBuf>>>,
    /// Package creation dialog
    pub create_dialog: Option<CreatePackageDialog>,
    /// Export in progress
    pub export_promise: Option<Promise<Result<PathBuf, String>>>,
    /// Status message
    pub status_message: Option<(String, StatusLevel)>,
}

/// Loaded package information
#[derive(Clone)]
pub struct LoadedPackage {
    pub path: PathBuf,
    pub uid: String,
    pub name: String,
    pub summary: PackageSummary,
    pub contents: Vec<PackageContent>,
}

/// Package creation dialog state
#[derive(Default)]
pub struct CreatePackageDialog {
    pub name: String,
    pub delete_on_receive: bool,
    pub files_to_add: Vec<PathBuf>,
    pub cot_events: Vec<(String, String)>, // (name, xml)
}

/// Status message level
#[derive(Clone, Copy, PartialEq)]
pub enum StatusLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl StatusLevel {
    fn color(&self) -> Color32 {
        match self {
            Self::Info => Color32::from_rgb(100, 150, 255),
            Self::Success => Color32::from_rgb(100, 255, 100),
            Self::Warning => Color32::from_rgb(255, 200, 100),
            Self::Error => Color32::from_rgb(255, 100, 100),
        }
    }
}

/// Render the Data Package panel
pub fn render_datapackage_panel(
    ui: &mut Ui,
    _state: &Arc<Mutex<AppState>>,
    panel_state: &mut DataPackagePanelState,
) -> Option<(String, CrateStatusLevel)> {
    let mut status_to_return = None;

    ui.heading("TAK Data Packages");
    ui.add_space(8.0);

    // Check for import completion
    if let Some(promise) = &panel_state.import_promise {
        if let Some(result) = promise.ready() {
            if let Some(path) = result {
                match DataPackageReader::open(path) {
                    Ok(reader) => {
                        let loaded = LoadedPackage {
                            path: path.clone(),
                            uid: reader.uid().unwrap_or("unknown").to_string(),
                            name: reader.name().unwrap_or("unknown").to_string(),
                            summary: reader.summary(),
                            contents: reader.contents().to_vec(),
                        };
                        panel_state.loaded_package = Some(loaded);
                        panel_state.status_message = Some((
                            format!("Loaded package: {}", path.display()),
                            StatusLevel::Success,
                        ));
                    }
                    Err(e) => {
                        panel_state.status_message =
                            Some((format!("Failed to load package: {}", e), StatusLevel::Error));
                    }
                }
            }
            panel_state.import_promise = None;
        }
    }

    // Check for export completion
    if let Some(promise) = &panel_state.export_promise {
        if let Some(result) = promise.ready() {
            match result {
                Ok(path) => {
                    panel_state.status_message = Some((
                        format!("Package created: {}", path.display()),
                        StatusLevel::Success,
                    ));
                }
                Err(e) => {
                    panel_state.status_message =
                        Some((format!("Export failed: {}", e), StatusLevel::Error));
                }
            }
            panel_state.export_promise = None;
        }
    }

    // Show status message
    if let Some((msg, level)) = &panel_state.status_message {
        ui.colored_label(level.color(), msg);
        ui.add_space(4.0);
    }

    // Action buttons
    ui.horizontal(|ui| {
        if ui.button("📦 Import Package").clicked() {
            panel_state.import_promise = Some(spawn_file_picker());
        }

        if ui.button("➕ Create New Package").clicked() {
            panel_state.create_dialog = Some(CreatePackageDialog::default());
        }

        if panel_state.loaded_package.is_some() {
            if ui.button("🗑 Close Package").clicked() {
                panel_state.loaded_package = None;
                panel_state.status_message = None;
            }
        }
    });

    ui.separator();

    // Show loaded package details
    let has_loaded_package = panel_state.loaded_package.is_some();
    let has_create_dialog = panel_state.create_dialog.is_some();

    if has_loaded_package {
        // Clone the loaded package to avoid borrow conflicts
        let pkg = panel_state.loaded_package.as_ref().unwrap().clone();
        render_package_details(ui, &pkg, panel_state);
    } else if has_create_dialog {
        render_create_dialog(ui, panel_state);
    } else {
        ui.label("No package loaded. Import a .dpk/.zip file or create a new package.");
    }

    // Convert status message for return
    if let Some((msg, level)) = panel_state.status_message.take() {
        let return_level = match level {
            StatusLevel::Info => CrateStatusLevel::Info,
            StatusLevel::Success => CrateStatusLevel::Success,
            StatusLevel::Warning => CrateStatusLevel::Warning,
            StatusLevel::Error => CrateStatusLevel::Error,
        };
        status_to_return = Some((msg, return_level));
    }

    status_to_return
}

fn render_package_details(
    ui: &mut Ui,
    pkg: &LoadedPackage,
    panel_state: &mut DataPackagePanelState,
) {
    ui.heading("Package Details");

    egui::Grid::new("package_info")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("UID:");
            ui.monospace(&pkg.uid);
            ui.end_row();

            ui.label("Name:");
            ui.label(&pkg.name);
            ui.end_row();

            ui.label("Path:");
            ui.monospace(pkg.path.display().to_string());
            ui.end_row();

            ui.label("Total Size:");
            ui.label(pkg.summary.human_readable_size());
            ui.end_row();

            ui.label("Total Files:");
            ui.label(pkg.summary.total_files.to_string());
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.heading("Content Summary");

    egui::Grid::new("content_summary")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            if pkg.summary.cot_events > 0 {
                ui.label("CoT Events:");
                ui.label(pkg.summary.cot_events.to_string());
                ui.end_row();
            }
            if pkg.summary.map_overlays > 0 {
                ui.label("Map Overlays:");
                ui.label(pkg.summary.map_overlays.to_string());
                ui.end_row();
            }
            if pkg.summary.map_tiles > 0 {
                ui.label("Map Tiles:");
                ui.label(pkg.summary.map_tiles.to_string());
                ui.end_row();
            }
            if pkg.summary.configs > 0 {
                ui.label("Config Files:");
                ui.label(pkg.summary.configs.to_string());
                ui.end_row();
            }
            if pkg.summary.certificates > 0 {
                ui.label("Certificates:");
                ui.label(pkg.summary.certificates.to_string());
                ui.end_row();
            }
            if pkg.summary.attachments > 0 {
                ui.label("Attachments:");
                ui.label(pkg.summary.attachments.to_string());
                ui.end_row();
            }
        });

    ui.add_space(8.0);
    ui.heading("Files");

    ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        for content in &pkg.contents {
            ui.horizontal(|ui| {
                let icon = match content.content_type {
                    ContentType::CotEvent => "📍",
                    ContentType::MapOverlay => "🗺",
                    ContentType::MapTiles => "🧩",
                    ContentType::Configuration | ContentType::Preferences => "⚙",
                    ContentType::Certificate => "🔐",
                    ContentType::Attachment | ContentType::Route => "📎",
                    ContentType::Unknown => "📄",
                };

                ui.label(icon);
                ui.monospace(&content.path);
                ui.label(format!("({})", format_size(content.size)));

                if content.ignore {
                    ui.colored_label(Color32::YELLOW, "[ignored]");
                }
            });
        }
    });

    ui.add_space(8.0);

    // Export actions
    ui.horizontal(|ui| {
        if ui.button("📥 Extract All Files").clicked() {
            // Spawn extract dialog
            panel_state.status_message = Some((
                "Extract functionality - select destination folder".to_string(),
                StatusLevel::Info,
            ));
        }

        if ui.button("📋 Copy UID").clicked() {
            ui.ctx().copy_text(pkg.uid.clone());
            panel_state.status_message =
                Some(("UID copied to clipboard".to_string(), StatusLevel::Info));
        }
    });
}

fn render_create_dialog(ui: &mut Ui, panel_state: &mut DataPackagePanelState) {
    // Extract values we need before the borrow
    let (name, delete_on_receive, files_to_add, cot_events) = {
        let dialog = panel_state.create_dialog.as_ref().unwrap();
        (
            dialog.name.clone(),
            dialog.delete_on_receive,
            dialog.files_to_add.clone(),
            dialog.cot_events.clone(),
        )
    };

    let mut should_create = false;
    let mut should_cancel = false;
    let mut new_name = name.clone();
    let mut new_delete_on_receive = delete_on_receive;

    ui.heading("Create New Data Package");
    ui.add_space(8.0);

    egui::Grid::new("create_package")
        .num_columns(2)
        .spacing([10.0, 8.0])
        .show(ui, |ui| {
            ui.label("Package Name:");
            ui.text_edit_singleline(&mut new_name);
            ui.end_row();

            ui.label("Delete on Receive:");
            ui.checkbox(&mut new_delete_on_receive, "");
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.heading("Files to Include");

    if files_to_add.is_empty() {
        ui.label("No files added yet");
    } else {
        for file in &files_to_add {
            ui.horizontal(|ui| {
                ui.monospace(file.display().to_string());
                ui.small_button("❌");
            });
        }
    }

    ui.horizontal(|ui| {
        if ui.button("➕ Add File").clicked() {
            // File picker (TODO)
        }
        if ui.button("📁 Add Folder").clicked() {
            // Folder picker (TODO)
        }
    });

    ui.add_space(8.0);
    ui.heading("CoT Events");

    if cot_events.is_empty() {
        ui.label("No CoT events added");
    } else {
        for (event_name, _xml) in &cot_events {
            ui.horizontal(|ui| {
                ui.monospace(format!("{}.cot", event_name));
            });
        }
    }

    if ui.button("➕ Add CoT Event").clicked() {
        // Add event dialog (TODO)
    }

    ui.add_space(16.0);
    ui.separator();

    ui.horizontal(|ui| {
        if ui
            .add_enabled(!new_name.is_empty(), Button::new("💾 Create Package"))
            .clicked()
        {
            should_create = true;
        }

        if ui.button("Cancel").clicked() {
            should_cancel = true;
        }
    });

    // Update dialog state
    if let Some(dialog) = panel_state.create_dialog.as_mut() {
        dialog.name = new_name.clone();
        dialog.delete_on_receive = new_delete_on_receive;
    }

    // Handle actions after borrowing is complete
    if should_create {
        let pkg_name = if new_name.ends_with(".zip") || new_name.ends_with(".dpk") {
            new_name.clone()
        } else {
            format!("{}.zip", new_name)
        };

        let mut builder =
            DataPackageBuilder::new(&pkg_name).on_receive_delete(new_delete_on_receive);

        // Add CoT events
        for (event_name, xml) in &cot_events {
            builder = match builder.add_cot_event(event_name, xml) {
                Ok(b) => b,
                Err(_) => {
                    // Skip failed event - builder is consumed, can't continue
                    break;
                }
            };
        }

        panel_state.status_message = Some((
            format!("Package '{}' ready to save", pkg_name),
            StatusLevel::Success,
        ));
        panel_state.create_dialog = None;
    } else if should_cancel {
        panel_state.create_dialog = None;
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn spawn_file_picker() -> Promise<Option<PathBuf>> {
    Promise::spawn_thread("file_picker", || {
        rfd::FileDialog::new()
            .add_filter("TAK Data Package", &["zip", "dpk"])
            .add_filter("All Files", &["*"])
            .set_title("Select TAK Data Package")
            .pick_file()
    })
}
