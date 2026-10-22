use crate::HtmlError;
use axum::extract::{FromRequest, Request};
use axum::http::header;
use hypgm_http::{HttpError, MAX_BODY_BYTES};
use serde::de::DeserializeOwned;
use std::ops::{Deref, DerefMut};

/// Bounded `application/x-www-form-urlencoded` extractor for HTML forms.
///
/// Bodies are limited to [`MAX_BODY_BYTES`] for both declared and streamed
/// lengths. Rejections are safe HTML errors: 415 for other media types, 413
/// for oversized bodies and 400 for malformed bodies. Domain validation stays
/// in services.
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

fn is_length_limit_error(err: &axum::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if e.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = e.source();
    }
    false
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
