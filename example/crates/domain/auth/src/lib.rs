//! Browser authentication through the identity service (Ory Kratos):
//! [`session_gate`] resolves every request's session into a
//! [`hypgm_authn::Principal`], and [`router`] serves the self-service flow
//! pages and logout. Credentials, sessions and flow CSRF belong to Kratos.
//!
//! Identity traits (`username`, `display_name`) are immutable by design: the
//! Kratos `profile` settings method is disabled, so the copy made by
//! `ensure_account` on first sign-in never drifts from Kratos.
mod gate;
mod pages;

#[doc(hidden)]
pub mod fixtures;

use async_trait::async_trait;
use axum::http::HeaderValue;
use exemplum_dmn_users::accounts::AccountService;
use hypgmx_kratos::{Flow, FlowError, FlowKind, KratosClient, KratosError, Session, SessionCookie};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

pub use gate::{PendingAccount, session_gate};
pub use pages::router;

/// Session checks against the identity service.
#[async_trait]
pub trait SessionVerifier: Send + Sync {
    /// The active session behind the cookie.
    async fn whoami(&self, cookie: &SessionCookie) -> Result<Session, KratosError>;
    /// The browser URL that ends the session.
    async fn logout_url(&self, cookie: &SessionCookie) -> Result<String, KratosError>;
}

/// Self-service flow lookups against the identity service.
#[async_trait]
pub trait FlowClient: Send + Sync {
    /// Fetches a flow, forwarding the browser's `Cookie` header for CSRF.
    async fn get_flow(
        &self,
        kind: FlowKind,
        id: Uuid,
        cookie_header: Option<&HeaderValue>,
    ) -> Result<Flow, KratosError>;
    async fn get_error(&self, id: Uuid) -> Result<FlowError, KratosError>;
}

#[async_trait]
impl SessionVerifier for KratosClient {
    async fn whoami(&self, cookie: &SessionCookie) -> Result<Session, KratosError> {
        KratosClient::whoami(self, cookie).await
    }
    async fn logout_url(&self, cookie: &SessionCookie) -> Result<String, KratosError> {
        KratosClient::logout_url(self, cookie).await
    }
}

#[async_trait]
impl FlowClient for KratosClient {
    async fn get_flow(
        &self,
        kind: FlowKind,
        id: Uuid,
        cookie_header: Option<&HeaderValue>,
    ) -> Result<Flow, KratosError> {
        KratosClient::get_flow(self, kind, id, cookie_header).await
    }
    async fn get_error(&self, id: Uuid) -> Result<FlowError, KratosError> {
        KratosClient::get_error(self, id).await
    }
}

/// Browser-facing location of the identity service's public API, e.g.
/// `http://localhost:8080/.ory/kratos`. Flow redirects and form actions point
/// here, and only URLs beneath it are followed or rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserUrl {
    /// The parsed base; its path never ends with `/`.
    base: Url,
}

impl BrowserUrl {
    /// Accepts an absolute `http(s)` URL without credentials, query or
    /// fragment; a trailing slash is ignored.
    pub fn new(url: &str) -> Result<Self, hypgm_errors::ConfigError> {
        let invalid = || {
            hypgm_errors::ConfigError("Kratos browser URL must be an absolute http(s) URL".into())
        };
        let mut base = Url::parse(url).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(invalid());
        }
        let path = base.path().trim_end_matches('/').to_owned();
        base.set_path(&path);
        Ok(Self { base })
    }

    fn join(&self, path: &str) -> String {
        format!("{}/{path}", self.base.as_str().trim_end_matches('/'))
    }

    /// Whether `url`, once parsed (dot segments resolved), points beneath this
    /// base on the same origin, so following it cannot leave the identity service.
    fn contains(&self, url: &str) -> bool {
        let Ok(candidate) = Url::parse(url) else {
            return false;
        };
        let base_path = self.base.path().trim_end_matches('/');
        candidate.scheme() == self.base.scheme()
            && candidate.host() == self.base.host()
            && candidate.port_or_known_default() == self.base.port_or_known_default()
            && candidate.username().is_empty()
            && candidate.password().is_none()
            && candidate
                .path()
                .strip_prefix(base_path)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

#[derive(Clone)]
pub struct AuthComponents {
    sessions: Arc<dyn SessionVerifier>,
    flows: Arc<dyn FlowClient>,
    accounts: Arc<dyn AccountService>,
    browser_url: BrowserUrl,
}

impl AuthComponents {
    pub fn new(
        sessions: Arc<dyn SessionVerifier>,
        flows: Arc<dyn FlowClient>,
        accounts: Arc<dyn AccountService>,
        browser_url: BrowserUrl,
    ) -> Self {
        Self {
            sessions,
            flows,
            accounts,
            browser_url,
        }
    }
}

/// Test and local defaults: no valid sessions and approved fixture accounts.
pub struct AuthComponentsFactory {
    pub sessions: Arc<dyn SessionVerifier>,
    pub flows: Arc<dyn FlowClient>,
    pub accounts: Arc<dyn AccountService>,
    pub browser_url: BrowserUrl,
}

impl Default for AuthComponentsFactory {
    fn default() -> Self {
        Self {
            sessions: Arc::new(fixtures::NoSessions),
            flows: Arc::new(fixtures::NoSessions),
            accounts: Arc::new(fixtures::ApprovedAccounts),
            browser_url: BrowserUrl::new("http://localhost:8080/.ory/kratos")
                .expect("valid default URL"),
        }
    }
}

impl AuthComponentsFactory {
    pub fn build(self) -> AuthComponents {
        AuthComponents::new(self.sessions, self.flows, self.accounts, self.browser_url)
    }
}
