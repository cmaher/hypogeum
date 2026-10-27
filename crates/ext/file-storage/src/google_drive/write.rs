use super::{GoogleDriveStorage, error};
use crate::{ByteStream, ExpectedIntegrity, ObjectInfo, ObjectRef, StorageError, StoredObject};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use google_drive3::{
    common,
    hyper::{self, Request, header},
};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use std::io;

const UPLOAD_CHUNK_BYTES: usize = 8 * 1024 * 1024;
type Response = hyper::Response<hyper::body::Incoming>;

impl GoogleDriveStorage {
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
        let metadata = serde_json::json!({
            "name": uuid::Uuid::new_v4().to_string(), "parents": [self.folder_id], "mimeType": "application/octet-stream"
        });
        let url = format!(
            "{}upload/drive/v3/files?uploadType=resumable&supportsAllDrives=true&fields=id,size,sha256Checksum",
            self.root_url
        );
        let request = Request::post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .header("X-Upload-Content-Type", "application/octet-stream")
            .body(common::to_body(Some(metadata.to_string())))
            .map_err(|_| error::io("start upload"))?;
        let response = self.request(request, "start upload").await?;
        if !response.status().is_success() {
            return Err(response_error(response, "start upload").await);
        }
        let session = response
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| error::io("start upload"))?
            .to_owned();
        // The bearer token is attached to every session request: only follow a
        // session URI on the configured Drive origin.
        if !session.starts_with(&self.root_url) {
            return Err(error::io("start upload"));
        }
        // Session URIs are bearer-like: keep them out of all error sources.
        let prepared = self.prepare(&session, input, expected).await;
        let (mut tail, mut offset, info) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.cancel(&session).await;
                return Err(error);
            }
        };
        // A transport failure can occur after successful headers, while reading
        // the final metadata body. Include that read in the one-query recovery.
        let final_result = loop {
            let range = if info.size_bytes == 0 {
                "bytes */0".into()
            } else {
                format!("bytes {offset}-{}/{}", info.size_bytes - 1, info.size_bytes)
            };
            match self.put(&session, &range, tail.clone()).await {
                Ok(response) if response.status().is_success() => {
                    break response_bytes(response).await;
                }
                Ok(response) if response.status().is_server_error() => {
                    break Err(error::io("complete upload"));
                }
                // Drive may persist only part of the final chunk. Resend the
                // unacknowledged, still nonempty, tail.
                Ok(response) if response.status().as_u16() == 308 && info.size_bytes > 0 => {
                    match acknowledged(&response, offset, info.size_bytes - 1) {
                        Ok(acked) => {
                            tail = tail.slice((acked - offset) as usize..);
                            offset = acked;
                        }
                        Err(error) => {
                            self.cancel(&session).await;
                            return Err(error);
                        }
                    }
                }
                Ok(response) => {
                    let error = response_error(response, "complete upload").await;
                    self.cancel(&session).await;
                    return Err(error);
                }
                Err(error) => break Err(error),
            }
        };
        let bytes = match final_result {
            Ok(bytes) => bytes,
            Err(StorageError::Io { .. }) => {
                let response = self.settle(&session, info.size_bytes).await?;
                response_bytes(response)
                    .await
                    .map_err(|_| uncertain("decode completed upload"))?
            }
            Err(error) => {
                self.cancel(&session).await;
                return Err(error);
            }
        };
        let file: google_drive3::api::File =
            serde_json::from_slice(&bytes).map_err(|_| uncertain("decode completed upload"))?;
        let key = file
            .id
            .ok_or_else(|| uncertain("identify completed upload"))?;
        let reference = ObjectRef {
            version: 1,
            store_id: self.store_id.clone(),
            key,
        };
        self.resolve(&reference)
            .map_err(|_| uncertain("identify completed upload"))?;
        if file.size.and_then(|s| u64::try_from(s).ok()) != Some(info.size_bytes)
            || file
                .sha256_checksum
                .as_ref()
                .is_some_and(|hash| hash != &info.sha256)
        {
            // Only this upload's newly returned file ID is eligible for cleanup.
            // An unconfirmed cleanup may leave an unreferenced object behind.
            return match self
                .hub
                .files()
                .delete(&reference.key)
                .supports_all_drives(true)
                .add_scope(self.scope.as_url())
                .doit()
                .await
            {
                Ok(_) => Err(StorageError::IntegrityMismatch),
                Err(e) => match error::api(e, "delete mismatched upload") {
                    StorageError::NotFound(_) => Err(StorageError::IntegrityMismatch),
                    _ => Err(uncertain("delete mismatched upload")),
                },
            };
        }
        Ok(StoredObject {
            object_ref: reference,
            info,
        })
    }

    async fn prepare(
        &self,
        session: &str,
        mut input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<(Bytes, u64, ObjectInfo), StorageError> {
        let mut buffer = BytesMut::with_capacity(UPLOAD_CHUNK_BYTES + 1);
        let mut size = 0u64;
        let mut hash = Sha256::new();
        let mut offset = 0u64;
        while let Some(item) = input.next().await {
            let mut bytes = item?;
            while !bytes.is_empty() {
                let count = bytes.len().min(UPLOAD_CHUNK_BYTES + 1 - buffer.len());
                size = size
                    .checked_add(count as u64)
                    .filter(|size| *size <= self.max_object_bytes)
                    .ok_or(StorageError::SizeLimit {
                        limit: self.max_object_bytes,
                    })?;
                if expected.size_bytes.is_some_and(|expected| size > expected) {
                    return Err(StorageError::IntegrityMismatch);
                }
                let part = bytes.split_to(count);
                hash.update(&part);
                buffer.extend_from_slice(&part);
                if buffer.len() > UPLOAD_CHUNK_BYTES {
                    let chunk = buffer.split_to(UPLOAD_CHUNK_BYTES).freeze();
                    self.chunk(session, chunk, &mut offset).await?;
                }
            }
            // A producer yielding ready empty chunks must not starve cancellation.
            tokio::task::yield_now().await;
        }
        let info = ObjectInfo {
            size_bytes: size,
            sha256: hex::encode(hash.finalize()),
        };
        expected.verify(&info)?;
        Ok((buffer.freeze(), offset, info))
    }

    async fn chunk(
        &self,
        session: &str,
        mut bytes: Bytes,
        offset: &mut u64,
    ) -> Result<(), StorageError> {
        let end = *offset + bytes.len() as u64 - 1;
        while !bytes.is_empty() {
            let range = format!("bytes {}-{end}/*", *offset);
            let response = self.put(session, &range, bytes.clone()).await?;
            if response.status().as_u16() != 308 {
                return Err(error::io("upload chunk"));
            }
            let acknowledged = acknowledged(&response, *offset, end + 1)?;
            let count = (acknowledged - *offset) as usize;
            bytes = bytes.slice(count..);
            *offset = acknowledged;
        }
        Ok(())
    }

    async fn put(
        &self,
        session: &str,
        range: &str,
        bytes: Bytes,
    ) -> Result<Response, StorageError> {
        let request = Request::put(session)
            .header(header::CONTENT_RANGE, range)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(common::to_body(Some(bytes)))
            .map_err(|_| error::io("upload bytes"))?;
        self.request(request, "upload bytes").await
    }

    async fn request(
        &self,
        mut request: Request<common::Body>,
        operation: &'static str,
    ) -> Result<Response, StorageError> {
        let token = self
            .hub
            .auth
            .get_token(&[self.scope.as_url()])
            .await
            .map_err(|_| error::permission())?
            .ok_or_else(error::permission)?;
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .map_err(|_| error::permission())?,
        );
        self.hub
            .client
            .request(request)
            .await
            .map_err(|_| error::io(operation))
    }

    async fn cancel(&self, session: &str) {
        if let Ok(request) = Request::delete(session).body(common::to_body::<Bytes>(None)) {
            let _ = self.request(request, "cancel upload").await;
        }
    }

    async fn settle(&self, session: &str, total: u64) -> Result<Response, StorageError> {
        match self
            .put(session, &format!("bytes */{total}"), Bytes::new())
            .await
        {
            Ok(response) if response.status().is_success() => Ok(response),
            Ok(response) if response.status().as_u16() == 308 => {
                self.cancel(session).await;
                Err(error::io("complete upload"))
            }
            _ => Err(uncertain("complete upload")),
        }
    }
}

/// Bytes Drive persisted after a 308, which must advance past `offset` and not
/// exceed `limit`.
fn acknowledged(response: &Response, offset: u64, limit: u64) -> Result<u64, StorageError> {
    response
        .headers()
        .get(header::RANGE)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("bytes=0-"))
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|last| last.checked_add(1))
        .filter(|acked| *acked > offset && *acked <= limit)
        .ok_or_else(|| error::io("acknowledge upload chunk"))
}

fn uncertain(operation: &'static str) -> StorageError {
    StorageError::UncertainCompletion {
        operation,
        source: io::Error::other("Drive completion could not be confirmed"),
    }
}

async fn response_bytes(response: Response) -> Result<Bytes, StorageError> {
    http_body_util::Limited::new(response.into_body(), 1024 * 1024)
        .collect()
        .await
        .map(|body| body.to_bytes())
        .map_err(|_| error::io("read Drive response"))
}

async fn response_error(response: Response, operation: &'static str) -> StorageError {
    let status = response.status().as_u16();
    match response_bytes(response)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(value) => error::json_status(status, &value, operation),
        None => error::status(status, None, operation),
    }
}
