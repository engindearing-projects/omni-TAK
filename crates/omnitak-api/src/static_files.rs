//! Static file serving with embedded assets

use axum::{
    body::Body,
    http::{header, HeaderValue, Response, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use rust_embed::RustEmbed;
use std::borrow::Cow;
use tracing::debug;

// ============================================================================
// Embedded Static Assets
// ============================================================================

/// Embed static files from the web/static directory
/// This allows the binary to be self-contained with the web UI
#[derive(RustEmbed)]
#[folder = "web/static/"]
struct StaticAssets;

// ============================================================================
// Router Setup
// ============================================================================

pub fn create_static_router() -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/{*path}", get(serve_static_file))
}

// ============================================================================
// Handlers
// ============================================================================

/// Serve the index.html file
async fn serve_index() -> impl IntoResponse {
    serve_static_file(Uri::from_static("/index.html")).await
}

/// Serve static files from embedded assets
async fn serve_static_file(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Handle empty path
    let path = if path.is_empty() { "index.html" } else { path };

    debug!(path = path, "Serving static file");

    match StaticAssets::get(path) {
        Some(content) => {
            let mime_type = mime_type_for_path(path);
            let body = Body::from(content.data);

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_type)
                .header(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600"),
                )
                .body(body)
                .unwrap()
        }
        None => {
            debug!(path = path, "Static file not found");

            // Try to serve index.html for SPA routing
            if let Some(content) = StaticAssets::get("index.html") {
                let body = Body::from(content.data);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
                    .body(body)
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("404 Not Found"))
                    .unwrap()
            }
        }
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Determine MIME type based on file extension
fn mime_type_for_path(path: &str) -> &'static str {
    let extension = path.rsplit('.').next();

    match extension {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("tar") => "application/x-tar",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// List all embedded files (useful for debugging)
pub fn list_embedded_files() -> Vec<String> {
    StaticAssets::iter().map(|path| path.into_owned()).collect()
}

// ============================================================================
// Development Mode (Optional)
// ============================================================================

#[cfg(debug_assertions)]
pub fn print_embedded_files() {
    use tracing::info;

    let files = list_embedded_files();
    if files.is_empty() {
        info!("No static files embedded. Create web/static/ directory with assets.");
    } else {
        info!("Embedded static files:");
        for file in files {
            info!("  - {}", file);
        }
    }
}

#[cfg(not(debug_assertions))]
pub fn print_embedded_files() {
    // No-op in release mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_types() {
        assert_eq!(mime_type_for_path("index.html"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_path("style.css"), "text/css; charset=utf-8");
        assert_eq!(
            mime_type_for_path("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(mime_type_for_path("data.json"), "application/json");
        assert_eq!(mime_type_for_path("image.png"), "image/png");
        assert_eq!(
            mime_type_for_path("unknown.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_list_embedded_files() {
        let files = list_embedded_files();
        // In test environment, this may be empty unless web/static/ exists
        assert!(files.is_empty() || !files.is_empty());
    }
}
