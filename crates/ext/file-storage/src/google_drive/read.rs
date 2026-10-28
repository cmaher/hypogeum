use super::{FOLDER_MIME, GoogleDriveStorage, SHORTCUT_MIME, error, write::response_error};
use crate::{
    ByteRange, ByteStream, ExpectedIntegrity, FileStorage, ObjectInfo, ObjectRef, StorageError,
    StoredObject,
};
use crate::{
    ReadOptions,
    read_state::{ReadState, beyond_end},
};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use google_drive3::{
    common,
    hyper::{Request, header},
};
use http_body_util::BodyExt;
use std::io;

const READ_CHUNK_BYTES: usize = 64 * 1024;
/// Ancestor folders walked above an object before it is rejected.
const MAX_ANCESTOR_DEPTH: usize = 32;
const ANCESTRY_CACHE_LIMIT: usize = 4096;

fn outside() -> StorageError {
    StorageError::NotFound(io::Error::new(
        io::ErrorKind::NotFound,
        "object outside storage folder",
    ))
}

#[async_trait]
impl FileStorage for GoogleDriveStorage {
    async fn write(
        &self,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        GoogleDriveStorage::write(self, input, expected).await
    }

    async fn read(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
    ) -> Result<ByteStream, StorageError> {
        let reader = self.reader(reference, options, false).await?;
        Ok(Box::pin(stream::try_unfold(
            reader,
            |mut reader| async move { Ok(reader.next_chunk().await?.map(|chunk| (chunk, reader))) },
        )))
    }

    async fn inspect(&self, reference: &ObjectRef) -> Result<ObjectInfo, StorageError> {
        let mut reader = self.reader(reference, ReadOptions::default(), true).await?;
        while reader.next_chunk().await?.is_some() {}
        Ok(reader.state.info())
    }

    async fn delete(&self, reference: &ObjectRef) -> Result<(), StorageError> {
        let id = self.resolve(reference)?;
        match self.guard(id).await {
            Err(StorageError::NotFound(_)) => return Ok(()),
            result => {
                result?;
            }
        }
        match self
            .hub
            .files()
            .delete(id)
            .supports_all_drives(true)
            .add_scope(self.scope.as_url())
            .doit()
            .await
        {
            Ok(_) => Ok(()),
            Err(google_drive3::Error::HttpError(_) | google_drive3::Error::Io(_)) => {
                Err(StorageError::UncertainCompletion {
                    operation: "delete Drive object",
                    source: io::Error::other("Drive deletion could not be confirmed"),
                })
            }
            Err(e) => match error::api(e, "delete Drive object") {
                StorageError::NotFound(_) => Ok(()),
                error => Err(error),
            },
        }
    }
}

impl GoogleDriveStorage {
    /// Accept a non-trashed, non-folder, non-shortcut file anywhere below the
    /// storage folder. Drive items have a single parent: the walk checks every
    /// listed parent but follows only the first, at most `MAX_ANCESTOR_DEPTH`
    /// folders up. Folders proven to be descendants are cached. Returns the
    /// object's size when Drive reports one.
    async fn guard(&self, id: &str) -> Result<Option<u64>, StorageError> {
        let file = self.metadata(id).await?;
        if file.trashed == Some(true)
            || matches!(file.mime_type.as_deref(), Some(FOLDER_MIME | SHORTCUT_MIME))
        {
            return Err(outside());
        }
        let size = file.size.and_then(|size| u64::try_from(size).ok());
        let mut parents = file.parents.unwrap_or_default();
        let mut chain = Vec::new();
        loop {
            if self.contains_any(&parents) {
                self.remember(chain);
                return Ok(size);
            }
            let Some(parent) = parents.first().cloned() else {
                return Err(outside());
            };
            if chain.len() == MAX_ANCESTOR_DEPTH {
                return Err(outside());
            }
            let folder = match self.metadata(&parent).await {
                Err(StorageError::NotFound(_)) => return Err(outside()),
                result => result?,
            };
            if folder.trashed == Some(true) || folder.mime_type.as_deref() != Some(FOLDER_MIME) {
                return Err(outside());
            }
            chain.push(parent);
            parents = folder.parents.unwrap_or_default();
        }
    }

    async fn metadata(&self, id: &str) -> Result<google_drive3::api::File, StorageError> {
        let (_, file) = self
            .hub
            .files()
            .get(id)
            .supports_all_drives(true)
            .param("fields", "parents,trashed,mimeType,size")
            .add_scope(self.scope.as_url())
            .doit()
            .await
            .map_err(|e| error::api(e, "guard Drive object"))?;
        Ok(file)
    }

