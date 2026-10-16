#![cfg(unix)]
//! Backend-neutral cases: add adapters by constructing an isolated client and
//! passing it to these functions. No filesystem assumptions inside the cases.
use bytes::Bytes;
use file_storage::filesystem::{FilesystemConfig, FilesystemStorage};
use file_storage::{ByteStream, ExpectedIntegrity, FileStorage, ObjectRef, StorageError};
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn input(chunks: Vec<Bytes>) -> ByteStream {
    Box::pin(stream::iter(chunks.into_iter().map(Ok)))
}

async fn round_trip_contract(storage: Arc<dyn FileStorage>) {
    for chunks in [
        vec![],
        vec![Bytes::new()],
        vec![
            Bytes::from_static(b"abc"),
            Bytes::new(),
            Bytes::from(vec![42; 200_000]),
        ],
    ] {
        let all: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| chunk.iter().copied())
            .collect();
        let expected = ExpectedIntegrity {
            size_bytes: Some(all.len() as u64),
            sha256: Some(format!("{:x}", Sha256::digest(&all))),
        };
        let stored = storage
            .write(input(chunks), expected.clone())
            .await
            .unwrap();
        assert_eq!(stored.info.size_bytes, all.len() as u64);
        assert_eq!(Some(stored.info.sha256.clone()), expected.sha256);
        let encoded = serde_json::to_string(&stored.object_ref).unwrap();
        let reference: ObjectRef = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reference, stored.object_ref);
        assert_eq!(storage.inspect(&reference).await.unwrap(), stored.info);
        let mut read = storage.read(&reference, expected).await.unwrap();
        let mut actual = Vec::new();
        while let Some(chunk) = read.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= 64 * 1024);
            actual.extend_from_slice(&chunk);
        }
        assert_eq!(actual, all);
        // Another write never changes an existing completed object.
        storage
            .write(
                input(vec![Bytes::from_static(b"different")]),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        assert_eq!(storage.inspect(&reference).await.unwrap(), stored.info);
        storage.delete(&reference).await.unwrap();
        storage.delete(&reference).await.unwrap();
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.read(&reference, ExpectedIntegrity::default()).await,
            Err(StorageError::NotFound(_))
        ));
    }
}

async fn integrity_contract(storage: Arc<dyn FileStorage>) {
    for expected in [
        ExpectedIntegrity {
            size_bytes: Some(1),
            sha256: None,
        },
        ExpectedIntegrity {
            size_bytes: None,
            sha256: Some("0".repeat(64)),
        },
    ] {
        assert!(matches!(
            storage
                .write(input(vec![Bytes::from_static(b"abc")]), expected.clone())
                .await,
            Err(StorageError::IntegrityMismatch)
        ));
        let stored = storage
            .write(
                input(vec![Bytes::from_static(b"abc")]),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        let mut read = storage
            .read(&stored.object_ref, expected.clone())
            .await
            .unwrap();
        let mut actual = Vec::new();
        let mut mismatch = false;
        while let Some(item) = read.next().await {
            match item {
                Ok(chunk) => actual.extend_from_slice(&chunk),
                Err(error) => {
                    assert!(matches!(error, StorageError::IntegrityMismatch));
                    mismatch = true;
                    break;
                }
            }
        }
        assert!(mismatch, "integrity mismatch must precede successful EOF");
        assert!(b"abc".starts_with(&actual));
        assert!(read.next().await.is_none());
        // Dropping before the final poll makes no verification claim.
        let mut early = storage.read(&stored.object_ref, expected).await.unwrap();
        assert!(early.next().await.unwrap().is_ok());
        drop(early);
    }
    let invalid = ExpectedIntegrity {
        size_bytes: None,
        sha256: Some("A".repeat(64)),
    };
    assert!(matches!(
        storage.write(input(vec![]), invalid).await,
        Err(StorageError::InvalidInput(_))
    ));
    let stored = storage
        .write(input(vec![]), ExpectedIntegrity::default())
        .await
        .unwrap();
    let foreign = ObjectRef {
        store_id: "different-store".into(),
        ..stored.object_ref
    };
    assert!(matches!(
        storage.inspect(&foreign).await,
        Err(StorageError::InvalidReference(_))
    ));
}

async fn backpressure_contract(storage: Arc<dyn FileStorage>) {
    let polls = Arc::new(AtomicUsize::new(0));
    let count = polls.clone();
    let chunks = stream::iter((0..1024).map(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Bytes::from(vec![7; 1024]))
    }));
    let write = storage.write(Box::pin(chunks), ExpectedIntegrity::default());
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let stored = write.await.unwrap();
    assert_eq!(polls.load(Ordering::SeqCst), 1024);
    assert_eq!(stored.info.size_bytes, 1024 * 1024);
    // The returned read stream remains idle until polled, then yields bounded data.
    let mut read = storage
        .read(&stored.object_ref, ExpectedIntegrity::default())
        .await
        .unwrap();
    let first = read.next().await.unwrap().unwrap();
    assert!(first.len() <= 64 * 1024);
    drop(read);
}

#[cfg(unix)]
#[tokio::test]
async fn filesystem_contract() {
    let root = tempfile::tempdir().unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(
        FilesystemStorage::new(FilesystemConfig {
            root: root.path().into(),
            store_id: "contract".into(),
            max_object_bytes: 2_000_000,
        })
        .await
        .unwrap(),
    );
    round_trip_contract(storage.clone()).await;
    integrity_contract(storage.clone()).await;
    backpressure_contract(storage).await;
}

/// Preserve real storage/integrity behavior while changing only stream framing.
struct FragmentedReads(Arc<dyn FileStorage>);

#[async_trait::async_trait]
impl FileStorage for FragmentedReads {
    async fn write(
        &self,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<file_storage::StoredObject, StorageError> {
        self.0.write(input, expected).await
    }
    async fn read(
        &self,
        reference: &ObjectRef,
        expected: ExpectedIntegrity,
    ) -> Result<ByteStream, StorageError> {
        let stream = self.0.read(reference, expected).await?;
        Ok(Box::pin(stream.flat_map(|item| {
            let pieces: Vec<_> = match item {
                Ok(bytes) => (0..bytes.len())
                    .map(|index| Ok(bytes.slice(index..index + 1)))
                    .collect(),
                Err(error) => vec![Err(error)],
            };
            stream::iter(pieces)
        })))
    }
    async fn inspect(
        &self,
        reference: &ObjectRef,
    ) -> Result<file_storage::ObjectInfo, StorageError> {
        self.0.inspect(reference).await
    }
    async fn delete(&self, reference: &ObjectRef) -> Result<(), StorageError> {
        self.0.delete(reference).await
    }
}

#[tokio::test]
async fn integrity_contract_accepts_fragmented_reads() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(FilesystemConfig {
        root: root.path().into(),
        store_id: "fragmented".into(),
        max_object_bytes: 100,
    })
    .await
    .unwrap();
    integrity_contract(Arc::new(FragmentedReads(Arc::new(storage)))).await;
}
