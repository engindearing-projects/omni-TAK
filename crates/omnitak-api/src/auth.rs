//! Authentication and authorization with JWT and API keys

use crate::types::{ErrorResponse, UserRole};
use anyhow::{anyhow, Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json, RequestPartsExt,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// JWT secret key
    pub jwt_secret: String,

    /// JWT token expiration duration
    pub jwt_expiration: Duration,

    /// Enable API key authentication
    pub enable_api_keys: bool,

    /// Require authentication for all endpoints
    pub require_auth: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "change-me-in-production".to_string(),
            jwt_expiration: Duration::hours(24),
            enable_api_keys: true,
            require_auth: true,
        }
    }
}

// ============================================================================
// JWT Claims
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,

    /// User role
    pub role: UserRole,

    /// Issued at (Unix timestamp)
    pub iat: i64,

    /// Expiration time (Unix timestamp)
    pub exp: i64,

    /// JWT ID
    pub jti: String,
}

impl Claims {
    pub fn new(user_id: String, role: UserRole, expiration: Duration) -> Self {
        let now = Utc::now();
        Self {
            sub: user_id,
            role,
            iat: now.timestamp(),
            exp: (now + expiration).timestamp(),
            jti: Uuid::new_v4().to_string(),
        }
    }
}

// ============================================================================
// User Store
// ============================================================================

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub role: UserRole,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ApiKey {
    pub id: Uuid,
    pub key_hash: String,
    pub name: String,
    pub role: UserRole,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used: Option<DateTime<Utc>>,
}

// ============================================================================
// Authentication Service
// ============================================================================

pub struct AuthService {
    config: AuthConfig,
    pub users: Arc<DashMap<String, User>>,
    api_keys: Arc<DashMap<String, ApiKey>>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Self {
        let encoding_key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(config.jwt_secret.as_bytes());

        Self {
            config,
            users: Arc::new(DashMap::new()),
            api_keys: Arc::new(DashMap::new()),
            encoding_key,
            decoding_key,
        }
    }

    /// Hash a password using Argon2
    pub fn hash_password(&self, password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("Failed to hash password: {}", e))?
            .to_string();
        Ok(password_hash)
    }

    /// Verify a password against a hash
    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| anyhow::anyhow!("Failed to parse password hash: {}", e))?;
        let argon2 = Argon2::default();
        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// Create a new user
    pub fn create_user(&self, username: String, password: &str, role: UserRole) -> Result<Uuid> {
        if self.users.contains_key(&username) {
            return Err(anyhow!("User already exists"));
        }

        let password_hash = self.hash_password(password)?;
        let user = User {
            id: Uuid::new_v4(),
            username: username.clone(),
            password_hash,
            role,
            enabled: true,
            created_at: Utc::now(),
        };

        let user_id = user.id;
        self.users.insert(username, user);
        Ok(user_id)
    }

    /// Authenticate user and generate JWT token
    pub fn login(&self, username: &str, password: &str) -> Result<(String, DateTime<Utc>)> {
        let user = self
            .users
            .get(username)
            .ok_or_else(|| anyhow!("Invalid credentials"))?;

        if !user.enabled {
            return Err(anyhow!("User account is disabled"));
        }

        if !self.verify_password(password, &user.password_hash)? {
            return Err(anyhow!("Invalid credentials"));
        }

        let claims = Claims::new(user.id.to_string(), user.role, self.config.jwt_expiration);

        let expires_at = DateTime::from_timestamp(claims.exp, 0)
            .ok_or_else(|| anyhow!("Invalid expiration timestamp"))?;

        let token = encode(&Header::default(), &claims, &self.encoding_key)
            .context("Failed to encode JWT token")?;

        Ok((token, expires_at))
    }

    /// Verify and decode JWT token
    pub fn verify_token(&self, token: &str) -> Result<Claims> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &Validation::default())
            .context("Invalid JWT token")?;

        Ok(token_data.claims)
    }

    /// Generate a new API key
    pub fn create_api_key(
        &self,
        name: String,
        role: UserRole,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(String, Uuid)> {
        // Generate a random API key (64 hex characters)
        let api_key = format!("omni_{}", Uuid::new_v4().simple());
        let key_hash = self.hash_password(&api_key)?;

        let key_record = ApiKey {
            id: Uuid::new_v4(),
            key_hash: key_hash.clone(),
            name,
            role,
            enabled: true,
            created_at: Utc::now(),
            expires_at,
            last_used: None,
        };

        let key_id = key_record.id;
        self.api_keys.insert(key_hash, key_record);

        Ok((api_key, key_id))
    }

    /// Verify API key and return user role
    pub fn verify_api_key(&self, api_key: &str) -> Result<UserRole> {
        // Try to find matching API key by verifying hash
        for entry in self.api_keys.iter() {
            let key_record = entry.value();

            if !key_record.enabled {
                continue;
            }

            // Check expiration
            if let Some(expires_at) = key_record.expires_at {
                if Utc::now() > expires_at {
                    continue;
                }
            }

            // Verify the key
            if self.verify_password(api_key, &key_record.key_hash)? {
                // Update last used timestamp
                let key_hash = key_record.key_hash.clone();
                let role = key_record.role;
                drop(entry);
                if let Some(mut key) = self.api_keys.get_mut(&key_hash) {
                    key.last_used = Some(Utc::now());
                }
                return Ok(role);
            }
        }

        Err(anyhow!("Invalid API key"))
    }

    /// Revoke an API key
    pub fn revoke_api_key(&self, key_id: Uuid) -> Result<()> {
        for mut entry in self.api_keys.iter_mut() {
            if entry.value().id == key_id {
                entry.enabled = false;
                return Ok(());
            }
        }
        Err(anyhow!("API key not found"))
    }

    /// Check if user has required role
    pub fn check_role(&self, user_role: UserRole, required_role: UserRole) -> bool {
        match required_role {
            UserRole::Admin => user_role == UserRole::Admin,
            UserRole::Operator => user_role == UserRole::Admin || user_role == UserRole::Operator,
            UserRole::ReadOnly => true, // All roles include read-only
        }
    }
}

