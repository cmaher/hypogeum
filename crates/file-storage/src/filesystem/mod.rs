//! Filesystem adapter. The root and its parents must be application-controlled.
//! Concurrent hostile filesystem mutation is outside this adapter's boundary.
mod read;
mod write;

use crate::{ExpectedIntegrity, StorageError};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Root must already exist. No environment variables are read.
#[derive(Debug, Clone)]
pub struct FilesystemConfig {
    pub root: PathBuf,
    pub store_id: String,
    pub max_object_bytes: u64,
}

pub struct FilesystemStorage {
    root: PathBuf,
    store_id: String,
    max_object_bytes: u64,
    ops: Arc<FsOps>,
}

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    LayoutSync,
    Create,
    Write,
    FileSync,
    Install,
    CompletedSync,
    StagingSync,
    Cleanup,
    Delete,
}

#[derive(Default)]
struct FsOps {
    #[cfg(test)]
    failures: std::sync::Mutex<Vec<Operation>>,
    #[cfg(test)]
    key: Option<String>,
    #[cfg(test)]
    install_then_error: Option<io::ErrorKind>,
    #[cfg(test)]
    cleanup_observer: std::sync::Mutex<Option<std::sync::mpsc::Sender<std::thread::ThreadId>>>,
}

impl FsOps {
    fn check(&self, _operation: Operation) -> io::Result<()> {
        #[cfg(test)]
        {
            let mut failures = self.failures.lock().unwrap();
            if let Some(index) = failures.iter().position(|op| *op == _operation) {
                failures.remove(index);
                return Err(io::Error::other("injected filesystem failure"));
            }
        }
        Ok(())
    }

    fn key(&self) -> String {
        #[cfg(test)]
        if let Some(key) = &self.key {
            return key.clone();
        }
        uuid::Uuid::new_v4().to_string()
    }

    fn install(&self, source: &Path, destination: &Path) -> io::Result<()> {
        self.check(Operation::Install)?;
        fs::hard_link(source, destination)?;
        #[cfg(test)]
        if let Some(kind) = self.install_then_error {
            return Err(io::Error::new(kind, "injected lost installation reply"));
        }
        Ok(())
    }

    fn cleanup(&self, path: &Path) {
        if self.check(Operation::Cleanup).is_ok() {
            let _ = fs::remove_file(path);
        }
        #[cfg(test)]
        if let Some(observer) = self.cleanup_observer.lock().unwrap().take() {
            let _ = observer.send(std::thread::current().id());
        }
    }

    fn sync_dir(&self, path: &Path, operation: Operation) -> io::Result<()> {
        self.check(operation)?;
        sync_directory(path)
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory synchronization profile is unavailable",
    ))
}

impl FilesystemStorage {
    pub async fn new(config: FilesystemConfig) -> Result<Self, StorageError> {
        Self::construct(config, Arc::new(FsOps::default())).await
    }

    async fn construct(config: FilesystemConfig, ops: Arc<FsOps>) -> Result<Self, StorageError> {
        if config.store_id.trim().is_empty() || config.max_object_bytes == 0 {
            return Err(StorageError::InvalidInput(
                "store ID must be nonempty and maximum bytes positive".into(),
            ));
        }
        blocking(move || {
            initialize(&config.root, &ops)?;
            let root =
                fs::canonicalize(&config.root).map_err(|e| StorageError::io("resolve root", e))?;
            Ok(Self {
                root,
                store_id: config.store_id,
                max_object_bytes: config.max_object_bytes,
                ops,
            })
        })
        .await
    }
}

fn require_directory(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| StorageError::io("inspect layout", e))?;
    if !metadata.file_type().is_dir() {
        return Err(StorageError::InvalidInput(
            "storage layout must contain real directories".into(),
        ));
    }
    Ok(())
}

fn initialize(root: &Path, ops: &FsOps) -> Result<(), StorageError> {
    require_directory(root)?;
    let root = fs::canonicalize(root).map_err(|e| StorageError::io("resolve root", e))?;
    for name in ["staging", "completed"] {
        let path = root.join(name);
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(StorageError::io("create layout", e)),
        }
        require_directory(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let device = |p| {
                fs::metadata(p)
                    .map(|m| m.dev())
                    .map_err(|e| StorageError::io("inspect device", e))
            };
            if device(&path)? != device(&root)? {
                return Err(StorageError::InvalidInput(
                    "storage layout must share one filesystem".into(),
                ));
            }
        }
        ops.sync_dir(&path, Operation::LayoutSync)
            .map_err(|e| StorageError::io("sync layout", e))?;
    }
    ops.sync_dir(&root, Operation::LayoutSync)
        .map_err(|e| StorageError::io("sync root", e))?;
    if let Some(parent) = root.parent() {
        ops.sync_dir(parent, Operation::LayoutSync)
            .map_err(|e| StorageError::io("sync root parent", e))?;
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| StorageError::io("filesystem worker", io::Error::other(e)))?
}

#[cfg(all(test, unix))]
mod tests;
