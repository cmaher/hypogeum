use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderValue, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use exemplum_dmn_auth::{
    AuthComponentsFactory, BrowserUrl, FlowClient, PendingAccount, SessionVerifier, session_gate,
};
use exemplum_dmn_users::accounts::*;
use hypgm_authn::{OptionalAuth, RequireAuth};
use hypgmx_kratos::{Flow, FlowError, FlowKind, Identity, KratosError, Session, SessionCookie};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use uuid::Uuid;

const BROWSER: &str = "http://localhost:8080/.ory/kratos";
const COOKIE: &str = "ory_kratos_session=token";

struct Kratos {
    session: Result<Session, KratosError>,
    flow: Result<Flow, KratosError>,
    error: Result<FlowError, KratosError>,
    logout: Result<String, KratosError>,
    whoami_calls: Mutex<usize>,
    flow_cookie: Mutex<Option<String>>,
}

impl Default for Kratos {
    fn default() -> Self {
        Self {
            session: Ok(session()),
            flow: Err(KratosError::FlowGone),
            error: Err(KratosError::FlowGone),
            logout: Ok(format!("{BROWSER}/self-service/logout?token=t")),
            whoami_calls: Mutex::new(0),
            flow_cookie: Mutex::new(None),
        }
    }
}

#[async_trait]
impl SessionVerifier for Kratos {
    async fn whoami(&self, _: &SessionCookie) -> Result<Session, KratosError> {
        *self.whoami_calls.lock().unwrap() += 1;
        self.session.clone()
    }
    async fn logout_url(&self, _: &SessionCookie) -> Result<String, KratosError> {
        self.logout.clone()
    }
}

#[async_trait]
impl FlowClient for Kratos {
    async fn get_flow(
        &self,
        _: FlowKind,
        _: Uuid,
        cookie: Option<&HeaderValue>,
    ) -> Result<Flow, KratosError> {
        *self.flow_cookie.lock().unwrap() = cookie.map(|c| c.to_str().unwrap().to_owned());
        self.flow.clone()
    }
    async fn get_error(&self, _: Uuid) -> Result<FlowError, KratosError> {
        self.error.clone()
    }
}

struct Accounts {
    status: Option<ApprovalStatus>,
    ensured: Mutex<Vec<(Uuid, String, String)>>,
}

#[async_trait]
impl AccountService for Accounts {
    async fn ensure_account(
        &self,
        id: Uuid,
        username: &str,
        display_name: &str,
    ) -> Result<Account, AccountError> {
        self.ensured
            .lock()
            .unwrap()
            .push((id, username.into(), display_name.into()));
        let status = self
            .status
            .ok_or(AccountError::Repo(hypgm_errors::RepoError::NotFound))?;
        Ok(Account {
            id,
            display_name: display_name.into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            approval_status: status,
            is_admin: false,
            approved_at: None,
        })
    }
    async fn approve(&self, _: Uuid) -> Result<ManagedAccount, AccountError> {
        unreachable!()
    }
    async fn get(&self, _: Uuid) -> Result<Account, AccountError> {
        unreachable!()
    }
    async fn list(&self, _: AccountQuery) -> Result<AccountPage, AccountError> {
        unreachable!()
    }
}

fn session() -> Session {
    Session {
        id: Uuid::from_u128(1),
        active: true,
        identity: Identity {
            id: Uuid::from_u128(2),
            traits: json!({"username": "alice", "display_name": "Alice"}),
        },
    }
}

fn flow(value: serde_json::Value) -> Result<Flow, KratosError> {
    Ok(serde_json::from_value(value).unwrap())
}

struct App {
    router: Router,
    kratos: Arc<Kratos>,
    accounts: Arc<Accounts>,
}

