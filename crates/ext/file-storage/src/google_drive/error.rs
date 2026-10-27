use crate::StorageError;
use std::io;

pub(super) fn io(operation: &'static str) -> StorageError {
    StorageError::Io {
        operation,
        source: io::Error::other("Drive request failed"),
    }
}

pub(super) fn permission() -> StorageError {
    StorageError::PermissionDenied(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Drive authentication failed",
    ))
}

pub(super) fn status(code: u16, reason: Option<&str>, operation: &'static str) -> StorageError {
    // Only recognized protocol reasons are retained. A remote reason can itself
    // contain secrets; never copy arbitrary server strings into an error chain.
    let rate_limit = matches!(reason, Some("rateLimitExceeded" | "userRateLimitExceeded"));
    let description = if rate_limit {
        format!("Drive HTTP {code}: rate limit")
    } else {
        format!("Drive HTTP {code}")
    };
    let source = io::Error::other(description);
    match code {
        404 => StorageError::NotFound(source),
        401 | 403 if !rate_limit => StorageError::PermissionDenied(source),
        _ => StorageError::Io { operation, source },
    }
}

pub(super) fn json_status(
    code: u16,
    value: &serde_json::Value,
    operation: &'static str,
) -> StorageError {
    let reasons = value.pointer("/error/errors").and_then(|e| e.as_array());
    let reason = reasons.and_then(|errors| {
        errors
            .iter()
            .filter_map(|e| e.get("reason").and_then(|r| r.as_str()))
            .find(|r| matches!(*r, "rateLimitExceeded" | "userRateLimitExceeded"))
    });
    status(code, reason, operation)
}

pub(super) fn api(error: google_drive3::Error, operation: &'static str) -> StorageError {
    match error {
        google_drive3::Error::MissingToken(_) => permission(),
        google_drive3::Error::Failure(response) => {
            status(response.status().as_u16(), None, operation)
        }
        google_drive3::Error::BadRequest(value) => {
            let code = value
                .pointer("/error/code")
                .and_then(|c| c.as_u64())
                .and_then(|c| u16::try_from(c).ok())
                .unwrap_or(500);
            json_status(code, &value, operation)
        }
        _ => io(operation),
    }
}
