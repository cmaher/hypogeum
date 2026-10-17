use crate::{
    models::{PageRequest, User, UserPage, validate_name},
    repo::UserRepo,
};
use async_trait::async_trait;
use hypgm_errors::ServiceError;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
pub trait UserService: Send + Sync {
    async fn create(&self, name: &str) -> Result<User, ServiceError>;
    async fn get(&self, id: Uuid) -> Result<User, ServiceError>;
    async fn list(&self, page: PageRequest) -> Result<UserPage, ServiceError>;
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, ServiceError>;
    async fn delete(&self, id: Uuid) -> Result<(), ServiceError>;
}

pub struct DefaultUserService {
    repo: Arc<dyn UserRepo>,
}
impl DefaultUserService {
    pub fn new(repo: Arc<dyn UserRepo>) -> Self {
        Self { repo }
    }
}

fn normalize(name: &str) -> Result<&str, ServiceError> {
    let name = name.trim();
    validate_name(name).map_err(|_| {
        ServiceError::Validation("Display name must contain 1..200 characters without NUL".into())
    })?;
    Ok(name)
}

#[async_trait]
impl UserService for DefaultUserService {
    async fn create(&self, name: &str) -> Result<User, ServiceError> {
        Ok(self.repo.create(Uuid::new_v4(), normalize(name)?).await?)
    }
    async fn get(&self, id: Uuid) -> Result<User, ServiceError> {
        Ok(self.repo.get(id).await?)
    }
    async fn list(&self, page: PageRequest) -> Result<UserPage, ServiceError> {
        Ok(self.repo.list(page).await?)
    }
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, ServiceError> {
        Ok(self.repo.update_name(id, normalize(name)?).await?)
    }
    async fn delete(&self, id: Uuid) -> Result<(), ServiceError> {
        Ok(self.repo.delete(id).await?)
    }
}
