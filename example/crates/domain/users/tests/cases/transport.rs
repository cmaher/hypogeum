use exemplum_dmn_users::{
    UserError,
    client::GrpcUserClient,
    models::PageRequest,
    repo::PgUserRepo,
    server::GrpcUserServer,
    service::{DefaultUserService, UserService},
};
use hypgm_rpc::users::{GetUserRequest, ListUsersRequest, users_client::UsersClient};
use hypgm_test_support::TestDatabase;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::Arc;
use uuid::Uuid;

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn user_client_round_trip_uses_real_repository(
    _pool: PgPoolOptions,
    options: PgConnectOptions,
) {
    let db = TestDatabase::provision(options).await;
    let config =
        hypgm_ext_rdbms::DatabaseConfig::new("exemplum_migrator", "exemplum_owner", "exemplum_tasks")
            .with_app_schema("exemplum")
            .with_domain_migrator(<dyn hypgm_ext_rdbms::DomainMigrator>::from_sea_orm::<
                exemplum_dmn_users::pg::migrations::Migrator,
            >());
    db.migrate_with(&config).await;
    let pool = sqlx::PgPool::connect(&db.users_url).await.unwrap();
    let service = Arc::new(DefaultUserService::new(Arc::new(PgUserRepo::new(
        pool.clone(),
    ))));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(GrpcUserServer::new(service.clone()).into_service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = stopped.await;
                },
            ),
    );
    let client = GrpcUserClient::connect(endpoint.clone()).await.unwrap();
    let first = client.create("  O'Reilly Example  ").await.unwrap();
    let second = client.create("Second User").await.unwrap();
    assert_eq!(first.display_name, "O'Reilly Example");
    assert_eq!(client.get(first.id).await.unwrap(), first);
    assert_eq!(service.get(first.id).await.unwrap(), first);
    let page = client
        .list(PageRequest::new(None, 1).unwrap())
        .await
        .unwrap();
    assert_eq!(page.users.len(), 1);
    let next = client
        .list(PageRequest::new(page.next_after, 1).unwrap())
        .await
        .unwrap();
    assert_eq!(next.users.len(), 1);
    assert_ne!(page.users[0].id, next.users[0].id);
    assert!(next.next_after.is_none());
    let updated = client.update_name(first.id, "  Renamed  ").await.unwrap();
    assert_eq!(updated.display_name, "Renamed");
    assert_eq!(updated.created_at, first.created_at);
    client.delete(first.id).await.unwrap();
    client.delete(second.id).await.unwrap();
    assert!(matches!(
        client.get(first.id).await,
        Err(UserError::NotFound)
    ));
    assert!(matches!(
        client.create(" ").await,
        Err(UserError::Validation(_))
    ));
    let mut raw = UsersClient::connect(endpoint).await.unwrap();
    assert_eq!(
        raw.get_user(GetUserRequest { id: "bad".into() })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        raw.list_users(ListUsersRequest {
            after_id: "bad".into(),
            page_size: 1
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        raw.list_users(ListUsersRequest {
            after_id: String::new(),
            page_size: 101
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        raw.get_user(GetUserRequest {
            id: Uuid::new_v4().to_string()
        })
        .await
        .unwrap_err()
        .code(),
        tonic::Code::NotFound
    );
    // A closed database pool must produce only a redacted transport message.
    pool.close().await;
    let error = raw
        .get_user(GetUserRequest {
            id: first.id.to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::Internal);
    assert_eq!(error.message(), "Database operation failed");
    drop(client);
    drop(raw);
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
