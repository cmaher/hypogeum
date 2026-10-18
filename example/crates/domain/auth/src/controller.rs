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

#[derive(Clone, Debug)]
pub struct TokenConfig {
    pub jwt_secret: Vec<u8>,
    pub default_ttl: usize,
    pub max_ttl: usize,
    pub api_key_prefix: String,
}

impl TokenConfig {
    pub fn new(jwt_secret: Vec<u8>) -> Self {
        Self {
            jwt_secret,
            default_ttl: 3600,
            max_ttl: 86400,
            api_key_prefix: "exmpl_key".to_string(),
        }
    }
}

impl Default for TokenConfig {
    fn default() -> Self {
        Self::new(b"exemplum-default-secret-key-32bytes!".to_vec())
    }
}

#[derive(Clone)]
pub struct AuthComponents {
    pub repo: Arc<dyn AuthRepo>,
    pub token_config: TokenConfig,
}

impl AuthComponents {
    pub fn new(repo: Arc<dyn AuthRepo>, token_config: TokenConfig) -> Self {
        Self { repo, token_config }
    }
}

pub struct AuthComponentsFactory {
    pub repo: Arc<dyn AuthRepo>,
    pub token_config: TokenConfig,
}

impl Default for AuthComponentsFactory {
    fn default() -> Self {
        Self {
            repo: Arc::new(crate::repo::InMemoryAuthRepo::new()),
            token_config: TokenConfig::default(),
        }
    }
}

impl AuthComponentsFactory {
    pub fn build(self) -> AuthComponents {
        AuthComponents::new(self.repo, self.token_config)
    }
}

pub fn router(components: AuthComponents) -> Router<()> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/token", post(token))
        .with_state(components)
}

async fn login(
    State(components): State<AuthComponents>,
    mut auth_session: axum_login::AuthSession<PrincipalAuthBackend>,
    AppJson(req): AppJson<LoginRequest>,
) -> Result<AppJson<LoginResponse>, HttpError> {
    let identity = components
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
    State(components): State<AuthComponents>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<AppJson<TokenResponse>, HttpError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = content_type.split(';').next().unwrap_or("").trim();
    let mut req: TokenRequest = if body.is_empty() {
        TokenRequest::default()
    } else if mime.eq_ignore_ascii_case("application/json") {
        serde_json::from_slice(&body).map_err(|_| HttpError::invalid_request())?
    } else if mime.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        serde_urlencoded::from_bytes(&body).map_err(|_| HttpError::invalid_request())?
    } else {
        return Err(HttpError::unsupported_media_type());
    };

    if let Some(ref gt) = req.grant_type
        && gt != "client_credentials"
        && gt != "api_key"
    {
        return Err(HttpError::custom(
            axum::http::StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Unsupported grant type",
        ));
    }

    // Inspect Authorization header for HTTP Basic (RFC 6749 §2.3.1) or Bearer
    if let Some(auth_header) = headers.get(axum::http::header::AUTHORIZATION) {
        let header_str = auth_header
            .to_str()
            .map_err(|_| HttpError::invalid_request())?;
        let trimmed = header_str.trim();
        if let Some((scheme, val)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) {
            if scheme.eq_ignore_ascii_case("basic") {
                // Reject if credentials were also provided in the request body (RFC 6749 §2.3)
                if req.client_id.is_some() || req.client_secret.is_some() || req.api_key.is_some() {
                    return Err(HttpError::invalid_request());
                }

                let decoded_bytes = BASE64_STANDARD
                    .decode(val.trim())
                    .map_err(|_| HttpError::invalid_request())?;
                let credentials =
                    String::from_utf8(decoded_bytes).map_err(|_| HttpError::invalid_request())?;
                let (raw_id, raw_secret) = credentials
                    .split_once(':')
                    .ok_or_else(HttpError::invalid_request)?;

                let id = urlencoding::decode(raw_id)
                    .map_err(|_| HttpError::invalid_request())?
                    .into_owned();
                let secret = urlencoding::decode(raw_secret)
                    .map_err(|_| HttpError::invalid_request())?
                    .into_owned();

                req.client_id = Some(id);
                req.client_secret = Some(secret);
            } else if scheme.eq_ignore_ascii_case("bearer") {
                // Reject if credentials were also provided in the request body (RFC 6749 §2.3)
                if req.client_id.is_some() || req.client_secret.is_some() || req.api_key.is_some() {
                    return Err(HttpError::invalid_request());
                }
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

        let ident = components
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

        if !validate_api_key_prefix(key, &components.token_config.api_key_prefix) {
            return Err(AuthError::InvalidCredentials.into());
        }

        let key_hash = hash_api_key(key);
        let ident = components
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

    let requested_ttl = req
        .expires_in
        .unwrap_or(components.token_config.default_ttl);
    let effective_ttl = requested_ttl.min(components.token_config.max_ttl);
    let exp =
        (chrono::Utc::now() + chrono::Duration::seconds(effective_ttl as i64)).timestamp() as usize;
    let claims = Claims::new(identity.id, identity.user_id, exp);
    let access_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(&components.token_config.jwt_secret),
    )
    .map_err(|e| AuthError::Internal(format!("Failed to sign JWT: {e}")))?;

    Ok(AppJson(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: effective_ttl,
    }))
}
