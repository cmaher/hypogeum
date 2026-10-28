use super::*;
use crate::{ByteStream, FileStorage, ObjectInfo, ObjectKey, ObjectRef, StoredObject};
use crate::{
    ReadOptions,
    read_state::{ReadState, beyond_end},
};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use tokio::io::AsyncReadExt;

#[async_trait]
impl FileStorage for FilesystemStorage {
    async fn write(
        &self,
        key: &ObjectKey,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        FilesystemStorage::write(self, key, input, expected).await
    }

    async fn read(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
    ) -> Result<ByteStream, StorageError> {
        let reader = self.reader(reference, options, false).await?;
        Ok(Box::pin(stream::try_unfold(
            reader,
            |mut reader| async move { Ok(reader.next_chunk().await?.map(|bytes| (bytes, reader))) },
        )))
    }

    async fn inspect(&self, reference: &ObjectRef) -> Result<ObjectInfo, StorageError> {
        let mut reader = self.reader(reference, ReadOptions::default(), true).await?;
        while reader.next_chunk().await?.is_some() {}
        Ok(reader.state.info())
    }

    async fn delete(&self, reference: &ObjectRef) -> Result<(), StorageError> {
        let path = self.resolve(reference)?;
        let ops = self.ops.clone();
        let root = self.root.clone();
        blocking(move || {
            match require_parents(&root, &path) {
                // No parent directory, so no entry to remove or settle.
                Err(StorageError::NotFound(_)) => return Ok(()),
                result => result?,
            }
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
            ops.sync_dir(path.parent().expect("object parent"), Operation::ParentSync)
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
        let key = reference.key_in(&self.store_id)?;
        object_path(&self.root, &key)
            .ok_or_else(|| StorageError::InvalidReference("reserved object key".into()))
    }

    async fn reader(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
        inspect: bool,
    ) -> Result<Reader, StorageError> {
        let state = ReadState::new(options, self.max_object_bytes, inspect)?;
        let path = self.resolve(reference)?;
        let root = self.root.clone();
        let range = state.range();
        let file = blocking(move || {
            require_parents(&root, &path)?;
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
            let metadata = file
                .metadata()
                .map_err(|e| StorageError::io("inspect open object", e))?;
            if !metadata.is_file() {
                return Err(StorageError::InvalidReference(
                    "object must be a regular file".into(),
                ));
            }
            if let Some(range) = range {
                // ReadState rejects ranges whose end overflows.
                if metadata.len() < range.offset + range.length {
                    return Err(beyond_end());
                }
                use std::io::Seek;
                (&file)
                    .seek(io::SeekFrom::Start(range.offset))
                    .map_err(|e| StorageError::io("seek object", e))?;
            }
            Ok(file)
        })
        .await?;
        Ok(Reader {
            file: tokio::fs::File::from_std(file),
            state,
        })
    }
}

/// The layout is trusted against concurrent mutation, but never follow a
/// symlinked key segment. A segment that is a file means no such object.
fn require_parents(root: &Path, path: &Path) -> Result<(), StorageError> {
    for parent in parents(root, path) {
        let metadata = fs::symlink_metadata(parent)
            .map_err(|e| StorageError::io("inspect object parent", e))?;
        if metadata.file_type().is_symlink() {
            return Err(StorageError::InvalidReference(
                "object parents must be real directories".into(),
            ));
        }
        if !metadata.file_type().is_dir() {
            return Err(StorageError::NotFound(io::Error::new(
                io::ErrorKind::NotFound,
                "object parent is not a directory",
            )));
        }
    }
    Ok(())
}

fn regular_metadata(path: &Path) -> Result<(), StorageError> {
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
    state: ReadState,
}

impl Reader {
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, StorageError> {
        let wanted = self.state.remaining().map_or(CHUNK_BYTES, |remaining| {
            remaining.min(CHUNK_BYTES as u64) as usize
        });
        if wanted == 0 {
            self.state.finish()?;
            return Ok(None);
        }
        let mut buffer = vec![0; wanted];
        let count = self
            .file
            .read(&mut buffer)
            .await
            .map_err(|e| StorageError::io("read object", e))?;
        if count == 0 {
            self.state.finish()?;
            return Ok(None);
        }
        self.state.update(&buffer[..count])?;
        buffer.truncate(count);
        Ok(Some(Bytes::from(buffer)))
    }
}
