//! Command Palette for quick actions (Cmd/Ctrl+K)

use crate::{OmniTakApp, Tab};
use egui::{Key, Modifiers};

/// A command that can be executed from the palette
#[derive(Debug, Clone)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub description: String,
    pub shortcut: Option<String>,
    pub category: CommandCategory,
}

/// Category for organizing commands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Connection,
    View,
    Tools,
    Settings,
}

impl CommandCategory {
    pub fn label(&self) -> &'static str {
        match self {
            CommandCategory::Navigation => "Navigation",
            CommandCategory::Connection => "Connection",
            CommandCategory::View => "View",
            CommandCategory::Tools => "Tools",
            CommandCategory::Settings => "Settings",
        }
    }
}

/// Command palette state
#[derive(Default)]
pub struct CommandPaletteState {
    pub open: bool,
    pub search_query: String,
    pub selected_index: usize,
    pub filtered_commands: Vec<Command>,
}

impl CommandPaletteState {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.search_query.clear();
            self.selected_index = 0;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.search_query.clear();
        self.selected_index = 0;
    }
}

/// Get all available commands
pub fn get_all_commands() -> Vec<Command> {
    vec![
        // Navigation
        Command {
            id: "nav.dashboard".to_string(),
            name: "Go to Dashboard".to_string(),
            description: "Open the dashboard view".to_string(),
            shortcut: Some("Ctrl+1".to_string()),
            category: CommandCategory::Navigation,
        },
        Command {
            id: "nav.connections".to_string(),
            name: "Go to Connections".to_string(),
            description: "Open the connections view".to_string(),
            shortcut: Some("Ctrl+2".to_string()),
            category: CommandCategory::Navigation,
        },
        Command {
            id: "nav.messages".to_string(),
            name: "Go to Messages".to_string(),
            description: "Open the messages view".to_string(),
            shortcut: Some("Ctrl+3".to_string()),
            category: CommandCategory::Navigation,
        },
        Command {
            id: "nav.map".to_string(),
            name: "Go to Map".to_string(),
            description: "Open the map view".to_string(),
            shortcut: Some("Ctrl+4".to_string()),
            category: CommandCategory::Navigation,
        },
        Command {
            id: "nav.plugins".to_string(),
            name: "Go to Plugins".to_string(),
            description: "Open the plugins view".to_string(),
            shortcut: Some("Ctrl+5".to_string()),
            category: CommandCategory::Navigation,
        },
        Command {
            id: "nav.settings".to_string(),
            name: "Go to Settings".to_string(),
            description: "Open the settings view".to_string(),
            shortcut: Some("Ctrl+,".to_string()),
            category: CommandCategory::Navigation,
        },
        // Connections
        Command {
            id: "conn.add".to_string(),
            name: "Add New Connection".to_string(),
            description: "Add a new server connection".to_string(),
            shortcut: Some("Ctrl+N".to_string()),
            category: CommandCategory::Connection,
        },
        Command {
            id: "conn.quick".to_string(),
            name: "Quick Connect".to_string(),
            description: "Open the quick connect wizard".to_string(),
            shortcut: Some("Ctrl+Shift+N".to_string()),
            category: CommandCategory::Connection,
        },
        Command {
            id: "conn.refresh".to_string(),
            name: "Refresh Connections".to_string(),
            description: "Refresh connection status from API".to_string(),
            shortcut: Some("Ctrl+R".to_string()),
            category: CommandCategory::Connection,
        },
        // View
        Command {
            id: "view.theme.toggle".to_string(),
            name: "Toggle Dark Mode".to_string(),
            description: "Switch between light and dark themes".to_string(),
            shortcut: Some("Ctrl+Shift+D".to_string()),
            category: CommandCategory::View,
        },
        Command {
            id: "view.fullscreen".to_string(),
            name: "Toggle Fullscreen".to_string(),
            description: "Toggle fullscreen mode".to_string(),
            shortcut: Some("F11".to_string()),
            category: CommandCategory::View,
        },
        Command {
            id: "view.zoom.in".to_string(),
            name: "Zoom In".to_string(),
            description: "Increase UI scale".to_string(),
            shortcut: Some("Ctrl++".to_string()),
            category: CommandCategory::View,
        },
        Command {
            id: "view.zoom.out".to_string(),
            name: "Zoom Out".to_string(),
            description: "Decrease UI scale".to_string(),
            shortcut: Some("Ctrl+-".to_string()),
            category: CommandCategory::View,
        },
        Command {
            id: "view.zoom.reset".to_string(),
            name: "Reset Zoom".to_string(),
            description: "Reset UI scale to default".to_string(),
            shortcut: Some("Ctrl+0".to_string()),
            category: CommandCategory::View,
        },
        // Tools
        Command {
            id: "tools.export".to_string(),
            name: "Export Configuration".to_string(),
            description: "Export server configurations to file".to_string(),
            shortcut: Some("Ctrl+E".to_string()),
            category: CommandCategory::Tools,
        },
        Command {
            id: "tools.import".to_string(),
            name: "Import Configuration".to_string(),
            description: "Import server configurations from file".to_string(),
            shortcut: Some("Ctrl+I".to_string()),
            category: CommandCategory::Tools,
        },
        Command {
            id: "tools.clear_messages".to_string(),
            name: "Clear Message Log".to_string(),
            description: "Clear all messages from the log".to_string(),
            shortcut: None,
            category: CommandCategory::Tools,
        },
        // Settings
        Command {
            id: "settings.auto_connect".to_string(),
            name: "Toggle Auto-Connect".to_string(),
            description: "Toggle auto-connect on startup".to_string(),
            shortcut: None,
            category: CommandCategory::Settings,
        },
    ]
}