    fn contains_any(&self, parents: &[String]) -> bool {
        let descendants = self.descendants.lock().expect("ancestry cache lock");
        parents
            .iter()
            .any(|parent| *parent == self.folder_id || descendants.contains(parent))
    }

    fn remember(&self, folders: Vec<String>) {
        let mut descendants = self.descendants.lock().expect("ancestry cache lock");
        for folder in folders {
            if descendants.len() >= ANCESTRY_CACHE_LIMIT {
                descendants.clear();
            }
            descendants.insert(folder);
        }
    }

    async fn reader(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
        inspect: bool,
    ) -> Result<Reader, StorageError> {
        let id = self.resolve(reference)?;
        let state = ReadState::new(options, self.max_object_bytes, inspect)?;
        let size = self.guard(id).await?;
        let body = match state.range() {
            None => {
                let (response, _) = self
                    .hub
                    .files()
                    .get(id)
                    .supports_all_drives(true)
                    .param("alt", "media")
                    .add_scope(self.scope.as_url())
                    .doit()
                    .await
                    .map_err(|e| error::api(e, "read Drive object"))?;
                response.into_body()
            }
            // ReadState rejects ranges whose end overflows.
            Some(range) if size.is_some_and(|size| size < range.offset + range.length) => {
                return Err(beyond_end());
            }
            Some(ByteRange { length: 0, .. }) => common::to_body::<Bytes>(None),
            Some(range) => self.ranged_body(id, range).await?,
        };
        Ok(Reader {
            body,
            pending: Bytes::new(),
            state,
        })
    }

    /// Only a `206` carrying exactly the requested range is streamed: a server
    /// ignoring `Range` would otherwise send the whole object.
    async fn ranged_body(&self, id: &str, range: ByteRange) -> Result<common::Body, StorageError> {
        const OPERATION: &str = "read Drive object range";
        let end = range.offset + range.length - 1;
        let request = Request::get(format!(
            "{}drive/v3/files/{id}?alt=media&supportsAllDrives=true",
            self.root_url
        ))
        .header(header::RANGE, format!("bytes={}-{end}", range.offset))
        .body(common::to_body::<Bytes>(None))
        .map_err(|_| error::io(OPERATION))?;
        let response = self.request(request, OPERATION).await?;
        match response.status().as_u16() {
            206 => (),
            416 => return Err(beyond_end()),
            200 => return Err(error::io(OPERATION)),
            _ => return Err(response_error(response, OPERATION).await),
        }
        let expected = format!("bytes {}-{end}/", range.offset);
        if !response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.starts_with(&expected))
        {
            return Err(error::io(OPERATION));
        }
        Ok(response.into_body().boxed())
    }
}

struct Reader {
    body: common::Body,
    pending: Bytes,
    state: ReadState,
}
impl Reader {
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, StorageError> {
        while self.pending.is_empty() {
            match self.body.frame().await {
                None => {
                    self.state.finish()?;
                    return Ok(None);
                }
                Some(Err(_)) => return Err(error::io("read Drive body")),
                Some(Ok(frame)) => {
                    if let Ok(bytes) = frame.into_data() {
                        self.pending = bytes;
                    }
                }
            }
        }
        let bytes = self
            .pending
            .split_to(self.pending.len().min(READ_CHUNK_BYTES));
        self.state.update(&bytes)?;
        Ok(Some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_drive3::hyper::body::Frame;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn body_is_not_pulled_until_reader_is_polled() {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let frames = stream::iter([Ok::<_, google_drive3::hyper::Error>(Frame::data(
            Bytes::from(vec![7; READ_CHUNK_BYTES * 2]),
        ))]);
        use futures_util::StreamExt;
        let frames = frames.inspect(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let mut reader = Reader {
            body: common::Body::new(http_body_util::StreamBody::new(frames)),
            pending: Bytes::new(),
            state: ReadState::new(ReadOptions::default(), 1_000_000, false).unwrap(),
        };
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(
            reader.next_chunk().await.unwrap().unwrap().len(),
            READ_CHUNK_BYTES
        );
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reader.next_chunk().await.unwrap().unwrap().len(),
            READ_CHUNK_BYTES
        );
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert!(reader.next_chunk().await.unwrap().is_none());
    }
}
