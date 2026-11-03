use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use hypgm_errors::{PublicError, PublicErrorInfo};
use hypgmx_kratos::{
    FlowKind, KratosClient, KratosConfig, KratosError, SESSION_COOKIE, SessionCookie,
    UiNodeAttributes, session_cookie,
};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use uuid::Uuid;

const SESSION_ID: &str = "8f2c0e3e-5d0b-4b65-9a52-3c3c8b3f0a11";
const IDENTITY_ID: &str = "4d5a8b2e-0f3c-4c1d-9f2a-7d1e6b9c0a22";
const FLOW_ID: &str = "1b7e9c4d-2a3f-4e5b-8c6d-9e0f1a2b3c44";

const SESSION_JSON: &str = r#"{
  "id": "8f2c0e3e-5d0b-4b65-9a52-3c3c8b3f0a11",
  "active": true,
  "expires_at": "2026-11-04T12:00:00.000000Z",
  "authenticated_at": "2026-11-03T12:00:00.000000Z",
  "authenticator_assurance_level": "aal1",
  "authentication_methods": [{"method": "password", "aal": "aal1", "completed_at": "2026-11-03T12:00:00.000000Z"}],
  "issued_at": "2026-11-03T12:00:00.000000Z",
  "identity": {
    "id": "4d5a8b2e-0f3c-4c1d-9f2a-7d1e6b9c0a22",
    "schema_id": "default",
    "schema_url": "http://127.0.0.1:4433/schemas/ZGVmYXVsdA",
    "state": "active",
    "state_changed_at": "2026-11-03T11:59:00.000000Z",
    "traits": {"username": "alice", "display_name": "Alice"},
    "metadata_public": null,
    "created_at": "2026-11-03T11:59:00.000000Z",
    "updated_at": "2026-11-03T11:59:00.000000Z",
    "organization_id": null
  },
  "devices": [{"id": "0c1d2e3f-4a5b-4c6d-8e7f-9a0b1c2d3e55", "ip_address": "172.18.0.1", "user_agent": "Mozilla/5.0", "location": ""}]
}"#;

const LOGIN_FLOW_JSON: &str = r#"{
  "id": "1b7e9c4d-2a3f-4e5b-8c6d-9e0f1a2b3c44",
  "organization_id": null,
  "type": "browser",
  "expires_at": "2026-11-03T13:00:00.000000Z",
  "issued_at": "2026-11-03T12:00:00.000000Z",
  "request_url": "http://localhost:8080/.ory/kratos/self-service/login/browser",
  "ui": {
    "action": "http://localhost:8080/.ory/kratos/self-service/login?flow=1b7e9c4d-2a3f-4e5b-8c6d-9e0f1a2b3c44",
    "method": "POST",
    "nodes": [
      {"type": "input", "group": "default", "attributes": {"name": "csrf_token", "type": "hidden", "value": "c3JmLXRva2Vu", "required": true, "disabled": false, "node_type": "input"}, "messages": [], "meta": {}},
      {"type": "input", "group": "default", "attributes": {"name": "identifier", "type": "text", "value": "alice", "required": true, "autocomplete": "username", "disabled": false, "node_type": "input"}, "messages": [], "meta": {"label": {"id": 1070004, "text": "Username", "type": "info", "context": {"title": "Username"}}}},
      {"type": "input", "group": "password", "attributes": {"name": "password", "type": "password", "required": true, "autocomplete": "current-password", "disabled": false, "node_type": "input"}, "messages": [{"id": 4000002, "text": "Property password is missing.", "type": "error", "context": {"property": "password"}}], "meta": {"label": {"id": 1070001, "text": "Password", "type": "info"}}},
      {"type": "input", "group": "password", "attributes": {"name": "method", "type": "submit", "value": "password", "disabled": false, "node_type": "input"}, "messages": [], "meta": {"label": {"id": 1010022, "text": "Sign in with password", "type": "info"}}},
      {"type": "a", "group": "default", "attributes": {"href": "http://localhost:8080/registration", "title": {"id": 1, "text": "Register", "type": "info"}, "id": "register", "node_type": "a"}, "messages": [], "meta": {}}
    ],
    "messages": [{"id": 4000006, "text": "The provided credentials are invalid, check for spelling mistakes in your password or username, email address, or phone number.", "type": "error", "context": {}}]
  },
  "created_at": "2026-11-03T12:00:00.000000Z",
  "updated_at": "2026-11-03T12:00:00.000000Z",
  "refresh": false,
  "requested_aal": "aal1",
  "state": "choose_method"
}"#;

