use async_trait::async_trait;
use axum::response::{IntoResponse, Response};
use hypgm_errors::{PublicError, PublicErrorInfo, RepoError};
use thiserror::Error;

/// Rejection message for names that are too long or contain NUL.
pub const NAME_VALIDATION_MESSAGE: &str =
    "Names can have at most 200 characters and cannot contain NUL characters.";

#[derive(Debug, Error)]
pub enum HelloWorldError {
    /// Carries a fixed, user-safe message that HTML views may display. The
    /// JSON API still returns the generic validation classification.
    #[error("Validation failed: {0}")]
    Validation(&'static str),
    #[error("Greeting not found")]
    NotFound,
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error("Internal service error: {0}")]
    Internal(String),
}

impl PublicError for HelloWorldError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            Self::Validation(_) => PublicErrorInfo::VALIDATION_ERROR,
            Self::NotFound => PublicErrorInfo::NOT_FOUND,
            Self::Repo(repo) => repo.public_error(),
            Self::Internal(_) => PublicErrorInfo::INTERNAL_ERROR,
        }
    }
}

impl IntoResponse for HelloWorldError {
    fn into_response(self) -> Response {
        hypgm_http::render_error_response(self.public_error())
    }
}

#[async_trait]
pub trait HelloWorldService: Send + Sync {
    async fn say_hello(&self, name: &str) -> Result<String, HelloWorldError>;
}

#[derive(Default)]
pub struct DefaultHelloWorldService;

#[async_trait]
impl HelloWorldService for DefaultHelloWorldService {
    async fn say_hello(&self, name: &str) -> Result<String, HelloWorldError> {
        let name = name.trim();
        if name.contains('\0') || name.chars().count() > 200 {
            return Err(HelloWorldError::Validation(NAME_VALIDATION_MESSAGE));
        }
        let name = if name.is_empty() { "world" } else { name };
        Ok(format!("Hello, {name}!"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn greetings_trim_names_and_default_to_world() {
        for (name, expected) in [
            ("", "Hello, world!"),
            (" \n\t\u{2003}", "Hello, world!"),
            ("  Ada  ", "Hello, Ada!"),
            ("\u{2003}世界 🦀\u{2003}", "Hello, 世界 🦀!"),
            ("Ada  Lovelace", "Hello, Ada  Lovelace!"),
        ] {
            assert_eq!(
                DefaultHelloWorldService.say_hello(name).await.unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn validation_counts_unicode_scalars_after_trimming() {
        let name = "🦀".repeat(200);
        assert_eq!(
            DefaultHelloWorldService
                .say_hello(&format!("  {name}  "))
                .await
                .unwrap(),
            format!("Hello, {name}!")
        );
        for name in ["🦀".repeat(201), "x\0y".into(), "\0".into()] {
            assert!(matches!(
                DefaultHelloWorldService.say_hello(&name).await,
                Err(HelloWorldError::Validation(_))
            ));
        }
    }
}
