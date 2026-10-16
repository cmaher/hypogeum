use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinHandle,
};

/// Test-only PostgreSQL proxy that holds the next BEGIN's ReadyForQuery reply.
/// Clients must disable TLS so backend protocol messages remain visible.
pub(super) struct BeginBarrier {
    port: u16,
    armed: Arc<AtomicBool>,
    reached: Arc<Notify>,
    release: Arc<Notify>,
    proxy: JoinHandle<io::Result<()>>,
}

impl BeginBarrier {
    pub(super) async fn start(host: &str, port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let upstream = TcpStream::connect((host, port)).await.unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let reached = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let control = (armed.clone(), reached.clone(), release.clone());
        let proxy = tokio::spawn(async move {
            // The regression uses a pool with exactly one connection.
            let (client, _) = listener.accept().await?;
            let (mut client_read, mut client_write) = client.into_split();
            let (mut upstream_read, mut upstream_write) = upstream.into_split();
            let requests = async {
                tokio::io::copy(&mut client_read, &mut upstream_write).await?;
                upstream_write.shutdown().await
            };
            let responses = async {
                loop {
                    let mut header = [0; 5];
                    match upstream_read.read_exact(&mut header).await {
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                        Err(error) => return Err(error),
                    }
                    let length = u32::from_be_bytes(header[1..].try_into().unwrap())
                        .checked_sub(4)
                        .ok_or_else(|| io::Error::other("invalid PostgreSQL message length"))?;
                    let mut body = vec![0; length as usize];
                    upstream_read.read_exact(&mut body).await?;
                    // ReadyForQuery with status T confirms the server has opened
                    // the transaction. Hold it before SQLx can read the reply.
                    if header[0] == b'Z' && body == b"T" && control.0.swap(false, Ordering::SeqCst)
                    {
                        control.1.notify_one();
                        control.2.notified().await;
                    }
                    client_write.write_all(&header).await?;
                    client_write.write_all(&body).await?;
                }
            };
            tokio::try_join!(requests, responses)?;
            Ok(())
        });
        Self {
            port: proxy_port,
            armed,
            reached,
            release,
            proxy,
        }
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }
    pub(super) fn arm_handle(&self) -> Arc<AtomicBool> {
        self.armed.clone()
    }
    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
    pub(super) async fn reached(&self) {
        self.reached.notified().await;
    }
    pub(super) fn release(&self) {
        self.release.notify_one();
    }
    pub(super) async fn finish(&mut self) {
        match (&mut self.proxy).await.unwrap() {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) => {}
            Err(error) => panic!("BEGIN proxy failed: {error}"),
        }
    }
}

impl Drop for BeginBarrier {
    fn drop(&mut self) {
        self.proxy.abort();
    }
}
