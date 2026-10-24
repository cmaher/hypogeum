//! Opt-in server-rendered HTML and HTMX support.
//!
//! Domains own their routes and templates; this crate supplies request mode
//! detection, HTML response helpers and safe HTML error rendering. JSON
//! rendering defaults in `hypgm-http` are unchanged.

pub mod assets;
pub mod form;

pub use assets::{HTMX_SCRIPT_URL, HTMX_VERSION, VIEW_SCRIPT_URL};
pub use form::{
    AppForm, CROSS_SITE_FORM, cross_site_middleware, is_cross_site_request, is_form_content_type,
};

use askama::Template;
use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hypgm_errors::{PublicError, PublicErrorInfo};
use hypgm_http::{ErrorClassification, ErrorCode, HttpError};
use std::convert::Infallible;

pub static HX_REQUEST: HeaderName = HeaderName::from_static("hx-request");
pub static HX_HISTORY_RESTORE_REQUEST: HeaderName =
    HeaderName::from_static("hx-history-restore-request");

pub const HTML_CONTENT_TYPE: &str = "text/html; charset=utf-8";

/// Whether a handler should render a full document or an HTMX fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    FullPage,
    Fragment,
}

/// Request mode extracted from HTMX headers.
///
/// Only an exact `HX-Request: true` selects a fragment. History restoration
/// requests always select full pages because the browser replaces the whole
/// document with the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtmxRequest {
    pub mode: ViewMode,
}

impl HtmxRequest {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let mode = if header_is_true(headers, &HX_REQUEST)
            && !header_is_true(headers, &HX_HISTORY_RESTORE_REQUEST)
        {
            ViewMode::Fragment
        } else {
            ViewMode::FullPage
        };
        Self { mode }
    }

    pub fn is_fragment(&self) -> bool {
        self.mode == ViewMode::Fragment
    }
}

fn header_is_true(headers: &HeaderMap, name: &HeaderName) -> bool {
    headers
        .get(name)
        .is_some_and(|value| value.as_bytes() == b"true")
}

impl<S: Send + Sync> FromRequestParts<S> for HtmxRequest {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_headers(&parts.headers))
    }
}

fn vary_covers(headers: &HeaderMap, name: &HeaderName) -> bool {
    headers.get_all(header::VARY).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|token| token == "*" || token.eq_ignore_ascii_case(name.as_str()))
        })
    })
}

/// Appends `Vary` entries for every header [`HtmxRequest`] reads
/// (`HX-Request` and `HX-History-Restore-Request`), skipping any that an
/// existing `Vary` value already covers.
pub fn append_vary_htmx(headers: &mut HeaderMap) {
    for (name, value) in [
        (&HX_REQUEST, "HX-Request"),
        (&HX_HISTORY_RESTORE_REQUEST, "HX-History-Restore-Request"),
    ] {
        if !vary_covers(headers, name) {
            headers.append(header::VARY, HeaderValue::from_static(value));
        }
    }
}

/// Sets the HTML content type, merges the HTMX `Vary` entries, and adds
/// `X-Content-Type-Options: nosniff` and `X-Frame-Options: DENY` unless the
/// response already sets them.
pub fn with_html_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(HTML_CONTENT_TYPE),
    );
    append_vary_htmx(headers);
    headers
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry(header::X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    response
}

pub fn html_response(status: StatusCode, body: String) -> Response {
    with_html_headers((status, body).into_response())
}

/// Renders an Askama template, falling back to a static HTML 500 on failure.
///
/// Use only for non-error statuses. Error responses must go through
/// [`render_error_template`]: without a classification, routing middleware
/// ([`html_error_middleware`] and `hypgm_http::trace_layer`) replaces 404 and
/// 405 bodies with generic error pages. Debug builds assert this.
pub fn render_template<T: Template>(status: StatusCode, template: &T) -> Response {
    debug_assert!(
        !status.is_client_error() && !status.is_server_error(),
        "render_template called with error status {status}; use render_error_template"
    );
    match template.render() {
        Ok(body) => html_response(status, body),
        Err(err) => render_failure_response(template, &err),
    }
}

/// Renders a domain error template with the classification status and
/// tracing extensions of `classification`.
pub fn render_error_template<T: Template>(
    classification: PublicErrorInfo,
    template: &T,
) -> Response {
    match template.render() {
        Ok(body) => with_classification(html_response(classification.status, body), classification),
        Err(err) => render_failure_response(template, &err),
    }
}

