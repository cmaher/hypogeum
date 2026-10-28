use super::{
    GoogleDriveStorage, error,
    paths::{self, Located, Resolved},
    write::response_error,
};
use crate::{
    ByteRange, ByteStream, ExpectedIntegrity, FileStorage, ObjectInfo, ObjectKey, ObjectRef,
    StorageError, StoredObject,
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

/// A failed download. `Stale` failures may come from an outdated cached ID or
/// size and are worth one fresh resolution.
enum Fetch {
    Stale(StorageError),
    Failed(StorageError),
}

#[async_trait]
impl FileStorage for GoogleDriveStorage {
    async fn write(
        &self,
        key: &ObjectKey,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError> {
        GoogleDriveStorage::write(self, key, input, expected).await
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
        let key = reference.key_in(&self.store_id)?;
        let mut fresh = false;
        loop {
            let Located { file, cached } = match self.locate(&key, fresh).await {
                Err(StorageError::NotFound(_)) => return Ok(()),
                located => located?,
            };
            let result = self
                .hub
                .files()
                .delete(&file.id)
                .supports_all_drives(true)
                .add_scope(self.scope.as_url())
                .doit()
                .await;
            self.forget_file(&key).await;
            match result {
                Ok(_) => return Ok(()),
                Err(google_drive3::Error::HttpError(_) | google_drive3::Error::Io(_)) => {
                    return Err(StorageError::UncertainCompletion {
                        operation: "delete Drive object",
                        source: io::Error::other("Drive deletion could not be confirmed"),
                    });
                }
                Err(e) => match error::api(e, "delete Drive object") {
                    // A stale cached ID: the key may now name another file.
                    StorageError::NotFound(_) if cached && !fresh => {
                        self.forget_path(&key).await;
                        fresh = true;
                    }
                    StorageError::NotFound(_) => return Ok(()),
                    error => return Err(error),
                },
            }
        }
    }
}

impl GoogleDriveStorage {
    async fn reader(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
        inspect: bool,
    ) -> Result<Reader, StorageError> {
        let key = reference.key_in(&self.store_id)?;
        let state = ReadState::new(options, self.max_object_bytes, inspect)?;
        let mut fresh = false;
        loop {
            let Located { file, cached } = self.locate(&key, fresh).await?;
            match self.body(&file, state.range()).await {
                Ok(body) => {
                    return Ok(Reader {
                        body,
                        pending: Bytes::new(),
                        state,
                    });
                }
                Err(Fetch::Stale(_)) if cached && !fresh => {
                    self.forget_path(&key).await;
                    fresh = true;
                }
                Err(Fetch::Stale(error)) => {
                    self.forget_file(&key).await;
                    return Err(error);
                }
                Err(Fetch::Failed(error)) => return Err(error),
            }
        }
    }

    async fn body(&self, file: &Resolved, range: Option<ByteRange>) -> Result<common::Body, Fetch> {
        match range {
            None => {
                let response = self
                    .hub
                    .files()
                    .get(&file.id)
                    .supports_all_drives(true)
                    .param("alt", "media")
                    .add_scope(self.scope.as_url())
                    .doit()
                    .await;
                match response {
                    Ok((response, _)) => Ok(response.into_body()),
                    Err(error) => {
                        let code = match &error {
                            google_drive3::Error::Failure(response) => {
                                Some(u64::from(response.status().as_u16()))
                            }
                            google_drive3::Error::BadRequest(value) => {
                                value.pointer("/error/code").and_then(|code| code.as_u64())
                            }
                            _ => None,
                        };
                        let error = error::api(error, "read Drive object");
                        Err(if matches!(code, Some(404 | 416)) {
                            Fetch::Stale(error)
                        } else {
                            Fetch::Failed(error)
                        })
                    }
                }
            }
            // ReadState rejects ranges whose end overflows.
            Some(range)
                if file
                    .size
                    .is_some_and(|size| size < range.offset + range.length) =>
            {
                Err(Fetch::Stale(beyond_end()))
            }
            Some(ByteRange { length: 0, .. }) => Ok(common::to_body::<Bytes>(None)),
            Some(range) => self.ranged_body(&file.id, range).await,
        }
    }

    /// Only a `206` carrying exactly the requested range is streamed: a server
    /// ignoring `Range` would otherwise send the whole object.
    async fn ranged_body(&self, id: &str, range: ByteRange) -> Result<common::Body, Fetch> {
        const OPERATION: &str = "read Drive object range";
        let end = range.offset + range.length - 1;
        // Resolved IDs are restricted to the Drive ID alphabet.
        debug_assert!(paths::valid_id(id));
        let request = Request::get(format!(
            "{}drive/v3/files/{id}?alt=media&supportsAllDrives=true",
            self.root_url
        ))
        .header(header::RANGE, format!("bytes={}-{end}", range.offset))
        .body(common::to_body::<Bytes>(None))
        .map_err(|_| Fetch::Failed(error::io(OPERATION)))?;
        let response = self
            .request(request, OPERATION)
            .await
            .map_err(Fetch::Failed)?;
        match response.status().as_u16() {
            206 => (),
            416 => return Err(Fetch::Stale(beyond_end())),
            404 => return Err(Fetch::Stale(response_error(response, OPERATION).await)),
            200 => return Err(Fetch::Failed(error::io(OPERATION))),
            _ => return Err(Fetch::Failed(response_error(response, OPERATION).await)),
        }
        let expected = format!("bytes {}-{end}/", range.offset);
        if !response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.starts_with(&expected))
        {
            return Err(Fetch::Failed(error::io(OPERATION)));
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
