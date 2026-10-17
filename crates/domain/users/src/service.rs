use crate::{
    models::{PageRequest, User, UserPage, validate_name},
    repo::UserRepo,
};
use async_trait::async_trait;
use hypgm_errors::{PublicError, PublicErrorInfo, RepoError};
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum UserError {
    #[error("Validation failed: {0}")]
    Validation(String),
    #[error("User not found")]
    NotFound,
    #[error(transparent)]
    Repo(RepoError),
    #[error("Remote user service failed: {0}")]
    Remote(String),
    #[error("Invalid user service response: {0}")]
    InvalidResponse(&'static str),
}

impl From<RepoError> for UserError {
    fn from(err: RepoError) -> Self {
        match err {
            RepoError::NotFound => UserError::NotFound,
            other => UserError::Repo(other),
        }
    }
}

impl PublicError for UserError {
    fn public_error(&self) -> PublicErrorInfo {
        match self {
            UserError::Validation(_) => PublicErrorInfo::VALIDATION_ERROR,
            UserError::NotFound => PublicErrorInfo::NOT_FOUND,
            UserError::Repo(repo) => repo.public_error(),
            UserError::Remote(_) | UserError::InvalidResponse(_) => {
                PublicErrorInfo::INTERNAL_ERROR
            }
        }
    }

    fn to_tonic_status(&self) -> tonic::Status {
        match self {
            UserError::Validation(msg) => tonic::Status::invalid_argument(msg.clone()),
            UserError::NotFound => tonic::Status::not_found("Entity not found"),
            UserError::Repo(repo) => repo.to_tonic_status(),
            UserError::Remote(_) => tonic::Status::internal("Remote service failed"),
            UserError::InvalidResponse(_) => tonic::Status::internal("Invalid service response"),
        }
    }
}

impl From<UserError> for tonic::Status {
    fn from(err: UserError) -> Self {
        err.to_tonic_status()
    }
}

#[async_trait]
pub trait UserService: Send + Sync {
    async fn create(&self, name: &str) -> Result<User, UserError>;
    async fn get(&self, id: Uuid) -> Result<User, UserError>;
    async fn list(&self, page: PageRequest) -> Result<UserPage, UserError>;
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, UserError>;
    async fn delete(&self, id: Uuid) -> Result<(), UserError>;
}

pub struct DefaultUserService {
    repo: Arc<dyn UserRepo>,
}
impl DefaultUserService {
    pub fn new(repo: Arc<dyn UserRepo>) -> Self {
        Self { repo }
    }
}

fn normalize(name: &str) -> Result<&str, UserError> {
    let name = name.trim();
    validate_name(name).map_err(|_| {
        UserError::Validation("Display name must contain 1..200 characters without NUL".into())
    })?;
    Ok(name)
}

#[async_trait]
impl UserService for DefaultUserService {
    async fn create(&self, name: &str) -> Result<User, UserError> {
        Ok(self.repo.create(Uuid::new_v4(), normalize(name)?).await?)
    }
    async fn get(&self, id: Uuid) -> Result<User, UserError> {
        Ok(self.repo.get(id).await?)
    }
    async fn list(&self, page: PageRequest) -> Result<UserPage, UserError> {
        Ok(self.repo.list(page).await?)
    }
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, UserError> {
        Ok(self.repo.update_name(id, normalize(name)?).await?)
    }
    async fn delete(&self, id: Uuid) -> Result<(), UserError> {
        Ok(self.repo.delete(id).await?)
    }
}
