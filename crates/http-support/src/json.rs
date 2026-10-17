use crate::error::HttpError;
use axum::extract::{FromRequest, Request};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::ops::{Deref, DerefMut};

pub const MAX_BODY_BYTES: usize = 16_384;

#[derive(Debug, Clone, Copy, Default)]
pub struct AppJson<T>(pub T);

impl<T> Deref for AppJson<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for AppJson<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Serialize> IntoResponse for AppJson<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

pub fn is_json_content_type(content_type: &str) -> bool {
    let media_type = match content_type.split(';').next() {
        Some(m) => m.trim(),
        None => return false,
    };

    if media_type.is_empty() {
        return false;
    }

    let mut parts = media_type.split('/');
    let top_type = match parts.next() {
        Some(t) if !t.is_empty() => t,
        _ => return false,
    };
    let sub_type = match parts.next() {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    if parts.next().is_some() {
        return false;
    }

    if !top_type.eq_ignore_ascii_case("application") {
        return false;
    }

    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '!' | '#'
                    | '$'
                    | '%'
                    | '&'
                    | '\''
                    | '*'
                    | '+'
                    | '-'
                    | '.'
                    | '^'
                    | '_'
                    | '`'
                    | '|'
                    | '~'
            )
    }

    if !sub_type.chars().all(is_token_char) {
        return false;
    }

    if sub_type.eq_ignore_ascii_case("json") {
        return true;
    }

    if sub_type.len() > 5 {
        let (prefix, suffix) = sub_type.split_at(sub_type.len() - 5);
        if suffix.eq_ignore_ascii_case("+json") && !prefix.is_empty() {
            return true;
        }
    }

    false
}

fn is_length_limit_error(err: &axum::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if e.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        if e.to_string().to_lowercase().contains("limit") {
            return true;
        }
        source = e.source();
    }
    false
}

impl<T, S> FromRequest<S> for AppJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();

        let content_type = parts
            .headers
            .get(header::CONTENT_TYPE)
            .ok_or_else(HttpError::unsupported_media_type)?
            .to_str()
            .map_err(|_| HttpError::unsupported_media_type())?;

        if !is_json_content_type(content_type) {
            return Err(HttpError::unsupported_media_type());
        }

        let content_length_exceeded = parts
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|cl| cl.to_str().ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
            .is_some_and(|len| len > MAX_BODY_BYTES);
        if content_length_exceeded {
            return Err(HttpError::payload_too_large());
        }

        let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(err) => {
                if is_length_limit_error(&err) {
                    return Err(HttpError::payload_too_large());
                } else {
                    return Err(HttpError::invalid_request());
                }
            }
        };

        if bytes.is_empty() {
            return Err(HttpError::invalid_request());
        }

        let value =
            serde_json::from_slice::<T>(&bytes).map_err(|_| HttpError::invalid_request())?;

        Ok(AppJson(value))
    }
}
