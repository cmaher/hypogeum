use super::*;
use crate::{ByteStream, ObjectInfo, ObjectKey, ObjectRef, StoredObject};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Only ever removes the staging link. A blocking installer owns this guard once
/// installation starts, so cancellation cannot unlink a completed object.
struct Staging {
    path: PathBuf,
    ops: Arc<FsOps>,
    cleanup_on_drop: bool,
}

impl Staging {
    // Only called on a blocking worker. Failed unlink may leave staging debris.
    fn cleanup_blocking(&mut self) {
        if self.cleanup_on_drop {
            self.cleanup_on_drop = false;
            self.ops.cleanup(&self.path);
        }
    }

    async fn cleanup(mut self) {
        let _ = blocking(move || {
            self.cleanup_blocking();
            Ok(())
        })
        .await;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.cleanup_on_drop
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let path = self.path.clone();
            let ops = self.ops.clone();
            // Cancellation cannot wait for unlink. Never perform filesystem I/O
            // on the executor; shutdown without a runtime can leave debris.
            runtime.spawn_blocking(move || ops.cleanup(&path));
        }
    }
}

struct PreparedObject {
    file: fs::File,
    info: ObjectInfo,
}

impl FilesystemStorage {
    pub async fn write(
        &self,
        key: &ObjectKey,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        let destination = object_path(&self.root, key).ok_or_else(|| {
            StorageError::InvalidInput("object key is reserved on this filesystem".into())
        })?;
        expected.validate()?;
        if expected
            .size_bytes
            .is_some_and(|size| size > self.max_object_bytes)
        {
            return Err(StorageError::SizeLimit {
                limit: self.max_object_bytes,
            });
        }
        let path = self
            .root
            .join(STAGING)
            .join(uuid::Uuid::new_v4().to_string());
        let ops = self.ops.clone();
        let (file, mut staging) = blocking(move || {
            ops.check(Operation::Create)
                .map_err(|e| StorageError::io("create staging", e))?;
            let file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| StorageError::io("create staging", e))?;
            Ok((
                file,
                Staging {
                    path,
                    ops,
                    cleanup_on_drop: true,
                },
            ))
        })
        .await?;
        let prepared = match prepare(file, input, expected, self.max_object_bytes, &self.ops).await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                staging.cleanup().await;
                return Err(error);
            }
        };
        let root = self.root.clone();
        let object_ref = ObjectRef::new(self.store_id.clone(), key);
        blocking(move || {
            let result = install(prepared.file, &mut staging, &root, &destination);
            staging.cleanup_blocking();
            result?;
            Ok(StoredObject {
                object_ref,
                info: prepared.info,
            })
        })
        .await
    }
}

async fn prepare(
    file: fs::File,
    mut input: ByteStream,
    expected: ExpectedIntegrity,
    limit: u64,
    ops: &FsOps,
) -> Result<PreparedObject, StorageError> {
    let mut file = tokio::fs::File::from_std(file);
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    while let Some(chunk) = input.next().await {
        let chunk = chunk?;
        if chunk.is_empty() {
            // Empty ready chunks do not reach filesystem awaits. Cooperate
            // so a source of empty chunks cannot prevent cancellation.
            tokio::task::yield_now().await;
        }
        for bytes in chunk.chunks(CHUNK_BYTES) {
            size = size
                .checked_add(bytes.len() as u64)
                .filter(|size| *size <= limit)
                .ok_or(StorageError::SizeLimit { limit })?;
            ops.check(Operation::Write)
                .map_err(|e| StorageError::io("write staging", e))?;
            file.write_all(bytes)
                .await
                .map_err(|e| StorageError::io("write staging", e))?;
            hasher.update(bytes);
        }
    }
    let info = ObjectInfo {
        size_bytes: size,
        sha256: hex::encode(hasher.finalize()),
    };
    expected.verify(&info)?;
    file.flush()
        .await
        .map_err(|e| StorageError::io("flush staging", e))?;
    Ok(PreparedObject {
        file: file.into_std().await,
        info,
    })
}

fn install(
    file: fs::File,
    staging: &mut Staging,
    root: &Path,
    destination: &Path,
) -> Result<(), StorageError> {
    staging
        .ops
        .check(Operation::FileSync)
        .and_then(|()| file.sync_all())
        .map_err(|e| StorageError::io("sync object bytes", e))?;
    create_parents(root, destination, &staging.ops)?;
    match fs::symlink_metadata(destination) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(StorageError::InvalidInput(
                "existing object must be a regular file".into(),
            ));
        }
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(StorageError::io("inspect object", e)),
    }
    let installed = staging.ops.install(&staging.path, destination);
    if let Err(source) = installed {
        // On remote filesystems a failed reply can follow a successful rename
        // whose RPC was replayed. Fail closed.
        return Err(match source.kind() {
            io::ErrorKind::Unsupported => StorageError::io("install object", source),
            _ => StorageError::UncertainCompletion {
                operation: "install object",
                source,
            },
        });
    }
    // The staging link moved with the rename; there is nothing left to clean.
    staging.cleanup_on_drop = false;
    staging
        .ops
        .sync_dir(
            destination.parent().expect("object parent"),
            Operation::ParentSync,
        )
        .map_err(|source| StorageError::UncertainCompletion {
            operation: "sync installed object",
            source,
        })?;
    staging
        .ops
        .sync_dir(&root.join(STAGING), Operation::StagingSync)
        .map_err(|source| StorageError::UncertainCompletion {
            operation: "sync staging cleanup",
            source,
        })?;
    Ok(())
}

/// Create missing directories between the root and `destination`, syncing each
/// new entry. Existing segments must be real directories, never symlinks.
fn create_parents(root: &Path, destination: &Path, ops: &FsOps) -> Result<(), StorageError> {
    let not_directory =
        || StorageError::InvalidInput("object parents must be real directories".into());
    for parent in parents(root, destination) {
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.file_type().is_dir() => continue,
            Ok(_) => return Err(not_directory()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(StorageError::io("inspect object parent", e)),
        }
        match ops
            .check(Operation::CreateParent)
            .and_then(|()| fs::create_dir(parent))
        {
            Ok(()) => (),
            // A concurrent writer created it first.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                require_directory(parent).map_err(|_| not_directory())?;
                continue;
            }
            Err(e) => return Err(StorageError::io("create object parent", e)),
        }
        ops.sync_dir(
            parent.parent().expect("object parent below root"),
            Operation::CreateParentSync,
        )
        .map_err(|e| StorageError::io("sync object parent", e))?;
    }
    Ok(())
}