fn app(kratos: Kratos, status: Option<ApprovalStatus>) -> App {
    let kratos = Arc::new(kratos);
    let accounts = Arc::new(Accounts {
        status,
        ensured: Mutex::new(Vec::new()),
    });
    let components = AuthComponentsFactory {
        sessions: kratos.clone(),
        flows: kratos.clone(),
        accounts: accounts.clone(),
        browser_url: BrowserUrl::new(BROWSER).unwrap(),
    }
    .build();
    let router =
        exemplum_dmn_auth::router(components.clone())
            .route(
                "/api/me",
                get(|RequireAuth(p): RequireAuth| async move {
                    format!("{} {}", p.session_id, p.user_id)
                }),
            )
            .route(
                "/",
                get(
                    |OptionalAuth(p): OptionalAuth,
                     pending: Option<axum::Extension<PendingAccount>>| async move {
                        format!("{} {}", p.is_some(), pending.is_some())
                    },
                ),
            )
            .route("/assets/app.js", get(|| async { "asset" }))
            .layer(axum::middleware::from_fn_with_state(
                components,
                session_gate,
            ));
    App {
        router,
        kratos,
        accounts,
    }
}

impl App {
    async fn send(&self, method: &str, uri: &str, cookie: Option<&str>) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        self.router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn get(&self, uri: &str, cookie: Option<&str>) -> (StatusCode, String) {
        let response = self.send("GET", uri, cookie).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }
}

fn location(response: &Response) -> &str {
    response.headers()["location"].to_str().unwrap()
}

