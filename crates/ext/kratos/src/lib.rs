//! Typed client for the Ory Kratos v1 public API: session checks, self-service
//! flow UI, flow errors, browser logout and readiness. No application policy.

mod models;

use std::time::Duration;

use hypgm_errors::{ConfigError, PublicError, PublicErrorInfo};
use reqwest::header::{ACCEPT, COOKIE, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url, redirect};
use serde::de::DeserializeOwned;
use uuid::Uuid;

pub use models::{
    Flow, FlowError, FlowErrorDetail, FlowKind, Identity, Session, UiContainer, UiNode,
    UiNodeAttributes, UiNodeInputAttributes, UiNodeMeta, UiText,
};

/// Name of the cookie carrying the Kratos session token.
pub const SESSION_COOKIE: &str = "ory_kratos_session";

/// Suggested request timeout for binaries that do not override it.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Kratos failures. Variants carry no upstream bodies, URLs or cookies, so
/// `Display`, `Debug` and public error info are always generic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KratosError {
    #[error("identity session is missing or not active")]
    Unauthenticated,
    #[error("identity flow is unknown, expired or not valid for this browser")]
    FlowGone,
    #[error("identity service is unavailable")]
    Unavailable,
    #[error("unexpected identity service response")]
    Unexpected,
}

impl PublicError for KratosError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            KratosError::Unauthenticated => PublicErrorInfo::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "Authentication required",
            ),
            KratosError::FlowGone => {
                PublicErrorInfo::new(StatusCode::GONE, "flow_expired", "Flow expired")
            }
            KratosError::Unavailable | KratosError::Unexpected => PublicErrorInfo::INTERNAL_ERROR,
        }
    }
}

/// The `ory_kratos_session` cookie, ready to forward on its own. Only
/// [`session_cookie`] constructs it, so it always holds a single cookie.
#[derive(Clone)]
pub struct SessionCookie(HeaderValue);

impl std::fmt::Debug for SessionCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionCookie(..)")
    }
}

/// Returns the first non-empty `ory_kratos_session` cookie across all `Cookie`
/// headers. Values outside the RFC 6265 cookie-value alphabet are skipped so
/// nothing but that one cookie can reach Kratos.
pub fn session_cookie(headers: &HeaderMap) -> Option<SessionCookie> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| {
            *name == SESSION_COOKIE
                && !value.is_empty()
                && value.bytes().all(
                    |b| matches!(b, 0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E),
                )
        })
        .and_then(|(_, value)| HeaderValue::from_str(&format!("{SESSION_COOKIE}={value}")).ok())
        .map(SessionCookie)
}

/// Connection settings for the Kratos public API.
#[derive(Debug, Clone)]
pub struct KratosConfig {
    /// Server-side root URL of the public API, e.g. `http://127.0.0.1:4433`.
    pub public_url: Url,
    /// Total per-request timeout.
    pub request_timeout: Duration,
}

/// Kratos public-API client. Cheap to clone.
#[derive(Clone)]
pub struct KratosClient {
    http: reqwest::Client,
    base: Url,
}

impl std::fmt::Debug for KratosClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KratosClient").finish_non_exhaustive()
    }
}

