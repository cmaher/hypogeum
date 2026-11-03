use axum::http::StatusCode;
use hypgm_errors::{PublicError, PublicErrorInfo};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AuthError {
    /// The request carries no authenticated principal.
    #[error("Authentication required")]
    Unauthenticated,
}

impl AuthError {
    pub const UNAUTHENTICATED: PublicErrorInfo = PublicErrorInfo::new(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "Authentication required",
    );
}

impl PublicError for AuthError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Unauthenticated => Self::UNAUTHENTICATED,
        }
    }
}
