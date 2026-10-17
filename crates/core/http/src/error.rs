use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hypgm_errors::{PublicError, PublicErrorInfo};

pub type ErrorClassification = PublicErrorInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub &'static str);

#[derive(Debug)]
pub enum HttpError {
    Public(PublicErrorInfo),
    MethodNotAllowed { allow: String },
}

impl HttpError {
    pub fn classification(&self) -> PublicErrorInfo {
        match self {
            Self::Public(c) => *c,
            Self::MethodNotAllowed { .. } => PublicErrorInfo::METHOD_NOT_ALLOWED,
        }
    }

    pub fn invalid_request() -> Self {
        Self::Public(PublicErrorInfo::INVALID_REQUEST)
    }

    pub fn validation_error() -> Self {
        Self::Public(PublicErrorInfo::VALIDATION_ERROR)
    }

    pub fn not_found() -> Self {
        Self::Public(PublicErrorInfo::NOT_FOUND)
    }

    pub fn method_not_allowed(allow: impl Into<String>) -> Self {
        Self::MethodNotAllowed {
            allow: allow.into(),
        }
    }

    pub fn payload_too_large() -> Self {
        Self::Public(PublicErrorInfo::PAYLOAD_TOO_LARGE)
    }

    pub fn unsupported_media_type() -> Self {
        Self::Public(PublicErrorInfo::UNSUPPORTED_MEDIA_TYPE)
    }

    pub fn internal_error() -> Self {
        Self::Public(PublicErrorInfo::INTERNAL_ERROR)
    }

    pub fn custom(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self::Public(PublicErrorInfo::new(status, code, message))
    }

    pub fn with_classification(classification: PublicErrorInfo) -> Self {
        Self::Public(classification)
    }

    pub fn reclassify(self, classification: PublicErrorInfo) -> Self {
        Self::Public(classification)
    }
}

impl<E: PublicError> From<E> for HttpError {
    fn from(err: E) -> Self {
        Self::Public(err.public_error())
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.classification();
        write!(f, "{}: {}", c.code, c.message)
    }
}

impl std::error::Error for HttpError {}

pub fn render_error_response(classification: PublicErrorInfo) -> Response {
    let body = serde_json::json!({
        "code": classification.code,
        "message": classification.message,
    });
    let mut response = (classification.status, axum::Json(body)).into_response();
    response.extensions_mut().insert(classification);
    response.extensions_mut().insert(classification.status);
    response
        .extensions_mut()
        .insert(ErrorCode(classification.code));
    response
}

pub fn render_public_error<E: PublicError + ?Sized>(err: &E) -> Response {
    render_error_response(err.public_error())
}

pub fn render_method_not_allowed(allow: impl AsRef<str>) -> Response {
    let mut response = render_error_response(PublicErrorInfo::METHOD_NOT_ALLOWED);
    let s = allow.as_ref();
    if !s.is_empty()
        && let Ok(val) = HeaderValue::from_str(s)
    {
        response.headers_mut().insert(header::ALLOW, val);
    }
    response
}

pub fn not_found_response() -> Response {
    render_error_response(PublicErrorInfo::NOT_FOUND)
}

pub async fn not_found_handler() -> Response {
    not_found_response()
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        match self {
            Self::MethodNotAllowed { allow } => render_method_not_allowed(allow),
            Self::Public(info) => render_error_response(info),
        }
    }
}
