use http::StatusCode;
use hypgm_errors::{PublicError, PublicErrorInfo};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthzError {
    #[error("Access denied by policy")]
    Forbidden,

    #[error("Policy load error: {0}")]
    PolicyLoadError(String),

    #[error("Evaluation error: {0}")]
    EvaluationError(String),
}

impl AuthzError {
    pub const FORBIDDEN: PublicErrorInfo = PublicErrorInfo::new(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Access denied by policy",
    );

    pub fn policy_load(err: impl std::fmt::Display) -> Self {
        Self::PolicyLoadError(err.to_string())
    }

    pub fn evaluation(err: impl std::fmt::Display) -> Self {
        Self::EvaluationError(err.to_string())
    }
}

impl PublicError for AuthzError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Forbidden => Self::FORBIDDEN,
            Self::PolicyLoadError(_) | Self::EvaluationError(_) => PublicErrorInfo::INTERNAL_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authz_error_forbidden_mapping() {
        let err = AuthzError::Forbidden;
        let info = err.public_error();
        assert_eq!(info.status, StatusCode::FORBIDDEN);
        assert_eq!(info.code, "forbidden");
        assert_eq!(info.message, "Access denied by policy");
    }

    #[test]
    fn test_authz_error_policy_load_error_redacted() {
        let secret_detail = "parse failed at line 42 with internal secret path /etc/secret";
        let err = AuthzError::PolicyLoadError(secret_detail.to_string());
        let info = err.public_error();
        assert_eq!(info.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(info.code, "internal_error");
        assert_eq!(info.message, "Internal server error");
        assert!(!info.message.contains("secret"));
    }

    #[test]
    fn test_authz_error_evaluation_error_redacted() {
        let secret_detail = "evaluation failed due to internal state overflow";
        let err = AuthzError::EvaluationError(secret_detail.to_string());
        let info = err.public_error();
        assert_eq!(info.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(info.code, "internal_error");
        assert_eq!(info.message, "Internal server error");
        assert!(!info.message.contains("overflow"));
    }
}
