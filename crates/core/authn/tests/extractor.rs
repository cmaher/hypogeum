use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use http_body_util::BodyExt;
use hypgm_authn::{OptionalAuth, Principal, RequireAuth};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

fn app() -> Router {
    Router::new()
        .route(
            "/protected",
            get(|RequireAuth(p): RequireAuth| async move { p.user_id.to_string() }),
        )
        .route(
            "/optional",
            get(|OptionalAuth(p): OptionalAuth| async move {
                p.map_or("none".to_owned(), |p| p.user_id.to_string())
            }),
        )
}

async fn get_body(path: &str, principal: Option<Principal>) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
    }
    let response = app().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn extractors_read_the_principal_from_request_extensions() {
    let principal = Principal::new(Uuid::new_v4(), Uuid::new_v4());
    let expected = principal.user_id.to_string().into_bytes();
    assert_eq!(
        get_body("/protected", Some(principal)).await,
        (StatusCode::OK, expected.clone())
    );
    assert_eq!(
        get_body("/optional", Some(principal)).await,
        (StatusCode::OK, expected)
    );
    assert_eq!(
        get_body("/optional", None).await,
        (StatusCode::OK, b"none".to_vec())
    );
}

#[tokio::test]
async fn missing_principal_is_a_json_401_and_headers_are_ignored() {
    let request = Request::builder()
        .uri("/protected")
        .header("authorization", "Bearer anything")
        .header("cookie", "ory_kratos_session=anything")
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], "unauthenticated");
    assert_eq!(json["message"], "Authentication required");
}