/// Minimal safe HTML 500 for a failed template render. Logs the cause and the
/// template type server-side; neither reaches the response. It never renders
/// a template, so it cannot fail recursively.
///
/// [`render_template`] and [`render_error_template`] call this; use it
/// directly when a handler renders a template itself, for example to
/// post-process the body.
pub fn render_failure_response<T: Template + ?Sized>(
    template: &T,
    err: &askama::Error,
) -> Response {
    tracing::error!(
        error = %err,
        template = std::any::type_name_of_val(template),
        "HTML template rendering failed"
    );
    html_error_response(PublicErrorInfo::INTERNAL_ERROR)
}

/// Attaches `classification` and its status and code to a response's
/// extensions, as JSON errors do, so tracing records the error and routing
/// middleware leaves the body in place. Use it for error responses built from
/// bodies a handler rendered itself.
pub fn with_classification(mut response: Response, classification: PublicErrorInfo) -> Response {
    response.extensions_mut().insert(classification);
    response.extensions_mut().insert(classification.status);
    response
        .extensions_mut()
        .insert(ErrorCode(classification.code));
    response
}

/// Escapes text for HTML element content and quoted attribute values.
pub fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Renders a public classification as a minimal, template-free HTML document.
///
/// The message is wrapped in a `data-view-error` element so the shared HTMX
/// configuration can surface it without replacing a domain form section.
pub fn html_error_response(classification: PublicErrorInfo) -> Response {
    let message = escape_html(classification.message);
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{message}</title>\n</head>\n<body>\n<main>\n<h1>{message}</h1>\n\
         <p data-view-error role=\"alert\">{message}</p>\n</main>\n</body>\n</html>\n"
    );
    with_classification(html_response(classification.status, body), classification)
}

pub fn render_html_public_error<E: PublicError + ?Sized>(err: &E) -> Response {
    html_error_response(err.public_error())
}

pub fn html_method_not_allowed(allow: impl AsRef<str>) -> Response {
    let mut response = html_error_response(PublicErrorInfo::METHOD_NOT_ALLOWED);
    let allow = allow.as_ref();
    if !allow.is_empty()
        && let Ok(value) = HeaderValue::from_str(allow)
    {
        response.headers_mut().insert(header::ALLOW, value);
    }
    response
}

pub fn html_not_found() -> Response {
    html_error_response(PublicErrorInfo::NOT_FOUND)
}

pub async fn html_not_found_handler() -> Response {
    html_not_found()
}

/// Converts unclassified routing 404 and 405 responses into HTML errors,
/// preserving `Allow`. Install on page routers with
/// `axum::middleware::from_fn(html_error_middleware)`.
pub async fn html_error_middleware(req: Request, next: Next) -> Response {
    html_routing_errors(next.run(req).await)
}

pub fn html_routing_errors(response: Response) -> Response {
    if response.extensions().get::<ErrorClassification>().is_some() {
        return response;
    }
    match response.status() {
        StatusCode::METHOD_NOT_ALLOWED => {
            let allow = response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_owned();
            html_method_not_allowed(allow)
        }
        StatusCode::NOT_FOUND => html_not_found(),
        _ => response,
    }
}

/// Safe HTML counterpart to [`HttpError`].
#[derive(Debug)]
pub enum HtmlError {
    Public(PublicErrorInfo),
    MethodNotAllowed { allow: String },
}

impl HtmlError {
    pub fn classification(&self) -> PublicErrorInfo {
        match self {
            Self::Public(classification) => *classification,
            Self::MethodNotAllowed { .. } => PublicErrorInfo::METHOD_NOT_ALLOWED,
        }
    }

    pub fn from_http(err: HttpError) -> Self {
        match err {
            HttpError::Public(classification) => Self::Public(classification),
            HttpError::MethodNotAllowed { allow } => Self::MethodNotAllowed { allow },
        }
    }
}

impl<E: PublicError> From<E> for HtmlError {
    fn from(err: E) -> Self {
        Self::Public(err.public_error())
    }
}

impl std::fmt::Display for HtmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let classification = self.classification();
        write!(f, "{}: {}", classification.code, classification.message)
    }
}

impl std::error::Error for HtmlError {}

impl IntoResponse for HtmlError {
    fn into_response(self) -> Response {
        match self {
            Self::Public(classification) => html_error_response(classification),
            Self::MethodNotAllowed { allow } => html_method_not_allowed(allow),
        }
    }
}
