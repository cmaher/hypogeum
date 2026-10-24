use axum::Router;
use axum::body::Bytes;
use axum::extract::{State, rejection::BytesRejection};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
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
use hypgm_errors::PublicErrorInfo;
use hypgm_http::{AppJson, HttpError};
use std::sync::Arc;

use crate::api::{LoginRequest, LoginResponse, TokenRequest, TokenResponse};
use crate::repo::AuthRepo;

// A synthetic Argon2id hash with the same cost as hash_password's defaults.
// No password can authenticate a missing identity, regardless of this comparison.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ZHVtbXktbG9naW4tc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[derive(Clone, Copy, Debug)]
pub struct PasswordConfig {
    pub max_verifications: std::num::NonZeroUsize,
}

impl Default for PasswordConfig {
    fn default() -> Self {
        Self {
            max_verifications: std::num::NonZeroUsize::new(4).expect("positive default"),
        }
    }
}

#[derive(Clone)]
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

impl std::fmt::Debug for TokenConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenConfig")
            .field("jwt_secret", &"[redacted]")
            .field("default_ttl", &self.default_ttl)
            .field("max_ttl", &self.max_ttl)
            .field("api_key_prefix", &self.api_key_prefix)
            .finish()
    }
}

// Factories use independent, ephemeral keys. Binaries must supply configured keys.
impl Default for TokenConfig {
    fn default() -> Self {
        let mut secret = vec![0; 32];
        getrandom::getrandom(&mut secret).expect("OS randomness is available");
        Self::new(secret)
    }
}

#[derive(Clone)]
pub struct AuthComponents {
    pub repo: Arc<dyn AuthRepo>,
    pub accounts: Arc<dyn exemplum_dmn_users::accounts::AccountService>,
    pub token_config: TokenConfig,
    pub(crate) password_verifications: Arc<tokio::sync::Semaphore>,
}

impl AuthComponents {
    pub fn new(
        repo: Arc<dyn AuthRepo>,
        accounts: Arc<dyn exemplum_dmn_users::accounts::AccountService>,
        token_config: TokenConfig,
        password_config: PasswordConfig,
    ) -> Self {
        Self {
            repo,
            accounts,
            token_config,
            password_verifications: Arc::new(tokio::sync::Semaphore::new(
                password_config.max_verifications.get(),
            )),
        }
    }
}

pub struct AuthComponentsFactory {
    pub password_config: PasswordConfig,
    pub repo: Arc<dyn AuthRepo>,
    pub accounts: Arc<dyn exemplum_dmn_users::accounts::AccountService>,
    pub token_config: TokenConfig,
}

impl Default for AuthComponentsFactory {
    fn default() -> Self {
        Self {
            repo: Arc::new(crate::repo::InMemoryAuthRepo::new()),
            accounts: Arc::new(crate::fixtures::ApprovedAccounts),
            password_config: PasswordConfig::default(),
            token_config: TokenConfig::default(),
        }
    }
}

impl AuthComponentsFactory {
    pub fn build(self) -> AuthComponents {
        AuthComponents::new(
            self.repo,
            self.accounts,
            self.token_config,
            self.password_config,
        )
    }
}

pub fn router(components: AuthComponents) -> Router<()> {
    Router::new()
        .route("/register", post(crate::registration::register))
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
    let principal = authenticate(&components, req).await?;
    auth_session
        .login(&principal)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    Ok(AppJson(LoginResponse {
        auth_id: principal.auth_id,
        user_id: principal.user_id,
    }))
}

pub(crate) async fn authenticate(
    components: &AuthComponents,
    req: LoginRequest,
) -> Result<Principal, HttpError> {
    let identity = components
        .repo
        .find_active_by_type_and_identifier("password", &req.username)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    let password = req.password;
    let secret_hash = identity
        .as_ref()
        .map(|identity| identity.secret_hash.as_str())
        .unwrap_or(DUMMY_PASSWORD_HASH)
        .to_owned();
    let permit = components
        .password_verifications
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;
    let span = tracing::Span::current();
    let valid = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        // Hold the permit until hashing ends, even if the HTTP request is cancelled.
        let _permit = permit;
        verify_password(&password, &secret_hash)
    })
    .await
    .map_err(|e| AuthError::Internal(e.to_string()))??;

    let identity = identity
        .filter(|_| valid)
        .ok_or(AuthError::InvalidCredentials)?;

    if !crate::registration::require_approved(components.accounts.as_ref(), identity.user_id)
        .await?
    {
        return Err(AuthError::InvalidCredentials.into());
    }
    Ok(Principal::new(
        identity.id,
        identity.user_id,
        AuthMethod::Session,
    ))
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
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let basic_auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_ascii_whitespace().next())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"));
    let result = match body {
        Ok(body) => issue_token(&components, &headers, &body).await,
        Err(error) if error.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            Err(HttpError::payload_too_large())
        }
        Err(_) => Err(HttpError::invalid_request()),
    };
    let mut response = match result {
        Ok(token) => token.into_response(),
        Err(error) => {
            let info = error.classification();
            let code = match info.status {
                StatusCode::UNAUTHORIZED => "invalid_client",
                status if status.is_server_error() => "server_error",
                _ if info.code == "unsupported_grant_type" => "unsupported_grant_type",
                _ => "invalid_request",
            };
            let mut response = (
                info.status,
                AppJson(serde_json::json!({
                    "error": code,
                    "error_description": info.message,
                })),
            )
                .into_response();
            response
                .extensions_mut()
                .insert(PublicErrorInfo::new(info.status, code, info.message));
            response.extensions_mut().insert(info.status);
            if info.status == StatusCode::UNAUTHORIZED {
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    if basic_auth {
                        "Basic realm=\"token\""
                    } else {
                        "Bearer"
                    }
                    .parse()
                    .expect("static challenge"),
                );
            }
            response
        }
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static cache directive"),
    );
    response.headers_mut().insert(
        header::PRAGMA,
        "no-cache".parse().expect("static cache directive"),
    );
    response
}