/// Filter commands based on search query
pub fn filter_commands(commands: &[Command], query: &str) -> Vec<Command> {
    if query.is_empty() {
        return commands.to_vec();
    }

    let query_lower = query.to_lowercase();
    let mut scored: Vec<(Command, i32)> = commands
        .iter()
        .filter_map(|cmd| {
            let name_lower = cmd.name.to_lowercase();
            let desc_lower = cmd.description.to_lowercase();
            let id_lower = cmd.id.to_lowercase();

            let mut score = 0;

            // Exact match in name (highest priority)
            if name_lower.contains(&query_lower) {
                score += 100;
                // Bonus for starting with query
                if name_lower.starts_with(&query_lower) {
                    score += 50;
                }
            }

            // Match in ID
            if id_lower.contains(&query_lower) {
                score += 50;
            }

            // Match in description
            if desc_lower.contains(&query_lower) {
                score += 25;
            }

            // Word matching (fuzzy)
            for word in query_lower.split_whitespace() {
                if name_lower.contains(word) {
                    score += 10;
                }
            }

            if score > 0 {
                Some((cmd.clone(), score))
            } else {
                None
            }
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| b.1.cmp(&a.1));

    scored.into_iter().map(|(cmd, _)| cmd).collect()
}

/// Render the command palette overlay
pub fn render_command_palette(
    ctx: &egui::Context,
    palette_state: &mut CommandPaletteState,
) -> Option<String> {
    if !palette_state.open {
        return None;
    }

    let mut executed_command = None;

    // Semi-transparent background overlay
    let screen_rect = ctx.screen_rect();
    egui::Area::new(egui::Id::new("command_palette_bg"))
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let painter = ui.painter();
            painter.rect_filled(
                screen_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
        });

    // Command palette window
    let palette_width = 600.0;
    let palette_height = 400.0;
    let palette_pos = egui::pos2(
        screen_rect.center().x - palette_width / 2.0,
        screen_rect.min.y + 100.0,
    );

    egui::Window::new("Command Palette")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_pos(palette_pos)
        .fixed_size([palette_width, palette_height])
        .show(ctx, |ui| {
            // Search input
            ui.horizontal(|ui| {
                ui.label(">");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut palette_state.search_query)
                        .desired_width(palette_width - 40.0)
                        .hint_text("Type a command...")
                        .font(egui::TextStyle::Heading),
                );

                // Focus the text input
                response.request_focus();
            });

            ui.separator();

            // Handle keyboard navigation
            let mut close_palette = false;
            ctx.input(|i| {
                if i.key_pressed(Key::Escape) {
                    close_palette = true;
                }
                if i.key_pressed(Key::ArrowUp) {
                    if palette_state.selected_index > 0 {
                        palette_state.selected_index -= 1;
                    }
                }
                if i.key_pressed(Key::ArrowDown) {
                    if !palette_state.filtered_commands.is_empty()
                        && palette_state.selected_index < palette_state.filtered_commands.len() - 1
                    {
                        palette_state.selected_index += 1;
                    }
                }
                if i.key_pressed(Key::Enter) && !palette_state.filtered_commands.is_empty() {
                    if let Some(cmd) = palette_state
                        .filtered_commands
                        .get(palette_state.selected_index)
                    {
                        executed_command = Some(cmd.id.clone());
                        close_palette = true;
                    }
                }
            });

            if close_palette {
                palette_state.close();
                return;
            }

            // Filter commands
            let all_commands = get_all_commands();
            palette_state.filtered_commands =
                filter_commands(&all_commands, &palette_state.search_query);

            // Ensure selected index is valid
            if palette_state.selected_index >= palette_state.filtered_commands.len() {
                palette_state.selected_index = 0;
            }

            // Display filtered commands
            let mut should_close_from_click = false;
            let mut hover_index: Option<usize> = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut current_category: Option<CommandCategory> = None;

                    for (index, cmd) in palette_state.filtered_commands.iter().enumerate() {
                        // Show category header if changed
                        if current_category != Some(cmd.category) {
                            current_category = Some(cmd.category);
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(cmd.category.label())
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                            ui.add_space(2.0);
                        }

                        let is_selected = index == palette_state.selected_index;
                        let bg_color = if is_selected {
                            egui::Color32::from_rgb(60, 120, 180)
                        } else {
                            egui::Color32::TRANSPARENT
                        };

                        let response = ui.horizontal(|ui| {
                            ui.painter().rect_filled(
                                ui.available_rect_before_wrap(),
                                4.0,
                                bg_color,
                            );

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&cmd.name).strong().color(
                                        if is_selected {
                                            egui::Color32::WHITE
                                        } else {
                                            ui.visuals().text_color()
                                        },
                                    ));

                                    if let Some(shortcut) = &cmd.shortcut {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(shortcut)
                                                        .small()
                                                        .color(egui::Color32::GRAY),
                                                );
                                            },
                                        );
                                    }
                                });

                                ui.label(egui::RichText::new(&cmd.description).small().color(
                                    if is_selected {
                                        egui::Color32::LIGHT_GRAY
                                    } else {
                                        egui::Color32::GRAY
                                    },
                                ));
                            });
                        });

                        // Handle click
                        if response.response.interact(egui::Sense::click()).clicked() {
                            executed_command = Some(cmd.id.clone());
                            should_close_from_click = true;
                        }

                        // Hover effect
                        if response.response.hovered() && !is_selected {
                            hover_index = Some(index);
                        }

                        ui.add_space(2.0);
                    }

                    if palette_state.filtered_commands.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(20.0);
                            ui.label(
                                egui::RichText::new("No commands found").color(egui::Color32::GRAY),
                            );
                        });
                    }
                });

            // Apply deferred state changes
            if should_close_from_click {
                palette_state.close();
            }
            if let Some(idx) = hover_index {
                palette_state.selected_index = idx;
            }

            // Help text at bottom
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("↑↓ Navigate").small());
                    ui.separator();
                    ui.label(egui::RichText::new("↵ Execute").small());
                    ui.separator();
                    ui.label(egui::RichText::new("Esc Close").small());
                });
            });
        });

    executed_command
}

