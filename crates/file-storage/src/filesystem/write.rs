use super::*;
use crate::{ByteStream, ObjectInfo, ObjectRef, StoredObject};
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
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        expected.validate()?;
        if expected
            .size_bytes
            .is_some_and(|size| size > self.max_object_bytes)
        {
            return Err(StorageError::SizeLimit {
                limit: self.max_object_bytes,
            });
        }
        let key = self.ops.key();
        let path = self
            .root
            .join("staging")
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
        let object_ref = ObjectRef {
            version: 1,
            store_id: self.store_id.clone(),
            key,
        };
        blocking(move || {
            let result = install(prepared.file, &mut staging, &root, &object_ref.key);
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
        sha256: format!("{:x}", hasher.finalize()),
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
    key: &str,
) -> Result<(), StorageError> {
    staging
        .ops
        .check(Operation::FileSync)
        .and_then(|()| file.sync_all())
        .map_err(|e| StorageError::io("sync object bytes", e))?;
    let installed = staging
        .ops
        .install(&staging.path, &root.join("completed").join(key));
    if let Err(source) = installed {
        // On remote filesystems even AlreadyExists can follow a successful
        // link whose reply was lost and whose RPC was replayed. Fail closed.
        return Err(match source.kind() {
            io::ErrorKind::Unsupported => StorageError::io("install object", source),
            _ => StorageError::UncertainCompletion {
                operation: "install object",
                source,
            },
        });
    }
    staging
        .ops
        .sync_dir(&root.join("completed"), Operation::CompletedSync)
        .map_err(|source| StorageError::UncertainCompletion {
            operation: "sync installed object",
            source,
        })?;
    // Removal is best effort; the completed link is never removed on failure.
    staging.cleanup_blocking();
    staging
        .ops
        .sync_dir(&root.join("staging"), Operation::StagingSync)
        .map_err(|source| StorageError::UncertainCompletion {
            operation: "sync staging cleanup",
            source,
        })?;
    Ok(())
}
