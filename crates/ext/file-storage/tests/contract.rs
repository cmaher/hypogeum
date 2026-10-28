#![cfg(unix)]
//! Backend-neutral cases: add adapters by constructing an isolated client and
//! passing it to these functions. No filesystem assumptions inside the cases.
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use hypgmx_file_storage::filesystem::{FilesystemConfig, FilesystemStorage};
use hypgmx_file_storage::{
    ByteRange, ByteStream, ExpectedIntegrity, FileStorage, ObjectKey, ObjectRef, ReadOptions,
    StorageError,
};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn input(chunks: Vec<Bytes>) -> ByteStream {
    Box::pin(stream::iter(chunks.into_iter().map(Ok)))
}

/// Cases write below `prefix`, so a shared live store stays partitioned.
fn key(prefix: &str, path: &str) -> ObjectKey {
    ObjectKey::new(format!("{prefix}/{path}")).unwrap()
}

async fn read_all(
    storage: &dyn FileStorage,
    reference: &ObjectRef,
    options: ReadOptions,
) -> Vec<u8> {
    let mut read = storage.read(reference, options).await.unwrap();
    let mut actual = Vec::new();
    while let Some(chunk) = read.next().await {
        actual.extend_from_slice(&chunk.unwrap());
    }
    actual
}

async fn round_trip_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    for (case, chunks) in [
        vec![],
        vec![Bytes::new()],
        vec![
            Bytes::from_static(b"abc"),
            Bytes::new(),
            Bytes::from(vec![42; 200_000]),
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let object = key(prefix, &format!("round-trip/{case}"));
        let all: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| chunk.iter().copied())
            .collect();
        let expected = ExpectedIntegrity {
            size_bytes: Some(all.len() as u64),
            sha256: Some(hex::encode(Sha256::digest(&all))),
        };
        let stored = storage
            .write(&object, input(chunks), expected.clone())
            .await
            .unwrap();
        assert_eq!(stored.object_ref.key, object.as_str());
        assert_eq!(stored.info.size_bytes, all.len() as u64);
        assert_eq!(Some(stored.info.sha256.clone()), expected.sha256);
        let encoded = serde_json::to_string(&stored.object_ref).unwrap();
        let reference: ObjectRef = serde_json::from_str(&encoded).unwrap();
        assert_eq!(reference, stored.object_ref);
        assert_eq!(storage.inspect(&reference).await.unwrap(), stored.info);
        let mut read = storage
            .read(
                &reference,
                ReadOptions {
                    expected,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut actual = Vec::new();
        while let Some(chunk) = read.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= 64 * 1024);
            actual.extend_from_slice(&chunk);
        }
        assert_eq!(actual, all);
        // A write to another key never changes this object.
        storage
            .write(
                &key(prefix, &format!("round-trip/{case}-sibling")),
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
            storage.read(&reference, ReadOptions::default()).await,
            Err(StorageError::NotFound(_))
        ));
    }
}

