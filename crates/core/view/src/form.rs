use crate::HtmlError;
use axum::extract::{FromRequest, Request};
use axum::http::request::Parts;
use axum::http::{HeaderName, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use hypgm_errors::PublicErrorInfo;
use hypgm_http::{HttpError, MAX_BODY_BYTES, is_length_limit_error};
use serde::de::DeserializeOwned;
use std::ops::{Deref, DerefMut};

pub static SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Classification for form submissions a browser reports as cross-site.
pub const CROSS_SITE_FORM: PublicErrorInfo = PublicErrorInfo::new(
    StatusCode::FORBIDDEN,
    "forbidden",
    "Cross-site form submissions are not allowed",
);

/// Bounded `application/x-www-form-urlencoded` extractor for HTML forms.
///
/// URL-encoded forms are a CORS-simple request, so browsers send them
/// cross-site without a preflight. Before reading the body, submissions are
/// rejected with an HTML 403 when `Sec-Fetch-Site` is anything other than
/// `same-origin` or `none`, or, without `Sec-Fetch-Site`, when `Origin` does not
/// match the request authority. Requests carrying neither header (non-browser
/// clients) are accepted. Page routers should also install
/// [`cross_site_middleware`] so handlers without a form body get the same
/// check. This complements the `SameSite=Strict` session cookie default; it is
/// not a substitute for authorization.
///
/// Bodies are limited to [`MAX_BODY_BYTES`] for both declared and streamed
/// lengths. Other rejections are safe HTML errors: 415 for other media types,
/// 413 for oversized bodies and 400 for malformed bodies. Domain validation
/// stays in services.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppForm<T>(pub T);

impl<T> Deref for AppForm<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for AppForm<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub fn is_form_content_type(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .map(str::trim)
        .is_some_and(|media_type| {
            media_type.eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

/// Whether browser-supplied fetch metadata or `Origin` marks the request as
/// coming from another site.
pub fn is_cross_site_request(parts: &Parts) -> bool {
    if let Some(site) = parts.headers.get(&SEC_FETCH_SITE) {
        return !matches!(site.as_bytes(), b"same-origin" | b"none");
    }
    let Some(origin) = parts.headers.get(header::ORIGIN) else {
        return false;
    };
    let authority = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.authority().map(|authority| authority.as_str()));
    let origin_authority = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, authority)| authority);
    match (origin_authority, authority) {
        (Some(origin), Some(authority)) => !origin.eq_ignore_ascii_case(authority),
        _ => true,
    }
}

/// Rejects cross-site requests with unsafe methods before any handler runs,
/// so page routes are protected whether or not they extract [`AppForm`].
/// Safe methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`) pass through. Install on
/// page routers with `axum::middleware::from_fn(cross_site_middleware)`.
pub async fn cross_site_middleware(req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    if !parts.method.is_safe() && is_cross_site_request(&parts) {
        return HtmlError::Public(CROSS_SITE_FORM).into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

fn reject(err: HttpError) -> HtmlError {
    HtmlError::from_http(err)
}

impl<T, S> FromRequest<S> for AppForm<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HtmlError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();

        if is_cross_site_request(&parts) {
            return Err(HtmlError::Public(CROSS_SITE_FORM));
        }

        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| reject(HttpError::unsupported_media_type()))?;
        if !is_form_content_type(content_type) {
            return Err(reject(HttpError::unsupported_media_type()));
        }

        let content_length_exceeded = parts
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<usize>().ok())
            .is_some_and(|len| len > MAX_BODY_BYTES);
        if content_length_exceeded {
            return Err(reject(HttpError::payload_too_large()));
        }

        let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
            .await
            .map_err(|err| {
                if is_length_limit_error(&err) {
                    reject(HttpError::payload_too_large())
                } else {
                    reject(HttpError::invalid_request())
                }
            })?;

        let value = serde_urlencoded::from_bytes::<T>(&bytes)
            .map_err(|_| reject(HttpError::invalid_request()))?;
        Ok(AppForm(value))
    }
}
