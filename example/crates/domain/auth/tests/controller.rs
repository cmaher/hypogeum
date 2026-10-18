use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use exemplum_dmn_auth::api::{LoginRequest, LoginResponse, TokenResponse};
use exemplum_dmn_auth::backend::ExemplumAuthBackend;
use exemplum_dmn_auth::controller::{AuthComponentsFactory, TokenConfig, router};
use exemplum_dmn_auth::repo::{AuthRepo, InMemoryAuthRepo};
use http_body_util::BodyExt;
use hypgm_authn::api_key::{GeneratedApiKey, generate_api_key};
use hypgm_authn::password::hash_password;
use hypgm_authn::session::PrincipalAuthBackend;
use std::sync::Arc;
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use uuid::Uuid;

fn build_test_app(repo: Arc<InMemoryAuthRepo>) -> axum::Router {
    let components = AuthComponentsFactory {
        repo: repo.clone(),
        password_config: Default::default(),
        token_config: TokenConfig {
            jwt_secret: b"test-secret-key-32bytes-for-jwt!".to_vec(),
            default_ttl: 3600,
            max_ttl: 7200,
            api_key_prefix: "exmpl_key".to_string(),
        },
    }
    .build();
    let routes = router(components);

    let backend = ExemplumAuthBackend::from_arc(repo);
    let login_backend = PrincipalAuthBackend::new(backend);
    let session_store = tower_sessions::MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();

    axum::Router::new().merge(routes).layer(auth_layer)
}

#[tokio::test]
async fn test_login_success_and_failure() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let pass_hash = hash_password("ValidPassword123!").unwrap();

    repo.create(
        auth_id,
        user_id,
        "password",
        "alice",
        &pass_hash,
        serde_json::json!({}),
    )
    .await
    .unwrap();

    let app = build_test_app(repo);

    // Wrong password -> 401
    let bad_req = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&LoginRequest {
                username: "alice".into(),
                password: "WrongPassword".into(),
            })
            .unwrap(),
        ))
        .unwrap();

    let resp = app.clone().oneshot(bad_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct password -> 200 and Set-Cookie
    let good_req = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&LoginRequest {
                username: "alice".into(),
                password: "ValidPassword123!".into(),
            })
            .unwrap(),
        ))
        .unwrap();

    let resp = app.oneshot(good_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().contains_key(header::SET_COOKIE));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let login_resp: LoginResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(login_resp.auth_id, auth_id);
    assert_eq!(login_resp.user_id, user_id);
}

#[tokio::test]
async fn test_token_exchange_standalone_api_key_and_clamped_ttl() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let GeneratedApiKey {
        raw_key,
        secret_hash: key_hash,
    } = generate_api_key("exmpl_key");

    // Standalone key stores key_hash in identifier, NEVER the raw key
    repo.create(
        auth_id,
        user_id,
        "api_key",
        &key_hash,
        &key_hash,
        serde_json::json!({}),
    )
    .await
    .unwrap();

    let app = build_test_app(repo);

    // Token exchange with custom requested TTL exceeding max_ttl (7200) -> clamped to 7200
    let payload = format!(r#"   {{"api_key": "{raw_key}", "expires_in": 99999}}  "#);
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(payload))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(resp.headers()[header::PRAGMA], "no-cache");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let token_resp: TokenResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(token_resp.expires_in, 7200); // clamped to max_token_ttl
    assert_eq!(token_resp.token_type, "Bearer");

    // Malformed JSON returns 400 Bad Request
    let bad_json = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"api_key": "unclosed_json"#))
        .unwrap();

    let resp = app.clone().oneshot(bad_json).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Invalid key prefix returns 401
    let bad_prefix = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"api_key": "wrongprefix_12345"}"#))
        .unwrap();

    let resp = app.oneshot(bad_prefix).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_token_exchange_http_basic_auth() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let client_id = "client_app_123";
    let client_secret = "super_secret_client_pass";
    let secret_hash = hypgm_authn::api_key::hash_api_key(client_secret);

    repo.create(
        auth_id,
        user_id,
        "api_key",
        client_id,
        &secret_hash,
        serde_json::json!({}),
    )
    .await
    .unwrap();

    let app = build_test_app(repo);

    let basic_creds = format!("{client_id}:{client_secret}");
    let basic_header = format!("Basic {}", BASE64_STANDARD.encode(basic_creds));

    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::AUTHORIZATION, basic_header)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=client_credentials"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let token_resp: TokenResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(token_resp.token_type, "Bearer");
    assert_eq!(token_resp.expires_in, 3600); // default TTL
}

