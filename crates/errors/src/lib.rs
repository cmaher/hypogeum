use thiserror::Error;

type Cause = Box<dyn std::error::Error + Send + Sync>;

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
impl ServiceError {
    pub fn remote<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Remote(Box::new(error))
    }
}

#[cfg(feature = "tonic")]
impl From<RepoError> for tonic::Status {
    fn from(error: RepoError) -> Self {
        match error {
            RepoError::InvalidInput(message) => Self::invalid_argument(message),
            RepoError::NotFound => Self::not_found("Entity not found"),
            RepoError::Database(_) => Self::internal("Database operation failed"),
        }
    }
}
#[cfg(feature = "tonic")]
impl From<ServiceError> for tonic::Status {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::Validation(message) => Self::invalid_argument(message),
            ServiceError::Repo(error) => error.into(),
            ServiceError::Remote(_) => Self::internal("Remote service failed"),
            ServiceError::InvalidResponse(_) => Self::internal("Invalid service response"),
        }
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
