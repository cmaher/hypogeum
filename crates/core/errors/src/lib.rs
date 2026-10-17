use http::StatusCode;
use thiserror::Error;

type Cause = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicErrorInfo {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}

impl PublicErrorInfo {
    pub const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    pub const NOT_FOUND: Self = Self {
        status: StatusCode::NOT_FOUND,
        code: "not_found",
        message: "Resource not found",
    };

    pub const BAD_REQUEST: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: "bad_request",
        message: "Bad request",
    };

    pub const VALIDATION_ERROR: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: "validation_error",
        message: "Validation failed",
    };

    pub const CONFLICT: Self = Self {
        status: StatusCode::CONFLICT,
        code: "conflict",
        message: "Conflict",
    };

    pub const INTERNAL_ERROR: Self = Self {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal_error",
        message: "Internal server error",
    };

    pub const INVALID_REQUEST: Self = Self {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_request",
        message: "Invalid request",
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

    pub const fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message,
        }
    }

    pub const fn method_not_allowed(message: &'static str) -> Self {
        Self {
            status: StatusCode::METHOD_NOT_ALLOWED,
            code: "method_not_allowed",
            message,
        }
    }

    pub const fn payload_too_large(message: &'static str) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message,
        }
    }

    pub const fn unsupported_media_type(message: &'static str) -> Self {
        Self {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "unsupported_media_type",
            message,
        }
    }

    pub const fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message,
        }
    }

    pub const fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message,
        }
    }

    pub const fn validation_error(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "validation_error",
            message,
        }
    }

    pub const fn conflict(message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message,
        }
    }

    pub const fn internal_error(message: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message,
        }
    }
}

impl std::fmt::Display for PublicErrorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PublicErrorInfo {}

impl PublicError for PublicErrorInfo {
    fn public_error(&self) -> PublicErrorInfo {
        *self
    }
}

pub trait PublicError: std::error::Error + Send + Sync {
    fn public_error(&self) -> PublicErrorInfo;

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        let info = self.public_error();
        match info.status {
            StatusCode::NOT_FOUND => tonic::Status::not_found(info.message),
            StatusCode::BAD_REQUEST => tonic::Status::invalid_argument(info.message),
            StatusCode::CONFLICT => tonic::Status::already_exists(info.message),
            StatusCode::PAYLOAD_TOO_LARGE => tonic::Status::resource_exhausted(info.message),
            _ => tonic::Status::internal(info.message),
        }
    }
}

#[derive(Debug, Error)]
#[error("Invalid configuration: {0}")]
pub struct ConfigError(pub String);

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("Migration invariant failed: {0}")]
    InvalidState(String),
    #[error("Database administration failed")]
    Database(#[source] Cause),
}
impl MigrationError {
    pub fn database<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Database(Box::new(error))
    }
}

impl PublicError for MigrationError {
    fn public_error(&self) -> PublicErrorInfo {
        PublicErrorInfo::INTERNAL_ERROR
    }

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        tonic::Status::internal("Database administration failed")
    }
}

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Entity not found")]
    NotFound,
    #[error("Database operation failed")]
    Database(#[source] Cause),
}
impl RepoError {
    pub fn database<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Database(Box::new(error))
    }
}

impl PublicError for RepoError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            RepoError::InvalidInput(_) => PublicErrorInfo::VALIDATION_ERROR,
            RepoError::NotFound => PublicErrorInfo::NOT_FOUND,
            RepoError::Database(_) => PublicErrorInfo::INTERNAL_ERROR,
        }
    }

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        match self {
            RepoError::InvalidInput(message) => tonic::Status::invalid_argument(message.clone()),
            RepoError::NotFound => tonic::Status::not_found("Entity not found"),
            RepoError::Database(_) => tonic::Status::internal("Database operation failed"),
        }
    }
}

#[deprecated(note = "Domain crates should define their own errors implementing PublicError")]
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Invalid service response: {0}")]
    InvalidResponse(&'static str),
    #[error("Validation failed: {0}")]
    Validation(String),
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error("Remote service failed")]
    Remote(#[source] Cause),
}
#[allow(deprecated)]
impl ServiceError {
    pub fn remote<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Remote(Box::new(error))
    }
}

#[allow(deprecated)]
impl PublicError for ServiceError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            ServiceError::Validation(_) => PublicErrorInfo::VALIDATION_ERROR,
            ServiceError::Repo(repo) => repo.public_error(),
            ServiceError::Remote(_) | ServiceError::InvalidResponse(_) => {
                PublicErrorInfo::INTERNAL_ERROR
            }
        }
    }

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        match self {
            ServiceError::Validation(message) => tonic::Status::invalid_argument(message.clone()),
            ServiceError::Repo(error) => error.to_tonic_status(),
            ServiceError::Remote(_) => tonic::Status::internal("Remote service failed"),
            ServiceError::InvalidResponse(_) => tonic::Status::internal("Invalid service response"),
        }
    }
}

