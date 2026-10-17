use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use errors::{RepoError, ServiceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorClassification {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}

impl ErrorClassification {
    pub const INVALID_REQUEST: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_request",
        message: "Invalid request",
    };

    pub const VALIDATION_ERROR: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: "validation_error",
        message: "Validation failed",
    };

    pub const NOT_FOUND: Self = Self {
        status: StatusCode::NOT_FOUND,
        code: "not_found",
        message: "Resource not found",
    };

    pub const METHOD_NOT_ALLOWED: Self = Self {
        status: StatusCode::METHOD_NOT_ALLOWED,
        code: "method_not_allowed",
        message: "Method not allowed",
    };

    pub const PAYLOAD_TOO_LARGE: Self = Self {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        code: "payload_too_large",
        message: "Payload too large",
    };

    pub const UNSUPPORTED_MEDIA_TYPE: Self = Self {
        status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
        code: "unsupported_media_type",
        message: "Unsupported media type",
    };

    pub const INTERNAL_ERROR: Self = Self {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal_error",
        message: "Internal server error",
    };

    pub const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
}

#[derive(Debug)]
pub enum HttpError {
    Service(ServiceError),
    Repo(RepoError),
    Classification(ErrorClassification),
}

impl HttpError {
    pub fn classification(&self) -> ErrorClassification {
        match self {
            Self::Service(err) => match err {
                ServiceError::Validation(_) => ErrorClassification::VALIDATION_ERROR,
                ServiceError::Repo(repo) => repo_to_classification(repo),
                ServiceError::Remote(_) | ServiceError::InvalidResponse(_) => {
                    ErrorClassification::INTERNAL_ERROR
                }
            },
            Self::Repo(repo) => repo_to_classification(repo),
            Self::Classification(c) => *c,
        }
    }

    pub fn invalid_request() -> Self {
        Self::Classification(ErrorClassification::INVALID_REQUEST)
    }

    pub fn validation_error() -> Self {
        Self::Classification(ErrorClassification::VALIDATION_ERROR)
    }

    pub fn not_found() -> Self {
        Self::Classification(ErrorClassification::NOT_FOUND)
    }

    pub fn method_not_allowed() -> Self {
        Self::Classification(ErrorClassification::METHOD_NOT_ALLOWED)
    }

    pub fn payload_too_large() -> Self {
        Self::Classification(ErrorClassification::PAYLOAD_TOO_LARGE)
    }

    pub fn unsupported_media_type() -> Self {
        Self::Classification(ErrorClassification::UNSUPPORTED_MEDIA_TYPE)
    }

    pub fn internal_error() -> Self {
        Self::Classification(ErrorClassification::INTERNAL_ERROR)
    }

    pub fn custom(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self::Classification(ErrorClassification::new(status, code, message))
    }

    pub fn with_classification(classification: ErrorClassification) -> Self {
        Self::Classification(classification)
    }

    pub fn reclassify(self, classification: ErrorClassification) -> Self {
        Self::Classification(classification)
    }
}

fn repo_to_classification(err: &RepoError) -> ErrorClassification {
    match err {
        RepoError::InvalidInput(_) => ErrorClassification::VALIDATION_ERROR,
        RepoError::NotFound => ErrorClassification::NOT_FOUND,
        RepoError::Database(_) => ErrorClassification::INTERNAL_ERROR,
    }
}

impl From<ServiceError> for HttpError {
    fn from(err: ServiceError) -> Self {
        Self::Service(err)
    }
}

impl From<RepoError> for HttpError {
    fn from(err: RepoError) -> Self {
        Self::Repo(err)
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.classification();
        write!(f, "{}: {}", c.code, c.message)
    }
}

impl std::error::Error for HttpError {}

pub fn render_error_response(classification: ErrorClassification) -> Response {
    let body = serde_json::json!({
        "code": classification.code,
        "message": classification.message,
    });
    let mut response = (classification.status, axum::Json(body)).into_response();
    response.extensions_mut().insert(classification);
    response
}

pub fn render_method_not_allowed(allow: impl AsRef<str>) -> Response {
    let mut response = render_error_response(ErrorClassification::METHOD_NOT_ALLOWED);
    let s = allow.as_ref();
    if !s.is_empty()
        && let Ok(val) = HeaderValue::from_str(s)
    {
        response.headers_mut().insert(header::ALLOW, val);
    }
    response
}

pub fn not_found_response() -> Response {
    render_error_response(ErrorClassification::NOT_FOUND)
}

pub async fn not_found_handler() -> Response {
    not_found_response()
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let classification = self.classification();
        render_error_response(classification)
    }
}