#[tokio::test]
async fn test_token_exchange_validates_media_type_and_grant_type() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let app = build_test_app(repo);

    // Unsupported media type
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("api_key=exmpl_key_123"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Unsupported grant type
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"grant_type": "authorization_code"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Conflicting credentials across Basic Auth and request body
    let basic_header = format!("Basic {}", BASE64_STANDARD.encode("client:secret"));
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::AUTHORIZATION, basic_header)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"client_secret": "conflict"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_token_exchange_http_basic_url_encoded_credentials() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let client_id = "client app";
    let client_secret = "super secret/pass";
    let secret_hash = hypgm_authn::api_key::hash_api_key(client_secret);

    repo.create(
        auth_id,
        user_id,
        "api_key",
        client_id,
        &secret_hash,
        serde_json::json!({}),
    )
    .await
    .unwrap();

    let app = build_test_app(repo);

    // URL-encoded in Basic credentials parameter per RFC 6749 §2.3.1
    let encoded_id = urlencoding::encode(client_id);
    let encoded_secret = urlencoding::encode(client_secret);
    let basic_creds = format!("{encoded_id}:{encoded_secret}");
    let basic_header = format!("Basic {}", BASE64_STANDARD.encode(basic_creds));

    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::AUTHORIZATION, basic_header)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=client_credentials"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn token_configuration_debug_redacts_the_signing_key() {
    let secret = b"sensitive-signing-key-never-log-this".to_vec();
    let config = TokenConfig::new(secret.clone());
    let debug = format!("{config:?}");
    assert!(!debug.contains(&format!("{secret:?}")));
    assert!(debug.contains("[redacted]"));
}

#[test]
fn default_token_configurations_do_not_share_a_signing_key() {
    let first = TokenConfig::default();
    let second = TokenConfig::default();
    assert_ne!(first.jwt_secret, second.jwt_secret);
}

#[tokio::test]
async fn basic_credentials_decode_form_spaces_and_literal_plus() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    repo.create(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "api_key",
        "client app",
        &hypgm_authn::api_key::hash_api_key("secret pass+suffix"),
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let app = build_test_app(repo);
    let req = Request::builder()
        .method("POST")
        .uri("/token")
        .header(
            header::AUTHORIZATION,
            format!(
                "Basic {}",
                BASE64_STANDARD.encode("client+app:secret+pass%2Bsuffix")
            ),
        )
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=client_credentials"))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::PRAGMA], "no-cache");
}

#[tokio::test]
async fn token_errors_use_oauth_codes_and_basic_challenges() {
    let app = build_test_app(Arc::new(InMemoryAuthRepo::new()));
    for (payload, authorization, status, expected_error) in [
        (
            "grant_type=client_credentials",
            Some(format!("Basic {}", BASE64_STANDARD.encode("unknown:wrong"))),
            StatusCode::UNAUTHORIZED,
            "invalid_client",
        ),
        (
            "grant_type=authorization_code",
            None,
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
        ),
        (
            "expires_in=not-a-number",
            None,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
    ] {
        let mut req = Request::builder()
            .method("POST")
            .uri("/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(value) = authorization {
            req = req.header(header::AUTHORIZATION, value);
        }
        let response = app
            .clone()
            .oneshot(req.body(Body::from(payload)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::UNAUTHORIZED {
            assert!(
                response.headers()[header::WWW_AUTHENTICATE]
                    .to_str()
                    .unwrap()
                    .starts_with("Basic")
            );
        }
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], expected_error);
        assert!(!String::from_utf8_lossy(&bytes).contains("unknown:wrong"));
    }
}

#[tokio::test]
async fn password_verification_yields_to_the_async_runtime() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let repo = Arc::new(InMemoryAuthRepo::new());
    repo.create(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "password",
        "alice",
        &hash_password("secret").unwrap(),
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let app = build_test_app(repo);
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"alice","password":"secret"}"#))
        .unwrap();
    let mut login = std::pin::pin!(app.oneshot(req));
    assert!(matches!(
        login.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(login.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn token_body_limit_errors_keep_the_oauth_envelope() {
    let app = build_test_app(Arc::new(InMemoryAuthRepo::new()))
        .layer(axum::extract::DefaultBodyLimit::max(16));
    let request = Request::builder()
        .method("POST")
        .uri("/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"api_key":"an-oversized-credential"}"#))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "invalid_request");
}

#[tokio::test]
async fn unknown_username_also_performs_blocking_password_verification() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let app = build_test_app(Arc::new(InMemoryAuthRepo::new()));
    let request = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"missing","password":"wrong"}"#))
        .unwrap();
    let mut login = std::pin::pin!(app.oneshot(request));
    assert!(matches!(
        login.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(login.await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invalid_stored_password_parameters_return_redacted_internal_error() {
    let repo = Arc::new(InMemoryAuthRepo::new());
    let password = "private-password";
    let hash = hash_password(password).unwrap().replace("m=19456", "m=1");
    repo.create(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "password",
        "broken",
        &hash,
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let response = build_test_app(repo)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&LoginRequest {
                        username: "broken".into(),
                        password: password.into(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "internal_error");
    assert!(!String::from_utf8_lossy(&body).contains(password));
    assert!(!String::from_utf8_lossy(&body).contains(&hash));
}
