use super::*;
use crate::{ByteStream, FileStorage, ObjectInfo, ObjectRef, StoredObject};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[async_trait]
impl FileStorage for FilesystemStorage {
    async fn write(
        &self,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        FilesystemStorage::write(self, input, expected).await
    }

    async fn read(
        &self,
        reference: &ObjectRef,
        expected: ExpectedIntegrity,
    ) -> Result<ByteStream, StorageError> {
        let reader = self.reader(reference, expected).await?;
        Ok(Box::pin(stream::try_unfold(
            reader,
            |mut reader| async move { Ok(reader.next_chunk().await?.map(|bytes| (bytes, reader))) },
        )))
    }

    async fn inspect(&self, reference: &ObjectRef) -> Result<ObjectInfo, StorageError> {
        let mut reader = self.reader(reference, ExpectedIntegrity::default()).await?;
        while reader.next_chunk().await?.is_some() {}
        Ok(reader.info())
    }

    async fn delete(&self, reference: &ObjectRef) -> Result<(), StorageError> {
        let path = self.resolve(reference)?;
        let ops = self.ops.clone();
        let completed = self.root.join("completed");
        blocking(move || {
            match regular_metadata(&path) {
                Ok(()) => {
                    ops.check(Operation::Delete)
                        .map_err(|e| StorageError::io("delete object", e))?;
                    match fs::remove_file(&path) {
                        Ok(()) => (),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                        Err(source) => {
                            return Err(StorageError::UncertainCompletion {
                                operation: "delete object",
                                source,
                            });
                        }
                    }
                }
                Err(StorageError::NotFound(_)) => (),
                Err(e) => return Err(e),
            }
            // Also sync when already missing: retry can settle a previous uncertain unlink.
            ops.sync_dir(&completed, Operation::CompletedSync)
                .map_err(|source| StorageError::UncertainCompletion {
                    operation: "sync deletion",
                    source,
                })
        })
        .await
    }
}

impl FilesystemStorage {
    fn resolve(&self, reference: &ObjectRef) -> Result<PathBuf, StorageError> {
        let valid_key = uuid::Uuid::parse_str(&reference.key)
            .is_ok_and(|uuid| uuid.to_string() == reference.key && uuid.get_version_num() == 4);
        if reference.version != 1 || reference.store_id != self.store_id || !valid_key {
            return Err(StorageError::InvalidReference(
                "unknown version, store or malformed key".into(),
            ));
        }
        Ok(self.root.join("completed").join(&reference.key))
    }

    async fn reader(
        &self,
        reference: &ObjectRef,
        expected: ExpectedIntegrity,
    ) -> Result<Reader, StorageError> {
        expected.validate()?;
        let path = self.resolve(reference)?;
        let file = blocking(move || {
            regular_metadata(&path)?;
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = options
                .open(&path)
                .map_err(|e| StorageError::io("open object", e))?;
            if !file
                .metadata()
                .map_err(|e| StorageError::io("inspect open object", e))?
                .is_file()
            {
                return Err(StorageError::InvalidReference(
                    "object must be a regular file".into(),
                ));
            }
            Ok(file)
        })
        .await?;
        Ok(Reader {
            file: tokio::fs::File::from_std(file),
            size: 0,
            hash: Sha256::new(),
            expected,
            limit: self.max_object_bytes,
        })
    }
}

fn regular_metadata(path: &Path) -> Result<(), StorageError> {
    // The layout is trusted against concurrent mutation, but never follow an
    // existing replacement of the completed directory or an object symlink.
    require_directory(path.parent().expect("completed object parent"))?;
    let metadata = fs::symlink_metadata(path).map_err(|e| StorageError::io("inspect object", e))?;
    if !metadata.file_type().is_file() {
        return Err(StorageError::InvalidReference(
            "object must be a regular file".into(),
        ));
    }
    Ok(())
}

struct Reader {
    file: tokio::fs::File,
    size: u64,
    hash: Sha256,
    expected: ExpectedIntegrity,
    limit: u64,
}

impl Reader {
    fn info(&self) -> ObjectInfo {
        ObjectInfo {
            size_bytes: self.size,
            sha256: hex::encode(self.hash.clone().finalize()),
        }
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, StorageError> {
        let mut buffer = vec![0; CHUNK_BYTES];
        let count = self
            .file
            .read(&mut buffer)
            .await
            .map_err(|e| StorageError::io("read object", e))?;
        if count == 0 {
            self.expected.verify(&self.info())?;
            return Ok(None);
        }
        self.size = self
            .size
            .checked_add(count as u64)
            .filter(|size| *size <= self.limit)
            .ok_or(StorageError::SizeLimit { limit: self.limit })?;
        self.hash.update(&buffer[..count]);
        buffer.truncate(count);
        Ok(Some(Bytes::from(buffer)))
    }
}