/// Execute a command by ID
pub fn execute_command(app: &mut OmniTakApp, command_id: &str, ctx: &egui::Context) {
    match command_id {
        // Navigation
        "nav.dashboard" => app.ui_state.selected_tab = Tab::Dashboard,
        "nav.connections" => app.ui_state.selected_tab = Tab::Connections,
        "nav.messages" => app.ui_state.selected_tab = Tab::Messages,
        "nav.map" => app.ui_state.selected_tab = Tab::Map,
        "nav.plugins" => app.ui_state.selected_tab = Tab::Plugins,
        "nav.settings" => app.ui_state.selected_tab = Tab::Settings,

        // View
        "view.theme.toggle" => {
            let mut state = app.state.lock().unwrap();
            state.settings.dark_mode = !state.settings.dark_mode;
            let is_dark = state.settings.dark_mode;
            drop(state);
            apply_theme(ctx, is_dark);
            app.show_status(
                if is_dark {
                    "Dark mode enabled".to_string()
                } else {
                    "Light mode enabled".to_string()
                },
                crate::StatusLevel::Info,
                2,
            );
        }
        "view.zoom.in" => {
            let mut state = app.state.lock().unwrap();
            state.settings.ui_scale = (state.settings.ui_scale + 0.1).min(2.0);
            let scale = state.settings.ui_scale;
            drop(state);
            ctx.set_pixels_per_point(scale);
        }
        "view.zoom.out" => {
            let mut state = app.state.lock().unwrap();
            state.settings.ui_scale = (state.settings.ui_scale - 0.1).max(0.5);
            let scale = state.settings.ui_scale;
            drop(state);
            ctx.set_pixels_per_point(scale);
        }
        "view.zoom.reset" => {
            let mut state = app.state.lock().unwrap();
            state.settings.ui_scale = 1.0;
            drop(state);
            ctx.set_pixels_per_point(1.0);
        }

        // Connections
        "conn.add" => {
            app.ui_state.selected_tab = Tab::Connections;
            app.ui_state.inline_server_form = Some(crate::ServerDialogState::new());
        }
        "conn.quick" => {
            app.ui_state.quick_connect =
                Some(crate::ui::quick_connect::QuickConnectState::default());
        }
        "conn.refresh" => {
            app.refresh_from_api();
            app.show_status(
                "Refreshed from API".to_string(),
                crate::StatusLevel::Success,
                2,
            );
        }

        // Tools
        "tools.export" => {
            app.ui_state.export_promise =
                Some(poll_promise::Promise::spawn_thread("export_picker", || {
                    rfd::FileDialog::new()
                        .add_filter("YAML", &["yaml", "yml"])
                        .set_file_name("omnitak-config.yaml")
                        .save_file()
                }));
        }
        "tools.import" => {
            app.ui_state.import_promise =
                Some(poll_promise::Promise::spawn_thread("import_picker", || {
                    rfd::FileDialog::new()
                        .add_filter("YAML", &["yaml", "yml"])
                        .pick_file()
                }));
        }
        "tools.clear_messages" => {
            let mut state = app.state.lock().unwrap();
            state.message_log.clear();
            drop(state);
            app.show_status(
                "Message log cleared".to_string(),
                crate::StatusLevel::Info,
                2,
            );
        }

        // Settings
        "settings.auto_connect" => {
            let mut state = app.state.lock().unwrap();
            state.settings.auto_start_connections = !state.settings.auto_start_connections;
            let enabled = state.settings.auto_start_connections;
            drop(state);
            app.show_status(
                if enabled {
                    "Auto-connect enabled".to_string()
                } else {
                    "Auto-connect disabled".to_string()
                },
                crate::StatusLevel::Info,
                2,
            );
        }

        _ => {
            tracing::warn!("Unknown command: {}", command_id);
        }
    }
}

