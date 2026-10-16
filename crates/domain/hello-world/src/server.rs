use crate::service::HelloWorldService;
use rpc::public::{
    HelloReply, HelloRequest,
    public_hello_world_server::{PublicHelloWorld, PublicHelloWorldServer},
};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct GrpcHelloWorldServer {
    service: Arc<dyn HelloWorldService>,
}

impl GrpcHelloWorldServer {
    pub fn new(service: Arc<dyn HelloWorldService>) -> Self {
        Self { service }
    }

    pub fn into_service(self) -> PublicHelloWorldServer<Self> {
        PublicHelloWorldServer::new(self)
    }
}

#[tonic::async_trait]
impl PublicHelloWorld for GrpcHelloWorldServer {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let message = self.service.say_hello(&request.into_inner().name).await?;
        Ok(Response::new(HelloReply { message }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::DefaultHelloWorldService;
    use errors::ServiceError;

    #[tokio::test]
    async fn adapter_returns_greeting_and_validation_status() {
        let server = GrpcHelloWorldServer::new(Arc::new(DefaultHelloWorldService));
        let reply = server
            .say_hello(Request::new(HelloRequest {
                name: " Ada ".into(),
            }))
            .await
            .unwrap();
        assert_eq!(reply.into_inner().message, "Hello, Ada!");
        let error = server
            .say_hello(Request::new(HelloRequest { name: "\0".into() }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
    }

    struct FailingService;
    #[tonic::async_trait]
    impl HelloWorldService for FailingService {
        async fn say_hello(&self, _: &str) -> Result<String, ServiceError> {
            Err(ServiceError::InvalidResponse("private internal detail"))
        }
    }

    #[tokio::test]
    async fn adapter_uses_injected_service_and_redacts_internal_errors() {
        let server = GrpcHelloWorldServer::new(Arc::new(FailingService));
        let error = server
            .say_hello(Request::new(HelloRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Internal);
        assert_eq!(error.message(), "Invalid service response");
    }
}
