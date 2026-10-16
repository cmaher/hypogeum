use anyhow::{Context, Result};
use std::{net::SocketAddr, time::Duration};
use tasks::board::Board;

#[tokio::main]
async fn main() -> Result<()> {
    let path =
        std::env::var("FOUNDATION_LOCAL_CONFIG").unwrap_or_else(|_| "env/local.secrets".into());
    let _ = dotenvy::from_path(path);
    let url = std::env::var("TASKS_DATABASE_URL").context("TASKS_DATABASE_URL required")?;
    let bind: SocketAddr = std::env::var("TASKS_BOARD_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8082".into())
        .parse()
        .context("Invalid TASKS_BOARD_BIND")?;
    let drain: u64 = std::env::var("TASKS_SHUTDOWN_SECONDS")
        .unwrap_or_else(|_| "30".into())
        .parse()
        .context("Invalid TASKS_SHUTDOWN_SECONDS")?;
    anyhow::ensure!(
        drain > 0 && drain <= 3600,
        "Shutdown must be 1..3600 seconds"
    );
    let pool = sqlx::PgPool::connect(&url)
        .await
        .map_err(|_| anyhow::anyhow!("Tasks connection failed (URL redacted)"))?;
    tasks::check_ready(&pool).await?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context("Cannot bind Board")?;
    // No production queues or handlers are registered by this infrastructure host.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, Board::new().router()).with_graceful_shutdown(async {
        let _ = stopped.await;
    });
    let serving = std::future::IntoFuture::into_future(server);
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => { result.context("Board server failed")?; }
        () = shutdown_signal() => {
            let _ = stop.send(());
            tokio::time::timeout(Duration::from_secs(drain), &mut serving).await.context("Board drain deadline exceeded")??;
        }
    }
    pool.close().await;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install termination signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
