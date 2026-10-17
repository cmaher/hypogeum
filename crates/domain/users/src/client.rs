use crate::{
    models::{PageRequest, User, UserPage},
    service::UserService,
    wire,
};
use async_trait::async_trait;
use hypgm_errors::{RepoError, ServiceError};
use hypgm_rpc::users::{self as pb, users_client::UsersClient};
use tonic::{Code, Status, transport::Channel};
use uuid::Uuid;

/// The client shares the plain-object service interface so callers can inject
/// either a remote client or a local implementation behind the same trait.
pub trait UserClient: UserService {}

#[derive(Clone)]
pub struct GrpcUserClient {
    client: UsersClient<Channel>,
}
impl GrpcUserClient {
    pub fn new(channel: Channel) -> Self {
        Self {
            client: UsersClient::new(channel),
        }
    }
    pub async fn connect(endpoint: String) -> Result<Self, ServiceError> {
        let client = UsersClient::connect(endpoint)
            .await
            .map_err(ServiceError::remote)?;
        Ok(Self { client })
    }
}
impl UserClient for GrpcUserClient {}

fn error(status: Status) -> ServiceError {
    match status.code() {
        Code::NotFound => RepoError::NotFound.into(),
        Code::InvalidArgument => ServiceError::Validation(status.message().into()),
        _ => ServiceError::remote(status),
    }
}
fn user(response: pb::User) -> Result<User, ServiceError> {
    wire::from_wire(response)
}

#[async_trait]
impl UserService for GrpcUserClient {
    async fn create(&self, name: &str) -> Result<User, ServiceError> {
        user(
            self.client
                .clone()
                .create_user(pb::CreateUserRequest {
                    display_name: name.into(),
                })
                .await
                .map_err(error)?
                .into_inner(),
        )
    }
    async fn get(&self, id: Uuid) -> Result<User, ServiceError> {
        user(
            self.client
                .clone()
                .get_user(pb::GetUserRequest { id: id.to_string() })
                .await
                .map_err(error)?
                .into_inner(),
        )
    }
    async fn list(&self, page: PageRequest) -> Result<UserPage, ServiceError> {
        let response = self
            .client
            .clone()
            .list_users(pb::ListUsersRequest {
                after_id: page.after().map(|id| id.to_string()).unwrap_or_default(),
                page_size: page.limit(),
            })
            .await
            .map_err(error)?
            .into_inner();
        let next_after = if response.next_after_id.is_empty() {
            None
        } else {
            Some(
                wire::id(&response.next_after_id)
                    .map_err(|_| ServiceError::InvalidResponse("Invalid pagination cursor"))?,
            )
        };
        Ok(UserPage {
            users: response
                .users
                .into_iter()
                .map(user)
                .collect::<Result<_, _>>()?,
            next_after,
        })
    }
    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, ServiceError> {
        user(
            self.client
                .clone()
                .update_user(pb::UpdateUserRequest {
                    id: id.to_string(),
                    display_name: name.into(),
                })
                .await
                .map_err(error)?
                .into_inner(),
        )
    }
    async fn delete(&self, id: Uuid) -> Result<(), ServiceError> {
        self.client
            .clone()
            .delete_user(pb::DeleteUserRequest { id: id.to_string() })
            .await
            .map_err(error)?;
        Ok(())
    }
}