// ============================================================================
// Request Extractors
// ============================================================================

/// Authenticated user from JWT token or API key
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Option<String>,
    pub role: UserRole,
}

impl AuthUser {
    pub fn has_role(&self, required_role: UserRole) -> bool {
        match required_role {
            UserRole::Admin => self.role == UserRole::Admin,
            UserRole::Operator => self.role == UserRole::Admin || self.role == UserRole::Operator,
            UserRole::ReadOnly => true,
        }
    }
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Try JWT token first
        if let Ok(TypedHeader(Authorization(bearer))) =
            parts.extract::<TypedHeader<Authorization<Bearer>>>().await
        {
            // Extract auth service from state extensions
            let auth_service = parts
                .extensions
                .get::<Arc<AuthService>>()
                .ok_or(AuthError::InternalError)?;

            let claims = auth_service
                .verify_token(bearer.token())
                .map_err(|_| AuthError::InvalidToken)?;

            return Ok(AuthUser {
                user_id: Some(claims.sub),
                role: claims.role,
            });
        }

        // Try API key header
        if let Some(api_key) = parts.headers.get("X-API-Key") {
            let api_key = api_key.to_str().map_err(|_| AuthError::InvalidApiKey)?;

            let auth_service = parts
                .extensions
                .get::<Arc<AuthService>>()
                .ok_or(AuthError::InternalError)?;

            let role = auth_service
                .verify_api_key(api_key)
                .map_err(|_| AuthError::InvalidApiKey)?;

            return Ok(AuthUser {
                user_id: None,
                role,
            });
        }

        Err(AuthError::MissingCredentials)
    }
}

// ============================================================================
// Role-based access control extractors
// ============================================================================

/// Require admin role
pub struct RequireAdmin(pub AuthUser);

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;

        if !user.has_role(UserRole::Admin) {
            return Err(AuthError::InsufficientPermissions);
        }

        Ok(RequireAdmin(user))
    }
}

/// Require operator role or higher
pub struct RequireOperator(pub AuthUser);

impl<S> FromRequestParts<S> for RequireOperator
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;

        if !user.has_role(UserRole::Operator) {
            return Err(AuthError::InsufficientPermissions);
        }

        Ok(RequireOperator(user))
    }
}

// ============================================================================
// Error Handling
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Missing authentication credentials")]
    MissingCredentials,

    #[error("Invalid JWT token")]
    InvalidToken,

    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Insufficient permissions")]
    InsufficientPermissions,

    #[error("Internal authentication error")]
    InternalError,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            AuthError::MissingCredentials => (
                StatusCode::UNAUTHORIZED,
                "missing_credentials",
                "Authentication required",
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "Invalid or expired JWT token",
            ),
            AuthError::InvalidApiKey => (
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
                "Invalid or expired API key",
            ),
            AuthError::InsufficientPermissions => (
                StatusCode::FORBIDDEN,
                "insufficient_permissions",
                "Insufficient permissions for this operation",
            ),
            AuthError::InternalError => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Internal authentication error",
            ),
        };

        let body = Json(ErrorResponse::new(error_code, message));
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let auth = AuthService::new(AuthConfig::default());
        let password = "test_password_123";

        let hash = auth.hash_password(password).unwrap();
        assert!(auth.verify_password(password, &hash).unwrap());
        assert!(!auth.verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_create_user() {
        let auth = AuthService::new(AuthConfig::default());

        let user_id = auth
            .create_user("testuser".to_string(), "password123", UserRole::ReadOnly)
            .unwrap();

        assert!(user_id != Uuid::nil());
        assert!(auth.users.contains_key("testuser"));
    }

    #[test]
    fn test_jwt_flow() {
        let auth = AuthService::new(AuthConfig::default());

        // Create user
        auth.create_user("testuser".to_string(), "password123", UserRole::Admin)
            .unwrap();

        // Login
        let (token, _expires) = auth.login("testuser", "password123").unwrap();

        // Verify token
        let claims = auth.verify_token(&token).unwrap();
        assert_eq!(claims.role, UserRole::Admin);
    }

    #[test]
    fn test_role_checking() {
        let auth = AuthService::new(AuthConfig::default());

        assert!(auth.check_role(UserRole::Admin, UserRole::Admin));
        assert!(auth.check_role(UserRole::Admin, UserRole::Operator));
        assert!(auth.check_role(UserRole::Admin, UserRole::ReadOnly));

        assert!(!auth.check_role(UserRole::Operator, UserRole::Admin));
        assert!(auth.check_role(UserRole::Operator, UserRole::Operator));
        assert!(auth.check_role(UserRole::Operator, UserRole::ReadOnly));

        assert!(!auth.check_role(UserRole::ReadOnly, UserRole::Admin));
        assert!(!auth.check_role(UserRole::ReadOnly, UserRole::Operator));
        assert!(auth.check_role(UserRole::ReadOnly, UserRole::ReadOnly));
    }
}
