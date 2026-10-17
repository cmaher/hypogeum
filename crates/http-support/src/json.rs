use crate::error::HttpError;
use axum::extract::{FromRequest, Request};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
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
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "application/json"
        || (mime.starts_with("application/") && mime.ends_with("+json"))
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

        if let Some(cl) = parts.headers.get(header::CONTENT_LENGTH) {
            if let Ok(cl_str) = cl.to_str() {
                if let Ok(len) = cl_str.trim().parse::<usize>() {
                    if len > MAX_BODY_BYTES {
                        return Err(HttpError::payload_too_large());
                    }
                }
            }
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

        let value = serde_json::from_slice::<T>(&bytes).map_err(|_| HttpError::invalid_request())?;

        Ok(AppJson(value))
    }
}
