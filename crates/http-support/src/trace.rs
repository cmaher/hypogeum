use axum::extract::{MatchedPath, Request};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use tracing::Instrument;

pub static REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Marks a health-check response so successful completions are logged at DEBUG.
#[derive(Debug, Clone, Copy)]
pub struct HealthCheck;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn resolve_matched_path(req: &Request) -> &str {
    req.extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("unmatched")
}

struct RequestTraceMetadata {
    request_id: String,
    header_val: HeaderValue,
    method: String,
    matched_path: String,
    start: Instant,
}

fn prepare_request(req: &mut Request) -> (RequestTraceMetadata, tracing::Span) {
    let start = Instant::now();

    let request_id = uuid::Uuid::new_v4().to_string();
    let header_val = HeaderValue::from_str(&request_id).expect("valid request id header value");

    req.headers_mut()
        .insert(REQUEST_ID_HEADER.clone(), header_val.clone());
    req.extensions_mut().insert(RequestId(request_id.clone()));

    let method = req.method().to_string();
    let matched_path = resolve_matched_path(req).to_owned();

    let span = tracing::error_span!(
        "request",
        request_id = %request_id,
        method = %method,
        matched_path = %matched_path,
        status = tracing::field::Empty,
        elapsed_ms = tracing::field::Empty,
        error_code = tracing::field::Empty,
    );

    let meta = RequestTraceMetadata {
        request_id,
        header_val,
        method,
        matched_path,
        start,
    };

    (meta, span)
}

fn process_response(
    mut response: Response,
    meta: RequestTraceMetadata,
    span: &tracing::Span,
) -> Response {
    if response.status() == StatusCode::METHOD_NOT_ALLOWED
        && response
            .extensions()
            .get::<crate::error::ErrorClassification>()
            .is_none()
    {
        let allow = response
            .headers()
            .get(axum::http::header::ALLOW)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        response = crate::error::render_method_not_allowed(allow);
    } else if response.status() == StatusCode::NOT_FOUND
        && response
            .extensions()
            .get::<crate::error::ErrorClassification>()
            .is_none()
    {
        response = crate::error::not_found_response();
    }

    let elapsed_ms = meta.start.elapsed().as_millis() as u64;
    let status = response.status();
    let status_code = status.as_u16();

    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER.clone(), meta.header_val);
    response
        .extensions_mut()
        .insert(RequestId(meta.request_id.clone()));

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
    let is_health_success =
        response.extensions().get::<HealthCheck>().is_some() && status.is_success();

    if is_health_success {
        tracing::debug!(
            request_id = %meta.request_id,
            method = %meta.method,
            matched_path = %meta.matched_path,
            status = status_code,
            elapsed_ms = elapsed_ms,
            "request completed"
        );
    } else if status.is_server_error() {
        if let Some(code) = error_code {
            tracing::error!(
                request_id = %meta.request_id,
                method = %meta.method,
                matched_path = %meta.matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                error_code = code,
                "request completed"
            );
        } else {
            tracing::error!(
                request_id = %meta.request_id,
                method = %meta.method,
                matched_path = %meta.matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                "request completed"
            );
        }
    } else {
        if let Some(code) = error_code {
            tracing::info!(
                request_id = %meta.request_id,
                method = %meta.method,
                matched_path = %meta.matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                error_code = code,
                "request completed"
            );
        } else {
            tracing::info!(
                request_id = %meta.request_id,
                method = %meta.method,
                matched_path = %meta.matched_path,
                status = status_code,
                elapsed_ms = elapsed_ms,
                "request completed"
            );
        }
    }

    response
}

pub async fn trace_middleware(mut req: Request, next: Next) -> Response {
    let (meta, span) = prepare_request(&mut req);
    let response = next.run(req).instrument(span.clone()).await;
    process_response(response, meta, &span)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TraceLayer;

impl TraceLayer {
    pub fn new() -> Self {
        Self
    }
}

pub fn trace_layer() -> TraceLayer {
    TraceLayer::new()
}

impl<S> tower::Layer<S> for TraceLayer {
    type Service = TraceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceService { inner }
    }
}

#[derive(Clone, Debug)]
pub struct TraceService<S> {
    inner: S,
}

impl<S> tower::Service<Request> for TraceService<S>
where
    S: tower::Service<Request, Response = Response> + Clone + Send + 'static,
    S::Error: Into<std::convert::Infallible> + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let replacement = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, replacement);
        Box::pin(async move {
            let (meta, span) = prepare_request(&mut req);
            let response = inner.call(req).instrument(span.clone()).await?;
            Ok(process_response(response, meta, &span))
        })
    }
}
