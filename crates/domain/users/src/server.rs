use crate::{models::PageRequest, service::UserService, wire};
use hypgm_rpc::users::{
    self as pb,
    users_server::{Users, UsersServer},
};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct GrpcUserServer {
    service: Arc<dyn UserService>,
}
impl GrpcUserServer {
    pub fn new(service: Arc<dyn UserService>) -> Self {
        Self { service }
    }
    pub fn into_service(self) -> UsersServer<Self> {
        UsersServer::new(self)
    }
}

#[tonic::async_trait]
impl Users for GrpcUserServer {
    async fn create_user(
        &self,
        request: Request<pb::CreateUserRequest>,
    ) -> Result<Response<pb::User>, Status> {
        Ok(Response::new(wire::to_wire(
            self.service
                .create(&request.into_inner().display_name)
                .await?,
        )))
    }
    async fn get_user(
        &self,
        request: Request<pb::GetUserRequest>,
    ) -> Result<Response<pb::User>, Status> {
        Ok(Response::new(wire::to_wire(
            self.service
                .get(wire::id(&request.into_inner().id)?)
                .await?,
        )))
    }
    async fn list_users(
        &self,
        request: Request<pb::ListUsersRequest>,
    ) -> Result<Response<pb::ListUsersResponse>, Status> {
        let request = request.into_inner();
        let after = if request.after_id.is_empty() {
            None
        } else {
            Some(wire::id(&request.after_id)?)
        };
        let size = if request.page_size == 0 {
            50
        } else {
            request.page_size
        };
        let page = PageRequest::new(after, size).map_err(Status::from)?;
        let result = self.service.list(page).await?;
        Ok(Response::new(pb::ListUsersResponse {
            users: result.users.into_iter().map(wire::to_wire).collect(),
            next_after_id: result
                .next_after
                .map(|id| id.to_string())
                .unwrap_or_default(),
        }))
    }
    async fn update_user(
        &self,
        request: Request<pb::UpdateUserRequest>,
    ) -> Result<Response<pb::User>, Status> {
        let request = request.into_inner();
        Ok(Response::new(wire::to_wire(
            self.service
                .update_name(wire::id(&request.id)?, &request.display_name)
                .await?,
        )))
    }
    async fn delete_user(
        &self,
        request: Request<pb::DeleteUserRequest>,
    ) -> Result<Response<pb::DeleteUserResponse>, Status> {
        self.service
            .delete(wire::id(&request.into_inner().id)?)
            .await?;
        Ok(Response::new(pb::DeleteUserResponse {}))
    }
}