async fn integrity_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    for (case, expected) in [
        ExpectedIntegrity {
            size_bytes: Some(1),
            sha256: None,
        },
        ExpectedIntegrity {
            size_bytes: None,
            sha256: Some("0".repeat(64)),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let object = key(prefix, &format!("integrity/{case}"));
        assert!(matches!(
            storage
                .write(
                    &object,
                    input(vec![Bytes::from_static(b"abc")]),
                    expected.clone()
                )
                .await,
            Err(StorageError::IntegrityMismatch)
        ));
        let stored = storage
            .write(
                &object,
                input(vec![Bytes::from_static(b"abc")]),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        let mut read = storage
            .read(
                &stored.object_ref,
                ReadOptions {
                    expected: expected.clone(),
                    ..Default::default()
                },
            )
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
        let mut early = storage
            .read(
                &stored.object_ref,
                ReadOptions {
                    expected,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(early.next().await.unwrap().is_ok());
        drop(early);
    }
    let invalid = ExpectedIntegrity {
        size_bytes: None,
        sha256: Some("A".repeat(64)),
    };
    assert!(matches!(
        storage
            .write(&key(prefix, "integrity/invalid"), input(vec![]), invalid)
            .await,
        Err(StorageError::InvalidInput(_))
    ));
    let stored = storage
        .write(
            &key(prefix, "integrity/empty"),
            input(vec![]),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    let foreign = ObjectRef {
        store_id: "different-store".into(),
        ..stored.object_ref.clone()
    };
    assert!(matches!(
        storage.inspect(&foreign).await,
        Err(StorageError::InvalidReference(_))
    ));
    let first_version = ObjectRef {
        version: 1,
        ..stored.object_ref
    };
    assert!(matches!(
        storage.inspect(&first_version).await,
        Err(StorageError::InvalidReference(_))
    ));
}

// Run the same options cases against both real adapters.
async fn read_options_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    for data in [b"".as_slice(), b"abc"] {
        let stored = storage
            .write(
                &key(prefix, &format!("read-options/{}", data.len())),
                input(vec![Bytes::copy_from_slice(data)]),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        for options in [
            ReadOptions::default(),
            ReadOptions {
                max_bytes: Some(data.len() as u64),
                ..Default::default()
            },
            ReadOptions {
                expected: ExpectedIntegrity {
                    size_bytes: Some(data.len() as u64),
                    sha256: None,
                },
                ..Default::default()
            },
            ReadOptions {
                expected: ExpectedIntegrity {
                    size_bytes: None,
                    sha256: Some(stored.info.sha256.clone()),
                },
                ..Default::default()
            },
        ] {
            let mut read = storage.read(&stored.object_ref, options).await.unwrap();
            let mut actual = Vec::new();
            while let Some(chunk) = read.next().await {
                actual.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(actual, data);
        }
        if !data.is_empty() {
            for limit in [0, data.len() as u64 - 1] {
                let mut read = storage
                    .read(
                        &stored.object_ref,
                        ReadOptions {
                            max_bytes: Some(limit),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                assert!(
                    matches!(read.next().await, Some(Err(StorageError::SizeLimit { limit: actual })) if actual == limit)
                );
                assert!(read.next().await.is_none());
            }
        }
        let invalid = ReadOptions {
            expected: ExpectedIntegrity {
                size_bytes: None,
                sha256: Some("invalid".into()),
            },
            ..Default::default()
        };
        assert!(matches!(
            storage.read(&stored.object_ref, invalid).await,
            Err(StorageError::InvalidInput(_))
        ));
    }
}

async fn range_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let len = data.len() as u64;
    let stored = storage
        .write(
            &key(prefix, "range/object"),
            input(vec![Bytes::from(data.clone())]),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    let ranged = |offset, length| ReadOptions {
        range: Some(ByteRange { offset, length }),
        ..Default::default()
    };
    for (offset, length) in [(0, 10), (1_000, 150_000), (len - 7, 7), (5, 0), (len, 0)] {
        let mut read = storage
            .read(&stored.object_ref, ranged(offset, length))
            .await
            .unwrap();
        let mut actual = Vec::new();
        while let Some(chunk) = read.next().await {
            let chunk = chunk.unwrap();
            assert!(chunk.len() <= 64 * 1024);
            actual.extend_from_slice(&chunk);
        }
        let (start, end) = (offset as usize, (offset + length) as usize);
        assert_eq!(actual, data[start..end]);
    }
    for (offset, length) in [(len - 7, 8), (len + 1, 0), (len, 1), (u64::MAX, 2)] {
        assert!(matches!(
            storage
                .read(&stored.object_ref, ranged(offset, length))
                .await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    for expected in [
        ExpectedIntegrity {
            size_bytes: Some(10),
            sha256: None,
        },
        ExpectedIntegrity {
            size_bytes: None,
            sha256: Some(stored.info.sha256.clone()),
        },
    ] {
        let options = ReadOptions {
            expected,
            ..ranged(0, 10)
        };
        assert!(matches!(
            storage.read(&stored.object_ref, options).await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    let limited = ReadOptions {
        max_bytes: Some(9),
        ..ranged(0, 10)
    };
    assert!(matches!(
        storage.read(&stored.object_ref, limited).await,
        Err(StorageError::SizeLimit { limit: 9 })
    ));
}

async fn backpressure_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    let polls = Arc::new(AtomicUsize::new(0));
    let count = polls.clone();
    let chunks = stream::iter((0..1024).map(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Bytes::from(vec![7; 1024]))
    }));
    let object = key(prefix, "backpressure");
    let write = storage.write(&object, Box::pin(chunks), ExpectedIntegrity::default());
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let stored = write.await.unwrap();
    assert_eq!(polls.load(Ordering::SeqCst), 1024);
    assert_eq!(stored.info.size_bytes, 1024 * 1024);
    // The returned read stream remains idle until polled, then yields bounded data.
    let mut read = storage
        .read(&stored.object_ref, ReadOptions::default())
        .await
        .unwrap();
    let first = read.next().await.unwrap().unwrap();
    assert!(first.len() <= 64 * 1024);
    drop(read);
}

/// Nested keys: write, read, ranged read, derived siblings, replace and delete.
async fn path_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    let original = key(prefix, "paths/a/b/c.bin");
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 241) as u8).collect();
    let stored = storage
        .write(
            &original,
            input(vec![Bytes::from(data.clone())]),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(stored.object_ref.version, ObjectRef::VERSION);
    assert_eq!(stored.object_ref.key, original.as_str());
    let reference = stored.object_ref.clone();
    assert_eq!(
        read_all(storage.as_ref(), &reference, ReadOptions::default()).await,
        data
    );
    let ranged = ReadOptions {
        range: Some(ByteRange {
            offset: 70_000,
            length: 20_000,
        }),
        ..Default::default()
    };
    assert_eq!(
        read_all(storage.as_ref(), &reference, ranged.clone()).await,
        data[70_000..90_000]
    );
    // A derived object sits next to its source.
    let derived = storage
        .write(
            &key(prefix, "paths/a/b/c.thumb.bin"),
            input(vec![Bytes::from_static(b"thumb")]),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    // Replacement swaps the whole object at the same key.
    let replacement: Vec<u8> = data.iter().rev().copied().collect();
    let replaced = storage
        .write(
            &original,
            input(vec![Bytes::from(replacement.clone())]),
            ExpectedIntegrity {
                size_bytes: Some(replacement.len() as u64),
                sha256: Some(hex::encode(Sha256::digest(&replacement))),
            },
        )
        .await
        .unwrap();
    assert_eq!(replaced.object_ref, reference);
    assert_eq!(storage.inspect(&reference).await.unwrap(), replaced.info);
    assert_eq!(
        read_all(storage.as_ref(), &reference, ranged).await,
        replacement[70_000..90_000]
    );
    // A failed replacement leaves the current object in place.
    assert!(matches!(
        storage
            .write(
                &original,
                input(vec![Bytes::from_static(b"short")]),
                ExpectedIntegrity {
                    size_bytes: Some(6),
                    sha256: None,
                },
            )
            .await,
        Err(StorageError::IntegrityMismatch)
    ));
    assert_eq!(storage.inspect(&reference).await.unwrap(), replaced.info);
    assert_eq!(
        read_all(
            storage.as_ref(),
            &derived.object_ref,
            ReadOptions::default()
        )
        .await,
        b"thumb"
    );
    for object in [&reference, &derived.object_ref] {
        storage.delete(object).await.unwrap();
        storage.delete(object).await.unwrap();
        assert!(matches!(
            storage.inspect(object).await,
            Err(StorageError::NotFound(_))
        ));
    }
    let missing = ObjectRef::new(
        reference.store_id.clone(),
        &key(prefix, "paths/missing/parent/x"),
    );
    assert!(matches!(
        storage.inspect(&missing).await,
        Err(StorageError::NotFound(_))
    ));
    storage.delete(&missing).await.unwrap();
}

async fn full_contract(storage: Arc<dyn FileStorage>, prefix: &str) {
    round_trip_contract(storage.clone(), prefix).await;
    integrity_contract(storage.clone(), prefix).await;
    read_options_contract(storage.clone(), prefix).await;
    range_contract(storage.clone(), prefix).await;
    backpressure_contract(storage.clone(), prefix).await;
    path_contract(storage, prefix).await;
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
    full_contract(storage, "contract").await;
}

/// Preserve real storage/integrity behavior while changing only stream framing.
struct FragmentedReads(Arc<dyn FileStorage>);

#[async_trait::async_trait]
impl FileStorage for FragmentedReads {
    async fn write(
        &self,
        key: &ObjectKey,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<hypgmx_file_storage::StoredObject, StorageError> {
        self.0.write(key, input, expected).await
    }
    async fn read(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
    ) -> Result<ByteStream, StorageError> {
        let stream = self.0.read(reference, options).await?;
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
    ) -> Result<hypgmx_file_storage::ObjectInfo, StorageError> {
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
    integrity_contract(Arc::new(FragmentedReads(Arc::new(storage))), "fragmented").await;
}

#[cfg(feature = "google-drive")]
mod support;

#[cfg(feature = "google-drive")]
fn drive_config(folder_id: String) -> hypgmx_file_storage::google_drive::GoogleDriveConfig {
    use hypgmx_file_storage::google_drive::{DriveScope, GoogleDriveConfig};
    GoogleDriveConfig {
        store_id: "contract".into(),
        folder_id,
        client_id: "client".into(),
        client_secret: "secret".into(),
        refresh_token: "refresh".into(),
        scope: DriveScope::Full,
        max_object_bytes: 2_000_000,
        path_cache: Default::default(),
    }
}

#[cfg(feature = "google-drive")]
#[tokio::test]
async fn google_drive_contract() {
    let fake = support::fake_drive::FakeDrive::start().await;
    let storage = Arc::new(
        hypgmx_file_storage::google_drive::GoogleDriveStorage::with_test_endpoint(
            drive_config("folder".into()),
            &fake.root_url,
            "test-token",
        )
        .await
        .unwrap(),
    );
    full_contract(storage, "contract").await;
}

#[cfg(feature = "google-drive")]
#[tokio::test]
async fn google_drive_fragmented_reads() {
    let fake = support::fake_drive::FakeDrive::start().await;
    let storage = hypgmx_file_storage::google_drive::GoogleDriveStorage::with_test_endpoint(
        drive_config("folder".into()),
        &fake.root_url,
        "test-token",
    )
    .await
    .unwrap();
    integrity_contract(Arc::new(FragmentedReads(Arc::new(storage))), "fragmented").await;
}

/// Smoke test of nested keys (write, read, ranged read, replace, delete) and the
/// full contract against real Drive.
/// Requires HYPOGEUM_GDRIVE_TEST_{CLIENT_ID,CLIENT_SECRET,REFRESH_TOKEN,FOLDER_ID}.
/// Run: `cargo test -p hypgmx-file-storage --features google-drive -- --ignored google_drive_live`
#[cfg(feature = "google-drive")]
#[tokio::test]
#[ignore = "requires a dedicated live Drive folder and OAuth credentials"]
async fn google_drive_live_contract() {
    let get = |key| {
        std::env::var(format!("HYPOGEUM_GDRIVE_TEST_{key}"))
            .unwrap_or_else(|_| panic!("missing HYPOGEUM_GDRIVE_TEST_{key}"))
    };
    let config = hypgmx_file_storage::google_drive::GoogleDriveConfig {
        client_id: get("CLIENT_ID"),
        client_secret: get("CLIENT_SECRET"),
        refresh_token: get("REFRESH_TOKEN"),
        ..drive_config(get("FOLDER_ID"))
    };
    let storage = Arc::new(
        hypgmx_file_storage::google_drive::GoogleDriveStorage::new(config)
            .await
            .unwrap(),
    );
    // Objects are deleted; the per-run folders are left for the operator.
    full_contract(storage, &format!("hypogeum-live/{}", uuid::Uuid::new_v4())).await;
}