#[tokio::test]
async fn anonymous_requests_never_reach_the_identity_service() {
    let app = app(Kratos::default(), Some(ApprovalStatus::Approved));
    assert_eq!(app.get("/api/me", None).await.0, StatusCode::UNAUTHORIZED);
    let other_cookie = Some("session=x; ory_kratos_session=");
    assert_eq!(
        app.get("/", other_cookie).await,
        (StatusCode::OK, "false false".into())
    );
    assert_eq!(*app.kratos.whoami_calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn active_approved_session_becomes_the_principal() {
    let app = app(Kratos::default(), Some(ApprovalStatus::Approved));
    let (status, body) = app.get("/api/me", Some(COOKIE)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        format!("{} {}", Uuid::from_u128(1), Uuid::from_u128(2))
    );
    assert_eq!(
        *app.accounts.ensured.lock().unwrap(),
        [(Uuid::from_u128(2), "alice".into(), "Alice".into())]
    );
    assert_eq!(app.get("/", Some(COOKIE)).await.1, "true false");
}

#[tokio::test]
async fn pending_accounts_are_unauthenticated_and_confined_to_auth_pages() {
    let app = app(Kratos::default(), Some(ApprovalStatus::Pending));
    assert_eq!(
        app.get("/api/me", Some(COOKIE)).await.0,
        StatusCode::UNAUTHORIZED
    );
    for page in ["/", "/admin/users"] {
        let response = app.send("GET", page, Some(COOKIE)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{page}");
        assert_eq!(location(&response), "/registration-pending");
    }
    let (status, body) = app.get("/registration-pending", Some(COOKIE)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("approve") && body.contains("action=\"/logout\""));
    assert_eq!(
        app.get("/assets/app.js", Some(COOKIE)).await.0,
        StatusCode::OK
    );
    let response = app.send("POST", "/logout", Some(COOKIE)).await;
    assert_eq!(
        location(&response),
        format!("{BROWSER}/self-service/logout?token=t")
    );
    let response = app.send("GET", "/login", Some(COOKIE)).await;
    assert_eq!(
        location(&response),
        format!("{BROWSER}/self-service/login/browser")
    );
}

#[tokio::test]
async fn expired_sessions_are_anonymous() {
    let kratos = Kratos {
        session: Err(KratosError::Unauthenticated),
        ..Default::default()
    };
    let app = app(kratos, Some(ApprovalStatus::Approved));
    assert_eq!(
        app.get("/api/me", Some(COOKIE)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.get("/", Some(COOKIE)).await,
        (StatusCode::OK, "false false".into())
    );
}

#[tokio::test]
async fn identity_or_account_failures_fail_closed() {
    let missing_traits = Session {
        identity: Identity {
            id: Uuid::from_u128(2),
            traits: json!({"username": "alice"}),
        },
        ..session()
    };
    for (session, accounts) in [
        (
            Err(KratosError::Unavailable),
            Some(ApprovalStatus::Approved),
        ),
        (Err(KratosError::Unexpected), Some(ApprovalStatus::Approved)),
        (Ok(missing_traits), Some(ApprovalStatus::Approved)),
        (Ok(session()), None),
    ] {
        let app = app(
            Kratos {
                session,
                ..Default::default()
            },
            accounts,
        );
        let response = app.send("GET", "/api/me", Some(COOKIE)).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let response = app.send("GET", "/", Some(COOKIE)).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }
}

#[tokio::test]
async fn flow_pages_render_each_method_group_with_the_csrf_token_escaped() {
    let csrf = json!({"type": "input", "group": "default", "attributes": {"node_type": "input", "name": "csrf_token", "type": "hidden", "value": "tok\"en", "required": true}, "messages": [], "meta": {}});
    let kratos = Kratos {
        flow: flow(json!({"ui": {
            "action": format!("{BROWSER}/self-service/settings?flow=1"),
            "method": "POST",
            "messages": [{"id": 1, "type": "error", "text": "<script>alert(1)</script>"}],
            "nodes": [
                csrf,
                {"type": "input", "group": "profile", "attributes": {"node_type": "input", "name": "traits.display_name", "type": "text", "value": "A\"lice<b>"}, "messages": [], "meta": {"label": {"id": 1, "type": "info", "text": "Display name"}}},
                {"type": "input", "group": "profile", "attributes": {"node_type": "input", "name": "method", "type": "submit", "value": "profile"}, "messages": [], "meta": {"label": {"id": 2, "type": "info", "text": "Save"}}},
                {"type": "input", "group": "password", "attributes": {"node_type": "input", "name": "password", "type": "password", "required": true, "autocomplete": "new-password"}, "messages": [{"id": 3, "type": "error", "text": "too <short>"}], "meta": {"label": {"id": 4, "type": "info", "text": "Password"}}},
                {"type": "input", "group": "password", "attributes": {"node_type": "input", "name": "method", "type": "submit", "value": "password"}, "messages": [], "meta": {"label": {"id": 5, "type": "info", "text": "Change password"}}},
                {"type": "img", "group": "default", "attributes": {"node_type": "img", "src": "x"}, "messages": [], "meta": {}}
            ]
        }})),
        ..Default::default()
    };
    let app = app(kratos, Some(ApprovalStatus::Approved));
    let uri = format!("/settings?flow={}", Uuid::from_u128(9));
    let mut request = Request::builder().uri(&uri);
    request = request
        .header("cookie", COOKIE)
        .header("cookie", "csrf_token_x=abc");
    let response = app
        .router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        app.kratos.flow_cookie.lock().unwrap().as_deref(),
        Some("ory_kratos_session=token; csrf_token_x=abc")
    );
    assert_eq!(html.matches("<form method=\"POST\"").count(), 2, "{html}");
    assert_eq!(html.matches("name=\"csrf_token\"").count(), 2);
    let (profile, password) = html.split_once("name=\"password\"").unwrap();
    assert!(profile.contains("traits.display_name") && !password.contains("traits.display_name"));
    assert!(password.contains("Change password") && !password.contains(">Save<"));
    assert!(html.contains("autocomplete=\"new-password\"") && html.contains(" required"));
    for raw in ["<script>", "<b>", "<short>", "tok\"en", "A\"lice"] {
        assert!(!html.contains(raw), "unescaped {raw}");
    }
    assert!(html.contains("&#60;script&#62;alert(1)&#60;/script&#62;"));
    assert!(!html.contains("<img"));
}

#[tokio::test]
async fn flow_pages_restart_missing_or_expired_flows_and_fail_closed() {
    let flow_uri = format!("/login?flow={}", Uuid::from_u128(9));
    for (result, expected) in [
        (Err(KratosError::FlowGone), Some("/login")),
        (Err(KratosError::Unauthenticated), Some("/login")),
        (Err(KratosError::Unavailable), None),
        (Err(KratosError::Unexpected), None),
        (
            flow(
                json!({"ui": {"action": "https://evil.example/login", "method": "POST", "nodes": []}}),
            ),
            None,
        ),
        (
            flow(
                json!({"ui": {"action": format!("{BROWSER}.evil.example/x"), "method": "POST", "nodes": []}}),
            ),
            None,
        ),
    ] {
        let app = app(
            Kratos {
                flow: result,
                ..Default::default()
            },
            Some(ApprovalStatus::Approved),
        );
        let response = app.send("GET", &flow_uri, None).await;
        match expected {
            Some(target) => {
                assert_eq!(response.status(), StatusCode::SEE_OTHER);
                assert_eq!(location(&response), target);
            }
            None => assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
    let app = app(Kratos::default(), Some(ApprovalStatus::Approved));
    for uri in ["/registration", "/registration?flow=not-a-uuid"] {
        let response = app.send("GET", uri, None).await;
        assert_eq!(
            location(&response),
            format!("{BROWSER}/self-service/registration/browser")
        );
    }
}

#[tokio::test]
async fn logout_follows_only_identity_service_urls() {
    for (logout, expected) in [
        (Err(KratosError::Unauthenticated), Some("/login".to_owned())),
        (Ok("https://evil.example/".to_owned()), None),
        (Ok(format!("{BROWSER}.evil.example/x")), None),
        (Ok(format!("{BROWSER}/../../evil")), None),
        (Ok(format!("{BROWSER}/%2e%2e/%2e%2e/evil")), None),
        (
            Ok("http://user@localhost:8080/.ory/kratos/x".to_owned()),
            None,
        ),
        (Err(KratosError::Unavailable), None),
    ] {
        let app = app(
            Kratos {
                logout,
                ..Default::default()
            },
            Some(ApprovalStatus::Approved),
        );
        let response = app.send("POST", "/logout", Some(COOKIE)).await;
        match expected {
            Some(target) => assert_eq!(location(&response), target),
            None => assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR),
        }
    }
    let app = app(Kratos::default(), Some(ApprovalStatus::Approved));
    let response = app.send("POST", "/logout", None).await;
    assert_eq!(location(&response), "/login");
}

#[tokio::test]
async fn error_page_shows_the_escaped_message_or_a_generic_one() {
    let kratos = Kratos {
        error: Ok(serde_json::from_value(
            json!({"error": {"message": "bad <thing>", "reason": "internal"}}),
        )
        .unwrap()),
        ..Default::default()
    };
    let app = app(kratos, None);
    let (status, html) = app
        .get(&format!("/error?id={}", Uuid::from_u128(3)), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("bad &#60;thing&#62;") && !html.contains("internal"));
    let (_, html) = app.get("/error", None).await;
    assert!(html.contains("Something went wrong"));
    assert!(html.contains("href=\"/login\"") && !html.contains("/logout"));
}

#[tokio::test]
async fn every_rendered_flow_value_is_escaped() {
    let kratos = Kratos {
        flow: flow(json!({"ui": {
            "action": format!("{BROWSER}/self-service/login?flow=1&q=\"<x1>&'"),
            "method": "POST",
            "nodes": [
                {"type": "input", "group": "password", "attributes": {"node_type": "input", "name": "n\"<x2>&'", "type": "text", "autocomplete": "a\"<x3>&'"}, "messages": [{"id": 1, "type": "t\"<x4>&'", "text": "m"}], "meta": {"label": {"id": 2, "type": "info", "text": "l\"<x5>&'"}}},
                {"type": "input", "group": "password", "attributes": {"node_type": "input", "name": "method", "type": "submit", "value": "v\"<x6>&'"}, "messages": [], "meta": {"label": {"id": 3, "type": "info", "text": "b\"<x7>&'"}}}
            ]
        }})),
        ..Default::default()
    };
    let app = app(kratos, None);
    let (status, html) = app
        .get(&format!("/login?flow={}", Uuid::from_u128(9)), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    for marker in 1..=7 {
        assert!(
            !html.contains(&format!("<x{marker}>")),
            "x{marker} unescaped: {html}"
        );
        assert!(
            html.contains(&format!("&#60;x{marker}&#62;")),
            "x{marker} missing"
        );
    }
}

#[tokio::test]
async fn expired_registration_flow_restarts_registration() {
    let app = app(Kratos::default(), None);
    let response = app
        .send(
            "GET",
            &format!("/registration?flow={}", Uuid::from_u128(9)),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/registration");
}

#[test]
fn browser_url_must_be_an_absolute_base_url() {
    for bad in ["", "/x", "http://h/?q", "http://h#f", "ftp://h", "http://"] {
        assert!(BrowserUrl::new(bad).is_err(), "{bad}");
    }
    assert_eq!(
        BrowserUrl::new("http://h/").unwrap(),
        BrowserUrl::new("http://h").unwrap()
    );
}
