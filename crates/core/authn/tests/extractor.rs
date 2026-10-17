use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use http_body_util::BodyExt;
use hypgm_authn::{
    AuthMethod, Claims, JwtValidator, OptionalAuth, Principal, PrincipalAuthBackend, RequireAuth,
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const TEST_SECRET: &[u8] = b"super-secret-test-key-32-bytes-long!";

fn create_token(user_id: Uuid, auth_id: Uuid) -> String {
    let exp = chrono::Utc::now().timestamp() as usize + 3600;
    let claims = Claims::new(auth_id, user_id, exp);
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_SECRET),
    )
    .expect("token encoding")
}

#[tokio::test]
async fn test_require_auth_valid_bearer_header() {
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let token = create_token(user_id, auth_id);

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let mut req = Request::builder()
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(())
        .unwrap();
    req.extensions_mut().insert(validator);

    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_ok());
    let principal = result.unwrap().into_inner();
    assert_eq!(principal.user_id, user_id);
    assert_eq!(principal.auth_id, auth_id);
    assert_eq!(principal.method, AuthMethod::BearerJwt);

    // Verify principal is now in parts.extensions
    assert_eq!(parts.extensions.get::<Principal>(), Some(&principal));
}

#[tokio::test]
async fn test_require_auth_case_insensitive_bearer_header() {
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let token = create_token(user_id, auth_id);

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let mut req = Request::builder()
        .header(header::AUTHORIZATION, format!("BEARER {token}"))
        .body(())
        .unwrap();
    req.extensions_mut().insert(validator);

    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_ok());
    let principal = result.unwrap().into_inner();
    assert_eq!(principal.user_id, user_id);
    assert_eq!(principal.auth_id, auth_id);
}

#[tokio::test]
async fn test_require_auth_active_session() {
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let session_principal = Principal::new(auth_id, user_id, AuthMethod::Session);

    let mut req = Request::builder().body(()).unwrap();
    req.extensions_mut().insert(session_principal.clone());

    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_ok());
    let principal = result.unwrap().into_inner();
    assert_eq!(principal, session_principal);
    assert_eq!(principal.method, AuthMethod::Session);
}

#[tokio::test]
async fn test_require_auth_missing_credentials_returns_401_json() {
    let req = Request::builder().body(()).unwrap();
    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_err());
    let rejection = result.unwrap_err();
    let response = rejection.into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body_bytes).expect("valid json response");

    assert_eq!(json["code"], "unauthenticated");
    assert_eq!(json["message"], "Authentication required");
}

#[tokio::test]
async fn test_require_auth_invalid_token_returns_401_json() {
    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let mut req = Request::builder()
        .header(header::AUTHORIZATION, "Bearer invalid-garbage-token")
        .body(())
        .unwrap();
    req.extensions_mut().insert(validator);

    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_err());
    let rejection = result.unwrap_err();
    let response = rejection.into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body_bytes).expect("valid json response");

    assert_eq!(json["code"], "unauthenticated");
    assert_eq!(json["message"], "Invalid authentication credentials");
}

#[tokio::test]
async fn test_require_auth_missing_validator_returns_401_not_500() {
    let req = Request::builder()
        .header(header::AUTHORIZATION, "Bearer some-token")
        .body(())
        .unwrap();

    let (mut parts, ()) = req.into_parts();
    let result = RequireAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_err());
    let rejection = result.unwrap_err();
    let response = rejection.into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body_bytes).expect("valid json response");
    assert_eq!(json["code"], "unauthenticated");
    assert_eq!(json["message"], "Invalid authentication credentials");
}

#[tokio::test]
async fn test_optional_auth_anonymous_yields_none() {
    let req = Request::builder().body(()).unwrap();
    let (mut parts, ()) = req.into_parts();

    let result = OptionalAuth::from_request_parts(&mut parts, &()).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().into_inner(), None);
}

#[tokio::test]
async fn test_optional_auth_authenticated_yields_some() {
    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let token = create_token(user_id, auth_id);

    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);
    let mut req = Request::builder()
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(())
        .unwrap();
    req.extensions_mut().insert(validator);

    let (mut parts, ()) = req.into_parts();
    let result = OptionalAuth::from_request_parts(&mut parts, &()).await;

    assert!(result.is_ok());
    let principal = result.unwrap().into_inner().expect("principal present");
    assert_eq!(principal.user_id, user_id);
    assert_eq!(principal.auth_id, auth_id);

    // Verify principal is now in parts.extensions
    assert_eq!(parts.extensions.get::<Principal>(), Some(&principal));
}

#[tokio::test]
async fn test_axum_router_integration() {
    let validator = JwtValidator::from_hs256_secret(TEST_SECRET);

    let app = Router::new()
        .route(
            "/protected",
            get(|RequireAuth(principal): RequireAuth| async move {
                format!("authenticated: {}", principal.user_id)
            }),
        )
        .route(
            "/optional",
            get(|OptionalAuth(principal): OptionalAuth| async move {
                match principal {
                    Some(p) => format!("some: {}", p.user_id),
                    None => "none".to_string(),
                }
            }),
        )
        .layer(Extension(validator));

    let user_id = Uuid::new_v4();
    let auth_id = Uuid::new_v4();
    let token = create_token(user_id, auth_id);

    // 1. Protected route with valid token -> 200 OK
    let req = Request::builder()
        .uri("/protected")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], format!("authenticated: {user_id}").as_bytes());

    // 2. Protected route without credentials -> 401 Unauthorized
    let req = Request::builder()
        .uri("/protected")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "unauthenticated");

    // 3. Optional route without credentials -> 200 OK "none"
    let req = Request::builder()
        .uri("/optional")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"none");

    // 4. Optional route with valid credentials -> 200 OK "some: <user_id>"
    let req = Request::builder()
        .uri("/optional")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], format!("some: {user_id}").as_bytes());
}

#[tokio::test]
async fn test_require_auth_with_axum_login_auth_session() {
    struct MockBackend(Principal);

    #[async_trait::async_trait]
    impl hypgm_authn::AuthBackend for MockBackend {
        async fn resolve_principal(
            &self,
            auth_id: Uuid,
        ) -> Result<Option<Principal>, hypgm_authn::AuthError> {
            if auth_id == self.0.auth_id {
                Ok(Some(self.0.clone()))
            } else {
                Ok(None)
            }
        }
    }

    let auth_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let principal = Principal::new(auth_id, user_id, AuthMethod::Session);

    let backend = MockBackend(principal.clone());
    let login_backend = PrincipalAuthBackend::new(backend);
    let session_store = tower_sessions::MemoryStore::default();
    let session_layer = tower_sessions::SessionManagerLayer::new(session_store);
    let auth_layer = axum_login::AuthManagerLayerBuilder::new(login_backend, session_layer).build();

    let app = Router::new()
        .route(
            "/login-check",
            get(
                |RequireAuth(p): RequireAuth| async move { format!("session user: {}", p.user_id) },
            ),
        )
        .route(
            "/do-login",
            get(
                move |mut auth_session: axum_login::AuthSession<PrincipalAuthBackend>| async move {
                    let p = Principal::new(auth_id, user_id, AuthMethod::Session);
                    auth_session.login(&p).await.unwrap();
                    "logged in"
                },
            ),
        )
        .layer(auth_layer);

    // Initial check without session -> 401
    let req = Request::builder()
        .uri("/login-check")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Log in to get session cookie
    let req = Request::builder()
        .uri("/do-login")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Check with session cookie -> 200 OK with extracted user!
    let req = Request::builder()
        .uri("/login-check")
        .header(axum::http::header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], format!("session user: {user_id}").as_bytes());
}
