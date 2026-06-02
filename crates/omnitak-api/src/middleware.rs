//! Middleware for logging, CORS, rate limiting, and security headers

use crate::auth::AuthService;
use crate::types::{AuditLogEntry, ErrorResponse, UserRole};
use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use governor::{
    Quota, RateLimiter,
    clock::DefaultClock,
    state::{InMemoryState, direct::NotKeyed},
};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

// ============================================================================
// Audit Logging
// ============================================================================

pub struct AuditLogger {
    logs: Arc<DashMap<Uuid, AuditLogEntry>>,
}

impl AuditLogger {
    pub fn new() -> Self {
        Self {
            logs: Arc::new(DashMap::new()),
        }
    }

    pub fn log(
        &self,
        user: String,
        role: UserRole,
        action: String,
        resource: String,
        details: serde_json::Value,
        source_ip: String,
        success: bool,
    ) {
        let entry = AuditLogEntry {
            id: Uuid::new_v4(),
            user,
            role,
            action,
            resource,
            details,
            source_ip,
            timestamp: chrono::Utc::now(),
            success,
        };

        info!(
            audit = true,
            user = entry.user,
            action = entry.action,
            resource = entry.resource,
            success = entry.success,
            "Audit log"
        );

        self.logs.insert(entry.id, entry);
    }

    pub fn get_logs(&self) -> Vec<AuditLogEntry> {
        self.logs.iter().map(|e| e.value().clone()).collect()
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Request Logging Middleware
// ============================================================================

pub async fn logging_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let version = request.version();

    info!(
        method = %method,
        uri = %uri,
        version = ?version,
        remote_addr = %addr,
        "Incoming request"
    );

    let start = std::time::Instant::now();
    let response = next.run(request).await;
    let elapsed = start.elapsed();

    info!(
        method = %method,
        uri = %uri,
        status = response.status().as_u16(),
        duration_ms = elapsed.as_millis(),
        "Request completed"
    );

    response
}

// ============================================================================
// Rate Limiting
// ============================================================================

pub struct RateLimitState {
    limiter: Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

impl RateLimitState {
    pub fn new(requests_per_second: u32) -> Self {
        let quota = Quota::per_second(NonZeroU32::new(requests_per_second).unwrap());
        Self {
            limiter: Arc::new(RateLimiter::direct(quota)),
        }
    }

    pub fn new_per_minute(requests_per_minute: u32) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(requests_per_minute).unwrap());
        Self {
            limiter: Arc::new(RateLimiter::direct(quota)),
        }
    }
}

pub async fn rate_limit_middleware(
    State(rate_limiter): State<Arc<RateLimitState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    match rate_limiter.limiter.check() {
        Ok(_) => next.run(request).await,
        Err(_) => {
            warn!(
                remote_addr = %addr,
                "Rate limit exceeded"
            );

            let error = ErrorResponse::new(
                "rate_limit_exceeded",
                "Too many requests. Please try again later.",
            );

            (StatusCode::TOO_MANY_REQUESTS, Json(error)).into_response()
        }
    }
}

// ============================================================================
// Security Headers Middleware
// ============================================================================

pub async fn security_headers_middleware(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // Never cache API responses — the dashboard polls live status/connection state.
    // (Static assets keep their own cache headers.)
    if is_api {
        headers.insert(
            "Cache-Control",
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        );
    }

    // Prevent MIME type sniffing
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );

    // Enable XSS protection
    headers.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );

    // Prevent clickjacking
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));

    // Strict transport security (HSTS)
    headers.insert(
        "Strict-Transport-Security",
        HeaderValue::from_static("max-age=31536000; includeSubDomains; preload"),
    );

    // Content Security Policy
    headers.insert(
        "Content-Security-Policy",
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:;",
        ),
    );

    // Referrer policy
    headers.insert(
        "Referrer-Policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    // Permissions policy
    headers.insert(
        "Permissions-Policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );

    response
}

// ============================================================================
// CORS Configuration
// ============================================================================

pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any) // TODO: Configure allowed origins from config
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::HeaderName::from_static("x-api-key"),
        ])
        .max_age(Duration::from_secs(3600))
}

// ============================================================================
// Request ID Middleware
// ============================================================================

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4();
    request.extensions_mut().insert(request_id);

    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "X-Request-ID",
        HeaderValue::from_str(&request_id.to_string()).unwrap(),
    );

    response
}

// ============================================================================
// Error Handler
// ============================================================================

pub async fn handle_error(err: Box<dyn std::error::Error + Send + Sync>) -> Response {
    error!(error = %err, "Unhandled error");

    let error = ErrorResponse::new("internal_error", "An internal error occurred");

    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
}

// ============================================================================
// Timeout Middleware
// ============================================================================

pub async fn timeout_middleware(request: Request, next: Next) -> Response {
    let timeout_duration = Duration::from_secs(30);

    match tokio::time::timeout(timeout_duration, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            warn!("Request timeout");
            let error = ErrorResponse::new("request_timeout", "Request took too long to process");
            (StatusCode::REQUEST_TIMEOUT, Json(error)).into_response()
        }
    }
}

// ============================================================================
// Health Check Handler
// ============================================================================

pub async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now(),
    }))
}

// ============================================================================
// Readiness Check Handler
// ============================================================================

#[derive(Clone)]
pub struct ReadinessState {
    pub ready: Arc<std::sync::atomic::AtomicBool>,
}

impl ReadinessState {
    pub fn new() -> Self {
        Self {
            ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready
            .store(ready, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for ReadinessState {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn readiness_check(State(state): State<Arc<ReadinessState>>) -> impl IntoResponse {
    if state.is_ready() {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ready",
                "timestamp": chrono::Utc::now(),
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "timestamp": chrono::Utc::now(),
            })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_logger() {
        let logger = AuditLogger::new();

        logger.log(
            "testuser".to_string(),
            UserRole::Admin,
            "create_connection".to_string(),
            "/api/v1/connections".to_string(),
            serde_json::json!({"connection_id": "123"}),
            "127.0.0.1".to_string(),
            true,
        );

        let logs = logger.get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].user, "testuser");
        assert_eq!(logs[0].action, "create_connection");
    }

    #[test]
    fn test_readiness_state() {
        let state = ReadinessState::new();
        assert!(!state.is_ready());

        state.set_ready(true);
        assert!(state.is_ready());

        state.set_ready(false);
        assert!(!state.is_ready());
    }
}