async fn issue_token(
    components: &AuthComponents,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<AppJson<TokenResponse>, HttpError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = content_type.split(';').next().unwrap_or("").trim();
    let mut req: TokenRequest = if body.is_empty() {
        TokenRequest::default()
    } else if mime.eq_ignore_ascii_case("application/json") {
        serde_json::from_slice(body).map_err(|_| HttpError::invalid_request())?
    } else if mime.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        serde_urlencoded::from_bytes(body).map_err(|_| HttpError::invalid_request())?
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

                let id = decode_basic_component(raw_id)?;
                let secret = decode_basic_component(raw_secret)?;

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

    if !crate::registration::require_approved(components.accounts.as_ref(), identity.user_id)
        .await?
    {
        return Err(AuthError::InvalidCredentials.into());
    }

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

fn decode_basic_component(value: &str) -> Result<String, HttpError> {
    let form_value = value.replace('+', " ");
    urlencoding::decode(&form_value)
        .map(|decoded| decoded.into_owned())
        .map_err(|_| HttpError::invalid_request())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn cancelled_registration_holds_capacity_until_blocking_hash_finishes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, blocked) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                blocked.recv().unwrap();
            });
            ready.await.unwrap();
            let components = AuthComponentsFactory {
                password_config: PasswordConfig {
                    max_verifications: std::num::NonZeroUsize::new(1).unwrap(),
                },
                ..Default::default()
            }
            .build();
            let capacity = components.password_verifications.clone();
            let registration = tokio::spawn(async move {
                components
                    .register(crate::RegistrationRequest {
                        display_name: "Alice".into(),
                        username: "alice".into(),
                        password: "long password!".into(),
                    })
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while capacity.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            registration.abort();
            assert!(registration.await.unwrap_err().is_cancelled());
            assert_eq!(capacity.available_permits(), 0);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let _permit =
                tokio::time::timeout(std::time::Duration::from_secs(10), capacity.acquire())
                    .await
                    .unwrap()
                    .unwrap();
        });
    }

    #[tokio::test]
    async fn configured_password_capacity_blocks_login_until_a_permit_is_released() {
        for capacity in [1, 3] {
            let repo = Arc::new(crate::repo::InMemoryAuthRepo::new());
            // An invalid hash makes an incorrectly admitted request finish immediately.
            repo.create(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "password",
                "alice",
                "invalid",
                serde_json::json!({}),
            )
            .await
            .unwrap();
            let components = AuthComponents::new(
                repo.clone(),
                Arc::new(crate::fixtures::ApprovedAccounts),
                TokenConfig::default(),
                PasswordConfig {
                    max_verifications: std::num::NonZeroUsize::new(capacity).unwrap(),
                },
            );
            let permits = components
                .password_verifications
                .clone()
                .acquire_many_owned(capacity as u32)
                .await
                .unwrap();
            let backend = PrincipalAuthBackend::new(crate::backend::ExemplumAuthBackend::from_arc(
                repo,
                Arc::new(crate::fixtures::ApprovedAccounts),
            ));
            let layer = axum_login::AuthManagerLayerBuilder::new(
                backend,
                tower_sessions::SessionManagerLayer::new(tower_sessions::MemoryStore::default()),
            )
            .build();
            let app = router(components).layer(layer);
            let request = Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"username":"alice","password":"wrong"}"#))
                .unwrap();
            let mut login = Box::pin(app.oneshot(request));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(50), &mut login)
                    .await
                    .is_err()
            );
            drop(permits);
            assert_eq!(
                login.await.unwrap().status(),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        }
    }
}