/// Apply theme based on dark mode setting
pub fn apply_theme(ctx: &egui::Context, dark_mode: bool) {
    if dark_mode {
        ctx.set_visuals(egui::Visuals::dark());
    } else {
        ctx.set_visuals(egui::Visuals::light());
    }
}

/// Handle global keyboard shortcuts
pub fn handle_keyboard_shortcuts(ctx: &egui::Context, app: &mut OmniTakApp) -> bool {
    let mut handled = false;

    ctx.input(|i| {
        // Command Palette: Cmd/Ctrl + K
        if i.modifiers.command && i.key_pressed(Key::K) {
            app.command_palette.toggle();
            handled = true;
        }

        // Only process shortcuts if palette is not open
        if !app.command_palette.open {
            // Navigation shortcuts: Ctrl + 1-6
            if i.modifiers.ctrl && !i.modifiers.shift {
                if i.key_pressed(Key::Num1) {
                    app.ui_state.selected_tab = Tab::Dashboard;
                    handled = true;
                } else if i.key_pressed(Key::Num2) {
                    app.ui_state.selected_tab = Tab::Connections;
                    handled = true;
                } else if i.key_pressed(Key::Num3) {
                    app.ui_state.selected_tab = Tab::Messages;
                    handled = true;
                } else if i.key_pressed(Key::Num4) {
                    app.ui_state.selected_tab = Tab::Map;
                    handled = true;
                } else if i.key_pressed(Key::Num5) {
                    app.ui_state.selected_tab = Tab::Plugins;
                    handled = true;
                }
            }

            // Settings: Ctrl + ,
            if i.modifiers.command && i.key_pressed(Key::Comma) {
                app.ui_state.selected_tab = Tab::Settings;
                handled = true;
            }

            // New connection: Ctrl + N
            if i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::N) {
                app.ui_state.selected_tab = Tab::Connections;
                app.ui_state.inline_server_form = Some(crate::ServerDialogState::new());
                handled = true;
            }

            // Quick connect: Ctrl + Shift + N
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::N) {
                app.ui_state.quick_connect =
                    Some(crate::ui::quick_connect::QuickConnectState::default());
                handled = true;
            }

            // Refresh: Ctrl + R
            if i.modifiers.ctrl && i.key_pressed(Key::R) {
                app.refresh_from_api();
                handled = true;
            }

            // Toggle dark mode: Ctrl + Shift + D
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::D) {
                let mut state = app.state.lock().unwrap();
                state.settings.dark_mode = !state.settings.dark_mode;
                let is_dark = state.settings.dark_mode;
                drop(state);
                apply_theme(ctx, is_dark);
                handled = true;
            }

            // Zoom controls
            if i.modifiers.ctrl {
                if i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus) {
                    let mut state = app.state.lock().unwrap();
                    state.settings.ui_scale = (state.settings.ui_scale + 0.1).min(2.0);
                    let scale = state.settings.ui_scale;
                    drop(state);
                    ctx.set_pixels_per_point(scale);
                    handled = true;
                }
                if i.key_pressed(Key::Minus) {
                    let mut state = app.state.lock().unwrap();
                    state.settings.ui_scale = (state.settings.ui_scale - 0.1).max(0.5);
                    let scale = state.settings.ui_scale;
                    drop(state);
                    ctx.set_pixels_per_point(scale);
                    handled = true;
                }
                if i.key_pressed(Key::Num0) {
                    let mut state = app.state.lock().unwrap();
                    state.settings.ui_scale = 1.0;
                    drop(state);
                    ctx.set_pixels_per_point(1.0);
                    handled = true;
                }
            }

            // Export: Ctrl + E
            if i.modifiers.ctrl && i.key_pressed(Key::E) {
                app.ui_state.export_promise =
                    Some(poll_promise::Promise::spawn_thread("export_picker", || {
                        rfd::FileDialog::new()
                            .add_filter("YAML", &["yaml", "yml"])
                            .set_file_name("omnitak-config.yaml")
                            .save_file()
                    }));
                handled = true;
            }

            // Import: Ctrl + I
            if i.modifiers.ctrl && i.key_pressed(Key::I) {
                app.ui_state.import_promise =
                    Some(poll_promise::Promise::spawn_thread("import_picker", || {
                        rfd::FileDialog::new()
                            .add_filter("YAML", &["yaml", "yml"])
                            .pick_file()
                    }));
                handled = true;
            }
        }
    });

    handled
}
