use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use hypgm_authn::{
    api_key::{hash_api_key, validate_api_key_prefix, verify_api_key},
    error::AuthError,
    jwt::Claims,
    password::verify_password,
    principal::{AuthMethod, Principal},
    session::PrincipalAuthBackend,
};
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

use crate::api::{LoginRequest, LoginResponse, TokenRequest, TokenResponse};
use crate::repo::AuthRepo;

#[derive(Clone)]
pub struct AuthState {
    pub repo: Arc<dyn AuthRepo>,
    pub jwt_secret: Arc<Vec<u8>>,
    pub default_token_ttl: usize,
    pub max_token_ttl: usize,
    pub api_key_prefix: String,
}

impl AuthState {
    pub fn new(repo: Arc<dyn AuthRepo>, jwt_secret: Vec<u8>) -> Self {
        Self {
            repo,
            jwt_secret: Arc::new(jwt_secret),
            default_token_ttl: 3600,
            max_token_ttl: 86400,
            api_key_prefix: "exmpl_key".to_string(),
        }
    }

    pub fn with_ttl(mut self, default_ttl: usize, max_ttl: usize) -> Self {
        self.default_token_ttl = default_ttl;
        self.max_token_ttl = max_ttl;
        self
    }

    pub fn with_api_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.api_key_prefix = prefix.into();
        self
    }
}

pub fn router(state: AuthState) -> Router<()> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/token", post(token))
        .with_state(state)
}

async fn login(
    State(state): State<AuthState>,
    mut auth_session: axum_login::AuthSession<PrincipalAuthBackend>,
    AppJson(req): AppJson<LoginRequest>,
) -> Result<AppJson<LoginResponse>, HttpError> {
    let identity = state
        .repo
        .find_active_by_type_and_identifier("password", &req.username)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .ok_or(AuthError::InvalidCredentials)?;

    let valid = verify_password(&req.password, &identity.secret_hash)
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    if !valid {
        return Err(AuthError::InvalidCredentials.into());
    }

    let principal = Principal::new(identity.id, identity.user_id, AuthMethod::Session);
    auth_session
        .login(&principal)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    Ok(AppJson(LoginResponse {
        auth_id: identity.id,
        user_id: identity.user_id,
    }))
}

async fn logout(
    mut auth_session: axum_login::AuthSession<PrincipalAuthBackend>,
) -> Result<AppJson<serde_json::Value>, HttpError> {
    auth_session
        .logout()
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    Ok(AppJson(serde_json::json!({ "ok": true })))
}

async fn token(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<AppJson<TokenResponse>, HttpError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let mut req: TokenRequest = if body.is_empty() {
        TokenRequest::default()
    } else if content_type.contains("application/json") {
        serde_json::from_slice(&body).map_err(|_| HttpError::invalid_request())?
    } else if content_type.contains("application/x-www-form-urlencoded") {
        serde_urlencoded::from_bytes(&body).map_err(|_| HttpError::invalid_request())?
    } else {
        // Fall back to attempting JSON then URL-encoded
        if let Ok(r) = serde_json::from_slice(&body) {
            r
        } else if let Ok(r) = serde_urlencoded::from_bytes(&body) {
            r
        } else {
            return Err(HttpError::unsupported_media_type());
        }
    };

    // Inspect Authorization header for HTTP Basic (RFC 6749 §2.3.1) or Bearer
    if let Some(auth_header) = headers.get(axum::http::header::AUTHORIZATION)
        && let Ok(header_str) = auth_header.to_str()
    {
        let trimmed = header_str.trim();
        if let Some((scheme, val)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) {
            if scheme.eq_ignore_ascii_case("basic") {
                if let Ok(decoded_bytes) = BASE64_STANDARD.decode(val.trim())
                    && let Ok(credentials) = String::from_utf8(decoded_bytes)
                    && let Some((id, secret)) = credentials.split_once(':')
                {
                    if req.client_id.is_none() {
                        req.client_id = Some(id.to_string());
                    }
                    if req.client_secret.is_none() {
                        req.client_secret = Some(secret.to_string());
                    }
                }
            } else if scheme.eq_ignore_ascii_case("bearer")
                && req.api_key.is_none()
                && req.client_secret.is_none()
            {
                req.api_key = Some(val.trim().to_string());
            }
        }
    }

    let identity = if let Some(client_id) = req.client_id.as_deref() {
        let secret = req
            .client_secret
            .as_deref()
            .or(req.api_key.as_deref())
            .ok_or(AuthError::InvalidCredentials)?;

        let ident = state
            .repo
            .find_active_by_type_and_identifier("api_key", client_id)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;

        if !verify_api_key(secret, &ident.secret_hash) {
            return Err(AuthError::InvalidCredentials.into());
        }
        ident
    } else {
        let key = req
            .api_key
            .as_deref()
            .or(req.client_secret.as_deref())
            .ok_or(AuthError::InvalidCredentials)?;

        if !validate_api_key_prefix(key, &state.api_key_prefix) {
            return Err(AuthError::InvalidCredentials.into());
        }

        let key_hash = hash_api_key(key);
        let ident = state
            .repo
            .find_active_by_type_and_identifier("api_key", &key_hash)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;

        if !verify_api_key(key, &ident.secret_hash) {
            return Err(AuthError::InvalidCredentials.into());
        }
        ident
    };

    let requested_ttl = req.expires_in.unwrap_or(state.default_token_ttl);
    let effective_ttl = requested_ttl.min(state.max_token_ttl);
    let exp =
        (chrono::Utc::now() + chrono::Duration::seconds(effective_ttl as i64)).timestamp() as usize;
    let claims = Claims::new(identity.id, identity.user_id, exp);
    let access_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(&state.jwt_secret),
    )
    .map_err(|e| AuthError::Internal(format!("Failed to sign JWT: {e}")))?;

    Ok(AppJson(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: effective_ttl,
    }))
}
