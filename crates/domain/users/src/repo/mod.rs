pub mod entity;
mod postgres;
use crate::models::{PageRequest, User, UserPage};
use async_trait::async_trait;
use errors::RepoError;
pub use entity::{ActiveModel, Column, Entity, Model};
pub use postgres::{PgUserRepo, check_ready};
use uuid::Uuid;

#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn create(&self, id: Uuid, name: &str) -> Result<User, RepoError>;
    async fn get(&self, id: Uuid) -> Result<User, RepoError>;
    async fn list(&self, page: PageRequest) -> Result<UserPage, RepoError>;
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, RepoError>;
    async fn delete(&self, id: Uuid) -> Result<(), RepoError>;
}
