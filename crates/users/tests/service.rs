use async_trait::async_trait;
use errors::{RepoError, ServiceError};
use std::sync::{Arc, Mutex};
use users::{models::{PageRequest, User, UserPage}, repo::UserRepo, service::{DefaultUserService, UserService}};
use uuid::Uuid;

#[derive(Default)]
struct RecordingRepo { names: Mutex<Vec<String>> }
#[async_trait]
impl UserRepo for RecordingRepo {
    async fn create(&self, id: Uuid, name: &str) -> Result<User, RepoError> {
        self.names.lock().unwrap().push(name.into());
        let now = chrono::Utc::now();
        Ok(User { id, display_name: name.into(), created_at: now, updated_at: now })
    }
    async fn get(&self, _: Uuid) -> Result<User, RepoError> { Err(RepoError::NotFound) }
    async fn list(&self, _: PageRequest) -> Result<UserPage, RepoError> { Ok(UserPage { users: vec![], next_after: None }) }
    async fn update_name(&self, _: Uuid, _: &str) -> Result<User, RepoError> { panic!("invalid names must not reach repository") }
    async fn delete(&self, _: Uuid) -> Result<(), RepoError> { Err(RepoError::NotFound) }
}

#[tokio::test]
async fn creates_a_new_identity_and_normalizes_display_name() {
    let repo = Arc::new(RecordingRepo::default());
    let service = DefaultUserService::new(repo.clone());
    let first = service.create("  Example User\n").await.unwrap();
    let second = service.create("Example User").await.unwrap();
    assert_eq!(first.display_name, "Example User");
    assert_ne!(first.id, second.id);
    assert_eq!(first.id.get_version_num(), 4);
    assert_eq!(repo.names.lock().unwrap().as_slice(), ["Example User", "Example User"]);
}

#[tokio::test]
async fn rejects_invalid_input_before_persistence_and_preserves_not_found() {
    let service = DefaultUserService::new(Arc::new(RecordingRepo::default()));
    for bad in [String::new(), " \t\n".into(), "界".repeat(201), "bad\0name".into()] {
        assert!(matches!(service.create(&bad).await, Err(ServiceError::Validation(_))));
        assert!(matches!(service.update_name(Uuid::new_v4(), &bad).await, Err(ServiceError::Validation(_))));
    }
    assert!(matches!(service.get(Uuid::new_v4()).await, Err(ServiceError::Repo(RepoError::NotFound))));
    assert!(matches!(service.delete(Uuid::new_v4()).await, Err(ServiceError::Repo(RepoError::NotFound))));
    assert!(service.list(PageRequest::default()).await.unwrap().users.is_empty());
    for limit in [0,101,u32::MAX] { assert!(PageRequest::new(None, limit).is_err()); }
}
