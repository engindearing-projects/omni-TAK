//! OmniTAK GUI Application
//!
//! Desktop GUI for managing OmniTAK TAK server connections.

use eframe::egui;
use std::path::PathBuf;

fn main() -> Result<(), eframe::Error> {
    // Set up logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let config_path = if args.len() > 2 && args[1] == "--config" {
        Some(PathBuf::from(&args[2]))
    } else {
        // Default to config.yaml in current directory
        let default_path = PathBuf::from("config.yaml");
        if default_path.exists() {
            Some(default_path)
        } else {
            None
        }
    };

    if let Some(ref path) = config_path {
        tracing::info!("Loading configuration from: {}", path.display());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_icon(
                // Load icon if available
                eframe::icon_data::from_png_bytes(&[]).unwrap_or_else(|_| egui::IconData {
                    rgba: vec![],
                    width: 0,
                    height: 0,
                }),
            ),
        ..Default::default()
    };

    let config_path_clone = config_path.clone();
    eframe::run_native(
        "OmniTAK - TAK Server Aggregator",
        options,
        Box::new(move |cc| {
            Ok(Box::new(omnitak_gui::OmniTakApp::new(
                cc,
                config_path_clone,
            )))
        }),
    )
}