const UNAUTHORIZED_JSON: &str = r#"{"error": {"code": 401, "status": "Unauthorized", "reason": "No valid session credentials found in the request.", "message": "The request could not be authorized"}}"#;

const SECRET: &str = "postgres://kratos:hunter2@db/kratos";

struct Stub {
    status: u16,
    body: String,
    delay: Duration,
}

impl Stub {
    fn new(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_owned(),
            delay: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone)]
struct Recorded {
    uri: String,
    cookies: Vec<String>,
    accept: Option<String>,
}

struct Fake {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Fake {
    fn client(&self) -> KratosClient {
        client_for(&self.url, Duration::from_secs(2))
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn client_for(url: &str, timeout: Duration) -> KratosClient {
    KratosClient::new(KratosConfig {
        public_url: url.parse().unwrap(),
        request_timeout: timeout,
    })
    .unwrap()
}

async fn spawn(stub: Stub) -> Fake {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorder = requests.clone();
    let stub = Arc::new(stub);
    let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let recorder = recorder.clone();
        let stub = stub.clone();
        async move {
            recorder.lock().unwrap().push(Recorded {
                uri: uri.to_string(),
                cookies: headers
                    .get_all(header::COOKIE)
                    .iter()
                    .map(|v| v.to_str().unwrap().to_owned())
                    .collect(),
                accept: headers
                    .get(header::ACCEPT)
                    .map(|v| v.to_str().unwrap().to_owned()),
            });
            tokio::time::sleep(stub.delay).await;
            let status = StatusCode::from_u16(stub.status).unwrap();
            let mut response: Response = (status, stub.body.clone()).into_response();
            if status.is_redirection() {
                response
                    .headers_mut()
                    .insert(header::LOCATION, "http://evil.example/".parse().unwrap());
            }
            response
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fake {
        url: format!("http://{addr}"),
        requests,
    }
}

fn flow_id() -> Uuid {
    FLOW_ID.parse().unwrap()
}

fn cookie(value: &str) -> SessionCookie {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("{SESSION_COOKIE}={value}").parse().unwrap(),
    );
    session_cookie(&headers).unwrap()
}

#[tokio::test]
async fn whoami_parses_session_and_forwards_only_session_cookie() {
    let fake = spawn(Stub::new(200, SESSION_JSON)).await;
    let mut headers = HeaderMap::new();
    headers.append(
        header::COOKIE,
        "theme=dark; csrf_token_abc=xyz".parse().unwrap(),
    );
    headers.append(
        header::COOKIE,
        "a=b; ory_kratos_session=; ory_kratos_session=MTY4tok-en_=; c=d"
            .parse()
            .unwrap(),
    );
    let cookie = session_cookie(&headers).unwrap();
    assert_eq!(format!("{cookie:?}"), "SessionCookie(..)");
    let session = fake.client().whoami(&cookie).await.unwrap();

    assert_eq!(session.id.to_string(), SESSION_ID);
    assert!(session.active);
    assert_eq!(session.identity.id.to_string(), IDENTITY_ID);
    assert_eq!(session.identity.traits["username"], "alice");
    assert_eq!(session.identity.traits["display_name"], "Alice");

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].uri, "/sessions/whoami");
    assert_eq!(requests[0].cookies, vec!["ory_kratos_session=MTY4tok-en_="]);
    assert_eq!(requests[0].accept.as_deref(), Some("application/json"));
}

#[test]
fn session_cookie_rejects_missing_empty_or_malformed_values() {
    assert!(session_cookie(&HeaderMap::new()).is_none());
    for value in [
        "x=y",
        "ory_kratos_session=; x=y",
        "ory_kratos_session=a,b=c",
        "ory_kratos_session=a b",
        "ory_kratos_session=a\tb",
        "ory_kratos_session=\"quoted\"",
        "ory_kratos_session=a\\b",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, value.parse().unwrap());
        assert!(session_cookie(&headers).is_none(), "{value:?}");
    }
}

