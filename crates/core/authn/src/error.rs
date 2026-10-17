use http::StatusCode;
use hypgm_errors::{PublicError, PublicErrorInfo};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Missing credentials")]
    MissingCredentials,

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Token expired")]
    ExpiredToken,

    #[error("Session error: {0}")]
    Session(String),

    #[error("Internal authentication error: {0}")]
    Internal(String),
}

impl AuthError {
    pub const UNAUTHENTICATED_MISSING: PublicErrorInfo = PublicErrorInfo::new(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Authentication required",
    );

    pub const UNAUTHENTICATED_INVALID: PublicErrorInfo = PublicErrorInfo::new(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Invalid authentication credentials",
    );

    pub const UNAUTHENTICATED_EXPIRED: PublicErrorInfo = PublicErrorInfo::new(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Authentication token expired",
    );
}

impl PublicError for AuthError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::MissingCredentials => Self::UNAUTHENTICATED_MISSING,
            Self::InvalidCredentials | Self::InvalidToken(_) | Self::Session(_) => {
                Self::UNAUTHENTICATED_INVALID
            }
            Self::ExpiredToken => Self::UNAUTHENTICATED_EXPIRED,
            Self::Internal(_) => PublicErrorInfo::INTERNAL_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_error_public_error_mapping() {
        let missing = AuthError::MissingCredentials;
        let info = missing.public_error();
        assert_eq!(info.status, StatusCode::UNAUTHORIZED);
        assert_eq!(info.code, "unauthenticated");
        assert_eq!(info.message, "Authentication required");

        let invalid = AuthError::InvalidCredentials;
        let info = invalid.public_error();
        assert_eq!(info.status, StatusCode::UNAUTHORIZED);
        assert_eq!(info.code, "unauthenticated");

        let token_err = AuthError::InvalidToken("bad secret key signature".to_string());
        let info = token_err.public_error();
        assert_eq!(info.status, StatusCode::UNAUTHORIZED);
        assert_eq!(info.code, "unauthenticated");
        assert!(!info.message.contains("bad secret"));

        let expired = AuthError::ExpiredToken;
        let info = expired.public_error();
        assert_eq!(info.status, StatusCode::UNAUTHORIZED);
        assert_eq!(info.code, "unauthenticated");

        let internal = AuthError::Internal("db connection timeout".to_string());
        let info = internal.public_error();
        assert_eq!(info.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(info.code, "internal_error");
        assert!(!info.message.contains("db connection"));
    }
}
