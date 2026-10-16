use anyhow::{Context, Result};
use std::{net::SocketAddr, sync::Arc};
use users::{
    repo::{PgUserRepo, check_ready},
    server::GrpcUserServer,
    service::DefaultUserService,
};

#[tokio::main]
async fn main() -> Result<()> {
    let path =
        std::env::var("FOUNDATION_LOCAL_CONFIG").unwrap_or_else(|_| "env/local.secrets".into());
    if let Err(error) = dotenvy::from_path(path)
        && !error.not_found()
    {
        anyhow::bail!("Cannot load local configuration");
    }
    let url = std::env::var("USERS_DATABASE_URL").context("USERS_DATABASE_URL required")?;
    let bind: SocketAddr = std::env::var("USERS_BIND")
        .unwrap_or_else(|_| "127.0.0.1:50051".into())
        .parse()
        .context("Invalid USERS_BIND")?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let pool = postgres::create_pool(&postgres::PostgresConfig::new(url))
        .map_err(|_| anyhow::anyhow!("Invalid database configuration"))?;
    check_ready(&pool)
        .await
        .context("Users database is not ready; run explicit migrations")?;
    let service = Arc::new(DefaultUserService::new(Arc::new(PgUserRepo::new(
        pool.clone(),
    ))));
    tonic::transport::Server::builder()
        .add_service(GrpcUserServer::new(service).into_service())
        .serve_with_shutdown(bind, shutdown())
        .await
        .context("Users server failed")?;
    pool.close().await;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Install termination handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