#[tokio::test]
async fn whoami_maps_statuses_and_fails_closed() {
    let inactive = SESSION_JSON.replace("\"active\": true", "\"active\": false");
    let no_active = SESSION_JSON.replace("\"active\": true,", "");
    let no_traits = SESSION_JSON.replace(
        r#""traits": {"username": "alice", "display_name": "Alice"},"#,
        "",
    );
    assert_ne!(no_traits, SESSION_JSON);
    let mut cases = vec![
        (
            Stub::new(401, UNAUTHORIZED_JSON),
            KratosError::Unauthenticated,
        ),
        (Stub::new(200, &inactive), KratosError::Unauthenticated),
        (Stub::new(403, UNAUTHORIZED_JSON), KratosError::Unexpected),
        (Stub::new(200, &no_active), KratosError::Unexpected),
        (Stub::new(200, &no_traits), KratosError::Unexpected),
        (Stub::new(200, "{not json"), KratosError::Unexpected),
        (Stub::new(200, r#"{"id": "nope"}"#), KratosError::Unexpected),
        (Stub::new(302, ""), KratosError::Unexpected),
        // A redirect is never parsed, even when its body is a valid session.
        (Stub::new(302, SESSION_JSON), KratosError::Unexpected),
    ];
    for status in [400, 404, 409, 418, 429] {
        cases.push((Stub::new(status, SECRET), KratosError::Unexpected));
    }
    for status in 500..=504 {
        cases.push((Stub::new(status, SECRET), KratosError::Unavailable));
    }
    for (stub, expected) in cases {
        let status = stub.status;
        let fake = spawn(stub).await;
        assert_eq!(
            fake.client().whoami(&cookie("tok")).await,
            Err(expected),
            "status {status}"
        );
        // Redirects are never followed.
        assert_eq!(fake.requests().len(), 1);
    }
}

fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn logs_never_contain_bodies_cookies_or_urls() {
    let logs = LogBuffer::default();
    let writer = logs.clone();
    // Only this crate's events: transport-level crates log peers at debug.
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .with_filter(Targets::new().with_target("hypgmx_kratos", tracing::Level::TRACE)),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let secret = cookie("secret-session-token");
    let mut hosts = Vec::new();
    for stub in [Stub::new(500, SECRET), Stub::new(200, SECRET)] {
        let fake = spawn(stub).await;
        assert!(fake.client().whoami(&secret).await.is_err());
        hosts.push(fake.url.trim_start_matches("http://").to_owned());
    }
    let closed = closed_url();
    let client = client_for(&closed, Duration::from_secs(2));
    assert_eq!(client.whoami(&secret).await, Err(KratosError::Unavailable));
    assert_eq!(client.ready().await, Err(KratosError::Unavailable));
    hosts.push(closed.trim_start_matches("http://").to_owned());

    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(output.contains("Kratos"), "expected warnings: {output}");
    let mut leaks = vec!["hunter2", "secret-session-token", SESSION_COOKIE];
    leaks.extend(hosts.iter().map(String::as_str));
    for leak in leaks {
        assert!(!output.contains(leak), "{leak} leaked: {output}");
    }
}

#[tokio::test]
async fn timeouts_are_unavailable() {
    let mut slow = Stub::new(200, SESSION_JSON);
    slow.delay = Duration::from_secs(5);
    let fake = spawn(slow).await;
    let client = client_for(&fake.url, Duration::from_millis(200));
    assert_eq!(
        client.whoami(&cookie("tok")).await,
        Err(KratosError::Unavailable)
    );
}

#[tokio::test]
async fn get_flow_parses_ui_and_forwards_cookie_header() {
    let fake = spawn(Stub::new(200, LOGIN_FLOW_JSON)).await;
    let cookies = "csrf_token_806060ca=abc; ory_kratos_session=tok";
    let flow = fake
        .client()
        .get_flow(
            FlowKind::Login,
            flow_id(),
            Some(&HeaderValue::from_static(cookies)),
        )
        .await
        .unwrap();

    assert_eq!(flow.ui.method, "POST");
    assert!(flow.ui.action.ends_with(&format!("login?flow={FLOW_ID}")));
    assert_eq!(flow.ui.messages[0].kind, "error");
    assert_eq!(flow.ui.nodes.len(), 5);

    let UiNodeAttributes::Input(csrf) = &flow.ui.nodes[0].attributes else {
        panic!("csrf node should be an input");
    };
    assert_eq!(
        (csrf.name.as_str(), csrf.input_type.as_str()),
        ("csrf_token", "hidden")
    );
    assert_eq!(csrf.value_text().as_deref(), Some("c3JmLXRva2Vu"));
    assert!(csrf.required);

    let password = &flow.ui.nodes[2];
    assert_eq!(password.group, "password");
    assert_eq!(password.meta.label.as_ref().unwrap().text, "Password");
    assert_eq!(password.messages[0].text, "Property password is missing.");
    let UiNodeAttributes::Input(attrs) = &password.attributes else {
        panic!("password node should be an input");
    };
    assert_eq!(attrs.value_text(), None);
    assert_eq!(attrs.autocomplete.as_deref(), Some("current-password"));

    assert_eq!(flow.ui.nodes[4].attributes, UiNodeAttributes::Other);

    let requests = fake.requests();
    assert_eq!(
        requests[0].uri,
        format!("/self-service/login/flows?id={FLOW_ID}")
    );
    assert_eq!(requests[0].cookies, vec![cookies]);
}

#[tokio::test]
async fn get_flow_uses_kind_path_and_omits_absent_cookie() {
    for (kind, segment) in [
        (FlowKind::Registration, "registration"),
        (FlowKind::Settings, "settings"),
    ] {
        let fake = spawn(Stub::new(200, LOGIN_FLOW_JSON)).await;
        fake.client().get_flow(kind, flow_id(), None).await.unwrap();
        let requests = fake.requests();
        assert_eq!(
            requests[0].uri,
            format!("/self-service/{segment}/flows?id={FLOW_ID}")
        );
        assert!(requests[0].cookies.is_empty());
    }
}

#[tokio::test]
async fn get_flow_maps_statuses() {
    let gone = r#"{"error": {"id": "self_service_flow_expired", "code": 410, "status": "Gone", "reason": "The self-service flow has expired", "message": "The flow has expired"}}"#;
    let cases = [
        (400, KratosError::FlowGone),
        (403, KratosError::FlowGone),
        (404, KratosError::FlowGone),
        (410, KratosError::FlowGone),
        (401, KratosError::Unauthenticated),
        (502, KratosError::Unavailable),
        (409, KratosError::Unexpected),
    ];
    for (status, expected) in cases {
        let fake = spawn(Stub::new(status, gone)).await;
        let result = fake
            .client()
            .get_flow(
                FlowKind::Settings,
                flow_id(),
                Some(&HeaderValue::from_static("a=b")),
            )
            .await;
        assert_eq!(result, Err(expected), "status {status}");
    }
}

#[tokio::test]
async fn get_error_parses_user_facing_fields_only() {
    let body = format!(
        r#"{{"id": "{FLOW_ID}", "error": {{"code": 500, "status": "Internal Server Error", "message": "An internal server error occurred, please contact the system administrator", "reason": "{SECRET}", "debug": "{SECRET}"}}, "created_at": "2026-11-03T12:00:00Z", "updated_at": "2026-11-03T12:00:00Z"}}"#
    );
    let fake = spawn(Stub::new(200, &body)).await;
    let error = fake.client().get_error(flow_id()).await.unwrap();
    assert_eq!(
        error.error.message.as_deref(),
        Some("An internal server error occurred, please contact the system administrator")
    );
    assert!(!format!("{error:?}").contains("hunter2"));
    assert_eq!(
        fake.requests()[0].uri,
        format!("/self-service/errors?id={FLOW_ID}")
    );
    assert!(fake.requests()[0].cookies.is_empty());

    let fake = spawn(Stub::new(404, "{}")).await;
    assert_eq!(
        fake.client().get_error(flow_id()).await,
        Err(KratosError::FlowGone)
    );
}

#[tokio::test]
async fn logout_url_returns_url_and_forwards_only_session_cookie() {
    let body = r#"{"logout_url": "http://localhost:8080/.ory/kratos/self-service/logout?token=ory_lo_abc", "logout_token": "ory_lo_abc"}"#;
    let fake = spawn(Stub::new(200, body)).await;
    let url = fake.client().logout_url(&cookie("tok")).await.unwrap();
    assert_eq!(
        url,
        "http://localhost:8080/.ory/kratos/self-service/logout?token=ory_lo_abc"
    );
    let requests = fake.requests();
    assert_eq!(requests[0].uri, "/self-service/logout/browser");
    assert_eq!(requests[0].cookies, vec!["ory_kratos_session=tok"]);

    for (status, expected) in [
        (401, KratosError::Unauthenticated),
        (403, KratosError::Unexpected),
        (503, KratosError::Unavailable),
    ] {
        let fake = spawn(Stub::new(status, UNAUTHORIZED_JSON)).await;
        assert_eq!(
            fake.client().logout_url(&cookie("tok")).await,
            Err(expected),
            "status {status}"
        );
    }
}

#[tokio::test]
async fn ready_checks_health_endpoint() {
    let fake = spawn(Stub::new(200, r#"{"status": "ok"}"#)).await;
    assert_eq!(fake.client().ready().await, Ok(()));
    assert_eq!(fake.requests()[0].uri, "/health/ready");

    let fake = spawn(Stub::new(503, r#"{"errors": {"database": "not ready"}}"#)).await;
    assert_eq!(fake.client().ready().await, Err(KratosError::Unavailable));
}

#[tokio::test]
async fn errors_are_redacted() {
    let fake = spawn(Stub::new(500, SECRET)).await;
    let error = fake
        .client()
        .whoami(&cookie("secret-session-token"))
        .await
        .unwrap_err();
    let info = error.public_error();
    let rendered = format!("{error} {error:?} {info} {:?}", fake.client());
    for leak in [
        "hunter2",
        "secret-session-token",
        "127.0.0.1",
        SESSION_COOKIE,
    ] {
        assert!(!rendered.contains(leak), "{leak} leaked: {rendered}");
    }
    assert_eq!(info, PublicErrorInfo::INTERNAL_ERROR);
    assert_eq!(
        KratosError::Unexpected.public_error(),
        PublicErrorInfo::INTERNAL_ERROR
    );
}

#[test]
fn config_is_validated() {
    let invalid = [
        ("ftp://127.0.0.1:4433", Duration::from_secs(1)),
        ("https://127.0.0.1:4433", Duration::from_secs(1)),
        ("http://127.0.0.1:4433/.ory/kratos", Duration::from_secs(1)),
        ("http://127.0.0.1:4433", Duration::ZERO),
    ];
    for (url, timeout) in invalid {
        let result = KratosClient::new(KratosConfig {
            public_url: url.parse().unwrap(),
            request_timeout: timeout,
        });
        assert!(result.is_err(), "{url} {timeout:?}");
    }
}