#[cfg(feature = "tonic")]
impl From<RepoError> for tonic::Status {
    fn from(error: RepoError) -> Self {
        error.to_tonic_status()
    }
}
#[cfg(feature = "tonic")]
#[allow(deprecated)]
impl From<ServiceError> for tonic::Status {
    fn from(error: ServiceError) -> Self {
        error.to_tonic_status()
    }
}

/// Infrastructure composition failures; task state/errors remain upstream Apalis types.
#[derive(Debug, Error)]
pub enum TasksError {
    #[error("Invalid task framework configuration: {0}")]
    Config(String),
    #[error("Tasks runtime requires migrated Apalis schema and hypogeum_tasks credentials")]
    NotReady,
    #[error("Task framework operation failed: {0}")]
    Operation(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("Task worker drain deadline exceeded")]
    DrainTimeout,
}
impl TasksError {
    pub fn operation<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Operation(Box::new(error))
    }
}

/// Byte-storage errors. Operation labels contain no filesystem paths or credentials.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Invalid storage input: {0}")]
    InvalidInput(String),
    #[error("Invalid storage reference: {0}")]
    InvalidReference(String),
    #[error("Storage object not found")]
    NotFound(#[source] std::io::Error),
    #[error("Storage permission denied")]
    PermissionDenied(#[source] std::io::Error),
    #[error("Storage integrity mismatch")]
    IntegrityMismatch,
    #[error("Unsupported storage guarantee or operation: {operation}")]
    Unsupported {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("Storage object exceeds maximum of {limit} bytes")]
    SizeLimit { limit: u64 },
    #[error("Storage I/O failed: {operation}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// Installation/unlink may have taken effect. Never automatically remove a
    /// potentially completed object. The common crate deliberately has no ObjectRef dependency.
    #[error("Storage completion is uncertain: {operation}")]
    UncertainCompletion {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl StorageError {
    pub fn io(operation: &'static str, source: std::io::Error) -> Self {
        match source.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound(source),
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied(source),
            std::io::ErrorKind::Unsupported => Self::Unsupported { operation, source },
            _ => Self::Io { operation, source },
        }
    }
}

impl PublicError for TasksError {
    fn public_error(&self) -> PublicErrorInfo {
        PublicErrorInfo::INTERNAL_ERROR
    }

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        tonic::Status::internal("Task framework operation failed")
    }
}

impl PublicError for StorageError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            StorageError::NotFound(_) => PublicErrorInfo::NOT_FOUND,
            StorageError::InvalidInput(_) => PublicErrorInfo::VALIDATION_ERROR,
            StorageError::InvalidReference(_) => PublicErrorInfo::BAD_REQUEST,
            StorageError::IntegrityMismatch => PublicErrorInfo::CONFLICT,
            StorageError::SizeLimit { .. } => PublicErrorInfo::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Payload too large",
            ),
            StorageError::PermissionDenied(_)
            | StorageError::Unsupported { .. }
            | StorageError::Io { .. }
            | StorageError::UncertainCompletion { .. } => PublicErrorInfo::INTERNAL_ERROR,
        }
    }

    #[cfg(feature = "tonic")]
    fn to_tonic_status(&self) -> tonic::Status {
        match self {
            StorageError::NotFound(_) => tonic::Status::not_found("Storage object not found"),
            StorageError::InvalidInput(message) => tonic::Status::invalid_argument(message.clone()),
            StorageError::InvalidReference(message) => {
                tonic::Status::invalid_argument(message.clone())
            }
            StorageError::IntegrityMismatch => {
                tonic::Status::already_exists("Storage integrity mismatch")
            }
            StorageError::SizeLimit { limit } => tonic::Status::resource_exhausted(format!(
                "Storage object exceeds maximum of {limit} bytes"
            )),
            StorageError::PermissionDenied(_)
            | StorageError::Unsupported { .. }
            | StorageError::Io { .. }
            | StorageError::UncertainCompletion { .. } => {
                tonic::Status::internal("Storage operation failed")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_error_info_constants() {
        assert_eq!(PublicErrorInfo::NOT_FOUND.status, StatusCode::NOT_FOUND);
        assert_eq!(PublicErrorInfo::NOT_FOUND.code, "not_found");

        assert_eq!(PublicErrorInfo::BAD_REQUEST.status, StatusCode::BAD_REQUEST);
        assert_eq!(PublicErrorInfo::BAD_REQUEST.code, "bad_request");

        assert_eq!(
            PublicErrorInfo::VALIDATION_ERROR.status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(PublicErrorInfo::VALIDATION_ERROR.code, "validation_error");

        assert_eq!(PublicErrorInfo::CONFLICT.status, StatusCode::CONFLICT);
        assert_eq!(PublicErrorInfo::CONFLICT.code, "conflict");

        assert_eq!(
            PublicErrorInfo::INTERNAL_ERROR.status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(PublicErrorInfo::INTERNAL_ERROR.code, "internal_error");

        assert_eq!(
            PublicErrorInfo::INVALID_REQUEST.status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(PublicErrorInfo::INVALID_REQUEST.code, "invalid_request");

        assert_eq!(
            PublicErrorInfo::METHOD_NOT_ALLOWED.status,
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            PublicErrorInfo::METHOD_NOT_ALLOWED.code,
            "method_not_allowed"
        );

        assert_eq!(
            PublicErrorInfo::PAYLOAD_TOO_LARGE.status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(PublicErrorInfo::PAYLOAD_TOO_LARGE.code, "payload_too_large");

        assert_eq!(
            PublicErrorInfo::UNSUPPORTED_MEDIA_TYPE.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            PublicErrorInfo::UNSUPPORTED_MEDIA_TYPE.code,
            "unsupported_media_type"
        );

        let info = PublicErrorInfo::NOT_FOUND;
        assert_eq!(info.public_error(), PublicErrorInfo::NOT_FOUND);
        assert_eq!(info.to_string(), "not_found: Resource not found");
    }

    #[test]
    fn test_repo_error_conversions() {
        let not_found = RepoError::NotFound;
        assert_eq!(not_found.public_error(), PublicErrorInfo::NOT_FOUND);

        let invalid = RepoError::InvalidInput("bad name".into());
        assert_eq!(invalid.public_error(), PublicErrorInfo::VALIDATION_ERROR);

        let db_err = RepoError::database(std::io::Error::other("secret credentials"));
        assert_eq!(db_err.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        #[cfg(feature = "tonic")]
        {
            assert_eq!(not_found.to_tonic_status().code(), tonic::Code::NotFound);
            assert_eq!(
                invalid.to_tonic_status().code(),
                tonic::Code::InvalidArgument
            );
            let status = db_err.to_tonic_status();
            assert_eq!(status.code(), tonic::Code::Internal);
            assert!(!status.message().contains("secret credentials"));
        }
    }

    #[test]
    fn test_storage_error_conversions() {
        let not_found = StorageError::NotFound(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(not_found.public_error(), PublicErrorInfo::NOT_FOUND);

        let invalid_input = StorageError::InvalidInput("too short".into());
        assert_eq!(
            invalid_input.public_error(),
            PublicErrorInfo::VALIDATION_ERROR
        );

        let invalid_ref = StorageError::InvalidReference("../escape".into());
        assert_eq!(invalid_ref.public_error(), PublicErrorInfo::BAD_REQUEST);

        let conflict = StorageError::IntegrityMismatch;
        assert_eq!(conflict.public_error(), PublicErrorInfo::CONFLICT);

        let limit = StorageError::SizeLimit { limit: 1024 };
        let info = limit.public_error();
        assert_eq!(info.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(info.code, "payload_too_large");

        let io_err = StorageError::Io {
            operation: "write",
            source: std::io::Error::other("/etc/shadow password"),
        };
        assert_eq!(io_err.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        #[cfg(feature = "tonic")]
        {
            assert_eq!(not_found.to_tonic_status().code(), tonic::Code::NotFound);
            assert_eq!(
                invalid_input.to_tonic_status().code(),
                tonic::Code::InvalidArgument
            );
            assert_eq!(
                conflict.to_tonic_status().code(),
                tonic::Code::AlreadyExists
            );
            assert_eq!(
                limit.to_tonic_status().code(),
                tonic::Code::ResourceExhausted
            );
            let status = io_err.to_tonic_status();
            assert_eq!(status.code(), tonic::Code::Internal);
            assert!(!status.message().contains("/etc/shadow"));
        }
    }

    #[test]
    fn test_tasks_error_conversions() {
        let config = TasksError::Config("bad config".into());
        assert_eq!(config.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        let not_ready = TasksError::NotReady;
        assert_eq!(not_ready.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        let op = TasksError::operation(std::io::Error::other("private diagnostic"));
        assert_eq!(op.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        let drain = TasksError::DrainTimeout;
        assert_eq!(drain.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        #[cfg(feature = "tonic")]
        {
            let status = op.to_tonic_status();
            assert_eq!(status.code(), tonic::Code::Internal);
            assert!(!status.message().contains("private diagnostic"));
        }
    }

    #[test]
    fn test_migration_error_conversions() {
        let invalid = MigrationError::InvalidState("corrupt history".into());
        assert_eq!(invalid.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        let db = MigrationError::database(std::io::Error::other(
            "connection leak postgres://admin:pass@host/db",
        ));
        assert_eq!(db.public_error(), PublicErrorInfo::INTERNAL_ERROR);

        #[cfg(feature = "tonic")]
        {
            let status = db.to_tonic_status();
            assert_eq!(status.code(), tonic::Code::Internal);
            assert!(!status.message().contains("admin:pass"));
        }
    }
}