impl KratosClient {
    pub fn new(config: KratosConfig) -> Result<Self, ConfigError> {
        // No TLS backend is compiled in, so https is rejected up front.
        if config.public_url.scheme() != "http" || config.public_url.path() != "/" {
            return Err(ConfigError(
                "Kratos public URL must be a plain http root URL".into(),
            ));
        }
        if config.request_timeout.is_zero() {
            return Err(ConfigError(
                "Kratos request timeout must be non-zero".into(),
            ));
        }
        // Never follow redirects: cookies must reach Kratos only.
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|_| ConfigError("failed to build Kratos HTTP client".into()))?;
        Ok(Self {
            http,
            base: config.public_url,
        })
    }

    /// Resolves the session behind the cookie, sending only that cookie. A 401
    /// or an inactive session yields [`KratosError::Unauthenticated`].
    pub async fn whoami(&self, cookie: &SessionCookie) -> Result<Session, KratosError> {
        let url = self.url("sessions/whoami", None);
        let session: Session = self
            .get(url, Some(&cookie.0), unauthenticated_on_401)
            .await?;
        if session.active {
            Ok(session)
        } else {
            Err(KratosError::Unauthenticated)
        }
    }

    /// Fetches a browser flow. `cookie_header` is the browser's full `Cookie`
    /// header, which Kratos needs for its CSRF check. Unknown, expired or
    /// foreign flows yield [`KratosError::FlowGone`]; a settings flow without a
    /// session yields [`KratosError::Unauthenticated`].
    pub async fn get_flow(
        &self,
        kind: FlowKind,
        id: Uuid,
        cookie_header: Option<&HeaderValue>,
    ) -> Result<Flow, KratosError> {
        let path = format!("self-service/{}/flows", kind.as_str());
        let url = self.url(&path, Some(id));
        self.get(url, cookie_header, |status| match status.as_u16() {
            401 => Some(KratosError::Unauthenticated),
            403 => {
                // CSRF/cookie mismatch; repeated hits suggest a misconfiguration.
                tracing::debug!(status = 403, "Kratos rejected flow for this browser");
                Some(KratosError::FlowGone)
            }
            400 | 404 | 410 => Some(KratosError::FlowGone),
            _ => None,
        })
        .await
    }

    /// Fetches a self-service error by id. Unknown ids yield
    /// [`KratosError::FlowGone`].
    pub async fn get_error(&self, id: Uuid) -> Result<FlowError, KratosError> {
        let url = self.url("self-service/errors", Some(id));
        self.get(url, None, |status| {
            matches!(status.as_u16(), 400 | 403 | 404 | 410).then_some(KratosError::FlowGone)
        })
        .await
    }

    /// Creates a browser logout flow for the session and returns the URL the
    /// browser should visit to end it.
    pub async fn logout_url(&self, cookie: &SessionCookie) -> Result<String, KratosError> {
        let url = self.url("self-service/logout/browser", None);
        let flow: models::LogoutFlow = self
            .get(url, Some(&cookie.0), unauthenticated_on_401)
            .await?;
        Ok(flow.logout_url)
    }

    /// Checks `GET /health/ready`. Any failure is [`KratosError::Unavailable`].
    pub async fn ready(&self) -> Result<(), KratosError> {
        let response = self
            .http
            .get(self.url("health/ready", None))
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        if response.status() == StatusCode::OK {
            Ok(())
        } else {
            tracing::warn!(status = response.status().as_u16(), "Kratos is not ready");
            Err(KratosError::Unavailable)
        }
    }

    fn url(&self, path: &str, id: Option<Uuid>) -> Url {
        let mut url = self.base.join(path).expect("static relative path");
        if let Some(id) = id {
            url.query_pairs_mut()
                .append_pair("id", &id.hyphenated().to_string());
        }
        url
    }

    async fn get<T: DeserializeOwned>(
        &self,
        url: Url,
        cookie: Option<&HeaderValue>,
        classify: impl Fn(StatusCode) -> Option<KratosError>,
    ) -> Result<T, KratosError> {
        let mut request = self.http.get(url).header(ACCEPT, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, cookie.clone());
        }
        let response = request
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        let status = response.status();
        if status.is_success() {
            let body = response
                .bytes()
                .await
                .map_err(|error| transport_error(&error))?;
            return serde_json::from_slice(&body).map_err(|_| {
                tracing::warn!("Kratos returned an unparseable response body");
                KratosError::Unexpected
            });
        }
        if let Some(error) = classify(status) {
            return Err(error);
        }
        tracing::warn!(status = status.as_u16(), "Kratos request failed");
        Err(if status.is_server_error() {
            KratosError::Unavailable
        } else {
            KratosError::Unexpected
        })
    }
}

fn unauthenticated_on_401(status: StatusCode) -> Option<KratosError> {
    (status == StatusCode::UNAUTHORIZED).then_some(KratosError::Unauthenticated)
}

fn transport_error(error: &reqwest::Error) -> KratosError {
    tracing::warn!(
        timeout = error.is_timeout(),
        connect = error.is_connect(),
        "Kratos request did not complete"
    );
    KratosError::Unavailable
}
