use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::Router;
use hypgm_authn::{
    error::AuthError,
    jwt::Claims,
    principal::{AuthMethod, Principal},
    session::PrincipalAuthBackend,
};
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

use super::api::{LoginRequest, LoginResponse, TokenRequest, TokenResponse};
use super::password::{hash_api_key, verify_api_key, verify_password};
use super::repo::AuthRepo;

#[derive(Clone)]
pub struct AuthState {
    pub repo: Arc<dyn AuthRepo>,
    pub jwt_secret: Arc<Vec<u8>>,
}

impl AuthState {
    pub fn new(repo: Arc<dyn AuthRepo>, jwt_secret: Vec<u8>) -> Self {
        Self {
            repo,
            jwt_secret: Arc::new(jwt_secret),
        }
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
    let mut req: TokenRequest = if !body.is_empty() {
        if body.starts_with(b"{") {
            serde_json::from_slice(&body).unwrap_or_default()
        } else {
            serde_urlencoded::from_bytes(&body).unwrap_or_default()
        }
    } else {
        TokenRequest::default()
    };

    // If client credentials/api key not in body, check Authorization header
    if req.api_key.is_none() && req.client_secret.is_none() {
        if let Some(auth_header) = headers.get(axum::http::header::AUTHORIZATION) {
            if let Ok(header_str) = auth_header.to_str() {
                let trimmed = header_str.trim();
                if let Some((scheme, val)) = trimmed.split_once(' ') {
                    if scheme.eq_ignore_ascii_case("bearer") {
                        req.api_key = Some(val.trim().to_string());
                    }
                }
            }
        }
    }

    let identity = if let Some(ref client_id) = req.client_id {
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

        let key_hash = hash_api_key(key);
        let ident = match state
            .repo
            .find_active_by_type_and_identifier("api_key", key)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
        {
            Some(i) => {
                if !verify_api_key(key, &i.secret_hash) && i.secret_hash != key_hash {
                    return Err(AuthError::InvalidCredentials.into());
                }
                i
            }
            None => {
                let i = state
                    .repo
                    .find_active_by_type_and_identifier("api_key", &key_hash)
                    .await
                    .map_err(|e| AuthError::Internal(e.to_string()))?
                    .ok_or(AuthError::InvalidCredentials)?;

                if !verify_api_key(key, &i.secret_hash) && i.secret_hash != key_hash {
                    return Err(AuthError::InvalidCredentials.into());
                }
                i
            }
        };
        ident
    };

    let ttl_seconds = 3600;
    let exp =
        (chrono::Utc::now() + chrono::Duration::seconds(ttl_seconds as i64)).timestamp() as usize;
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
        expires_in: ttl_seconds,
    }))
}
