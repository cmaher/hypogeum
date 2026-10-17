use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use tracing::Instrument;

pub static REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn resolve_matched_path(req: &Request) -> &'static str {
    if let Some(matched) = req.extensions().get::<MatchedPath>() {
        match matched.as_str() {
            "/healthz" => "/healthz",
            "/api/hello" => "/api/hello",
            _ => "unmatched",
        }
    } else {
        match req.uri().path() {
            "/healthz" => "/healthz",
            "/api/hello" => "/api/hello",
            _ => "unmatched",
        }
    }
}

pub async fn trace_middleware(mut req: Request, next: Next) -> Response {
    let start = Instant::now();

    let request_id = uuid::Uuid::new_v4().to_string();
    let header_val = HeaderValue::from_str(&request_id).expect("valid request id header value");

    req.headers_mut()
        .insert(REQUEST_ID_HEADER.clone(), header_val.clone());
    req.extensions_mut()
        .insert(RequestId(request_id.clone()));

    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let matched_path = resolve_matched_path(&req);

    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %method,
        matched_path = %matched_path,
        status = tracing::field::Empty,
        elapsed_ms = tracing::field::Empty,
        error_code = tracing::field::Empty,
    );

    let mut response = next.run(req).instrument(span.clone()).await;

    if response.status() == StatusCode::METHOD_NOT_ALLOWED
        && response.extensions().get::<crate::error::ErrorClassification>().is_none()
    {
        let allow = response
            .headers()
            .get(axum::http::header::ALLOW)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        response = crate::error::render_method_not_allowed(allow);
    } else if response.status() == StatusCode::NOT_FOUND
        && response.extensions().get::<crate::error::ErrorClassification>().is_none()
    {
        response = crate::error::not_found_response();
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let status = response.status();
    let status_code = status.as_u16();

    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER.clone(), header_val);
    response
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let error_code = response
        .extensions()
        .get::<crate::error::ErrorClassification>()
        .map(|c| c.code)
        .or_else(|| {
            if status == StatusCode::NOT_FOUND {
                Some("not_found")
            } else if status == StatusCode::METHOD_NOT_ALLOWED {
                Some("method_not_allowed")
            } else if status.is_server_error() {
                Some("internal_error")
            } else {
                None
            }
        });

    span.record("status", status_code);
    span.record("elapsed_ms", elapsed_ms);
    if let Some(code) = error_code {
        span.record("error_code", code);
    }

    let _guard = span.enter();
    let is_health_success = (matched_path == "/healthz" || path == "/healthz") && status.is_success();

    if is_health_success {
        tracing::debug!(
            request_id = %request_id,
            method = %method,
            matched_path = %matched_path,
            status = status_code,
            elapsed_ms = elapsed_ms,
            "request completed"
        );
    } else if status.is_server_error() {
        if let Some(code) = error_code {
            tracing::error!(
                request_id = %request_id,
                method = %method,
                matched_path = %matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                error_code = code,
                "request completed"
            );
        } else {
            tracing::error!(
                request_id = %request_id,
                method = %method,
                matched_path = %matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                "request completed"
            );
        }
    } else {
        if let Some(code) = error_code {
            tracing::info!(
                request_id = %request_id,
                method = %method,
                matched_path = %matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                error_code = code,
                "request completed"
            );
        } else {
            tracing::info!(
                request_id = %request_id,
                method = %method,
                matched_path = %matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                "request completed"
            );
        }
    }

    response
}

pub fn trace_layer<S>() -> impl tower::Layer<S> + Clone {
    axum::middleware::from_fn::<_, (Request, Next)>(trace_middleware)
}
