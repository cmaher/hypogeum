#![cfg(feature = "google-drive")]
mod support;
use hypgmx_file_storage::{
    StorageError,
    google_drive::{DrivePathCacheConfig, DriveScope, GoogleDriveConfig, GoogleDriveStorage},
};
use std::time::Duration;
use support::fake_drive::FakeDrive;

fn config(folder_id: &str) -> GoogleDriveConfig {
    GoogleDriveConfig {
        store_id: "test".into(),
        folder_id: folder_id.into(),
        client_id: "client".into(),
        client_secret: "secret".into(),
        refresh_token: "refresh".into(),
        scope: DriveScope::Full,
        max_object_bytes: 32 * 1024 * 1024,
        path_cache: DrivePathCacheConfig::default(),
    }
}
async fn storage(fake: &FakeDrive) -> GoogleDriveStorage {
    GoogleDriveStorage::with_test_endpoint(config("folder"), &fake.root_url, "test-token")
        .await
        .unwrap()
}

#[tokio::test]
async fn constructor_requires_a_live_folder() {
    let fake = FakeDrive::start().await;
    storage(&fake).await;
    for (mime, trashed) in [
        ("application/octet-stream", false),
        ("application/vnd.google-apps.folder", true),
    ] {
        let id = fake.insert_file("elsewhere", "not-a-folder", trashed, mime, vec![]);
        assert!(matches!(
            GoogleDriveStorage::with_test_endpoint(config(&id), &fake.root_url, "test-token").await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        GoogleDriveStorage::with_test_endpoint(config("missing"), &fake.root_url, "test-token")
            .await,
        Err(StorageError::NotFound(_))
    ));
    assert!(matches!(
        GoogleDriveStorage::with_test_endpoint(config("folder"), &fake.root_url, "wrong-token")
            .await,
        Err(StorageError::PermissionDenied(_))
    ));
}

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use hypgmx_file_storage::{
    ByteStream, ExpectedIntegrity, FileStorage, ObjectKey, ObjectRef, ReadOptions,
};
fn input(bytes: Vec<u8>) -> ByteStream {
    Box::pin(stream::iter([Ok(Bytes::from(bytes))]))
}
fn key(key: &str) -> ObjectKey {
    ObjectKey::new(key).unwrap()
}
fn reference(key: &str) -> ObjectRef {
    ObjectRef {
        version: 2,
        store_id: "test".into(),
        key: key.into(),
    }
}
async fn read_all(
    storage: &GoogleDriveStorage,
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

#[tokio::test]
async fn uploads_empty_exact_and_multiple_chunks_with_partial_ack() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    for length in [0, 8 * 1024 * 1024, 17 * 1024 * 1024 + 33] {
        let bytes: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        if length > 8 * 1024 * 1024 {
            fake.faults(|f| f.partial_ack = Some(256 * 1024));
        }
        let stored = storage
            .write(
                &key(&format!("chunks/{length}")),
                input(bytes.clone()),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        let id = fake.id_at(&stored.object_ref.key).unwrap();
        assert_eq!(fake.content(&id).unwrap(), bytes);
        assert_eq!(stored.info.size_bytes, length as u64);
    }
    assert_eq!(fake.violations(), 0);
    assert!(
        fake.ranges()
            .iter()
            .any(|r| r.starts_with("bytes 262144-8388607/*"))
    );
}

#[tokio::test]
async fn validation_source_and_limit_failures_cancel_without_publishing() {
    for case in 0..4 {
        let fake = FakeDrive::start().await;
        let mut cfg = config("folder");
        if case == 2 {
            cfg.max_object_bytes = 1;
        }
        let storage = GoogleDriveStorage::with_test_endpoint(cfg, &fake.root_url, "test-token")
            .await
            .unwrap();
        let expected = match case {
            0 => ExpectedIntegrity {
                size_bytes: Some(99),
                sha256: None,
            },
            1 => ExpectedIntegrity {
                size_bytes: None,
                sha256: Some("0".repeat(64)),
            },
            _ => ExpectedIntegrity::default(),
        };
        let source = if case == 3 {
            Box::pin(stream::iter([
                Ok(Bytes::from_static(b"abc")),
                Err(StorageError::InvalidInput("source sentinel".into())),
            ])) as ByteStream
        } else {
            input(b"abc".to_vec())
        };
        let err = storage
            .write(&key("object"), source, expected)
            .await
            .unwrap_err();
        assert!(match case {
            0 | 1 => matches!(err, StorageError::IntegrityMismatch),
            2 => matches!(err, StorageError::SizeLimit { .. }),
            _ =>
                matches!(err, StorageError::InvalidInput(ref message) if message == "source sentinel"),
        });
        assert_eq!(fake.file_count(), 0);
        assert_eq!(fake.cancelled_sessions(), 1);
    }
}

#[tokio::test]
async fn final_failure_status_query_matrix() {
    for completed in [false, true] {
        for query_fails in [false, true] {
            let fake = FakeDrive::start().await;
            let storage = storage(&fake).await;
            fake.faults(|f| {
                f.final_failure = Some(completed);
                f.query_failure = query_fails;
            });
            let result = storage
                .write(
                    &key("object"),
                    input(b"abc".to_vec()),
                    ExpectedIntegrity::default(),
                )
                .await;
            if query_fails {
                assert!(matches!(
                    result,
                    Err(StorageError::UncertainCompletion { .. })
                ));
                assert_eq!(fake.cancelled_sessions(), 0);
            } else if completed {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(StorageError::Io { .. })));
                assert_eq!(fake.cancelled_sessions(), 1);
            }
            assert_eq!(fake.file_count(), usize::from(completed));
            assert_eq!(fake.deleted(), 0);
        }
    }
}

#[tokio::test]
async fn upload_cross_checks_server_metadata() {
    for case in 0..3 {
        let fake = FakeDrive::start().await;
        let storage = storage(&fake).await;
        fake.faults(|f| match case {
            0 => f.bad_checksum = true,
            1 => f.bad_size = true,
            _ => f.omit_checksum = true,
        });
        let result = storage
            .write(
                &key("object"),
                input(b"abc".to_vec()),
                ExpectedIntegrity::default(),
            )
            .await;
        if case == 2 {
            assert!(result.is_ok());
            assert_eq!(fake.file_count(), 1);
        } else {
            assert!(matches!(result, Err(StorageError::IntegrityMismatch)));
            assert_eq!(fake.file_count(), 0);
        }
    }
}

#[tokio::test]
async fn failed_intermediate_chunk_cancels_and_redacts_errors() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| f.intermediate_failure = true);
    let err = storage
        .write(
            &key("object"),
            input(vec![7; 8 * 1024 * 1024 + 1]),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::Io { .. }));
    assert!(!format!("{err:?}").contains("http"));
    assert!(!format!("{err:?}").contains("test-token"));
    assert_eq!(fake.cancelled_sessions(), 1);
    assert_eq!(fake.file_count(), 0);
}

#[tokio::test]
async fn upload_is_pull_driven_and_cancellation_never_publishes() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let count = Arc::new(AtomicUsize::new(0));
    let polls = count.clone();
    let source = stream::once(async move {
        polls.fetch_add(1, Ordering::SeqCst);
        Ok(Bytes::from(vec![1; 8 * 1024 * 1024 + 1]))
    })
    .chain(stream::pending());
    let object = key("object");
    let mut write =
        Box::pin(storage.write(&object, Box::pin(source), ExpectedIntegrity::default()));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    tokio::select! { result = &mut write => panic!("unexpected completion: {result:?}"), _ = async { while fake.ranges().is_empty() { tokio::task::yield_now().await; } } => {} }
    drop(write);
    assert_eq!(fake.file_count(), 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_references_make_no_requests() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let requests = fake.requests();
    for bad in [
        reference("../file"),
        reference(""),
        reference("a//b"),
        reference("/absolute"),
        ObjectRef {
            version: 1,
            ..reference("file")
        },
        ObjectRef {
            version: 99,
            ..reference("file")
        },
        ObjectRef {
            store_id: "other".into(),
            ..reference("file")
        },
    ] {
        assert!(matches!(
            storage.read(&bad, ReadOptions::default()).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.inspect(&bad).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.delete(&bad).await,
            Err(StorageError::InvalidReference(_))
        ));
    }
    assert_eq!(fake.requests(), requests);
}

#[tokio::test]
async fn reads_are_bounded_and_integrity_errors_precede_eof() {
    use sha2::{Digest, Sha256};
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let bytes = vec![23; 400_000];
    fake.insert_file(
        "folder",
        "object",
        false,
        "application/octet-stream",
        bytes.clone(),
    );
    let reference = reference("object");
    let info = storage.inspect(&reference).await.unwrap();
    assert_eq!(info.size_bytes, bytes.len() as u64);
    assert_eq!(info.sha256, hex::encode(Sha256::digest(&bytes)));
    let mut read = storage
        .read(
            &reference,
            ReadOptions {
                expected: ExpectedIntegrity {
                    size_bytes: Some(1),
                    sha256: None,
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut actual = Vec::new();
    loop {
        match read.next().await.unwrap() {
            Ok(chunk) => {
                assert!(chunk.len() <= 64 * 1024);
                actual.extend_from_slice(&chunk);
            }
            Err(error) => {
                assert!(matches!(error, StorageError::IntegrityMismatch));
                break;
            }
        }
    }
    assert_eq!(actual, bytes);
    assert!(read.next().await.is_none());
    let small = GoogleDriveStorage::with_test_endpoint(
        GoogleDriveConfig {
            max_object_bytes: 1,
            ..config("folder")
        },
        &fake.root_url,
        "test-token",
    )
    .await
    .unwrap();
    for max_bytes in [None, Some(u64::MAX)] {
        let mut read = small
            .read(
                &reference,
                ReadOptions {
                    max_bytes,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            read.next().await,
            Some(Err(StorageError::SizeLimit { limit: 1 }))
        ));
        assert!(read.next().await.is_none());
    }
}

#[tokio::test]
async fn deletion_is_idempotent_and_lost_response_is_uncertain() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let id = fake.insert_file("folder", "object", false, "text/plain", b"abc".to_vec());
    let reference = reference("object");
    fake.faults(|f| f.delete_disconnect = true);
    assert!(matches!(
        storage.delete(&reference).await,
        Err(StorageError::UncertainCompletion { .. })
    ));
    assert!(fake.content(&id).is_none());
    storage.delete(&reference).await.unwrap();
    storage.delete(&reference).await.unwrap();
}

#[tokio::test]
async fn drive_http_errors_have_safe_classification() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    for (code, reason, permission) in [
        (401, "authError", true),
        (403, "forbidden", true),
        (403, "rateLimitExceeded", false),
        (403, "userRateLimitExceeded", false),
        (429, "", false),
        (503, "backendError", false),
    ] {
        fake.faults(|f| f.next_status = Some((code, reason)));
        let err = storage.inspect(&reference("some-file")).await.unwrap_err();
        assert!(if permission {
            matches!(err, StorageError::PermissionDenied(_))
        } else {
            matches!(err, StorageError::Io { .. })
        });
        assert!(!format!("{err:?}").contains("secret-token"));
    }
}

#[tokio::test]
async fn lost_final_responses_are_queried_once_and_never_blindly_deleted() {
    for completed in [false, true] {
        for query_fails in [false, true] {
            let fake = FakeDrive::start().await;
            let storage = storage(&fake).await;
            fake.faults(|f| {
                f.final_disconnect = Some(completed);
                f.query_disconnect = query_fails;
            });
            let result = storage
                .write(
                    &key("object"),
                    input(b"abc".to_vec()),
                    ExpectedIntegrity::default(),
                )
                .await;
            if query_fails {
                assert!(matches!(
                    result,
                    Err(StorageError::UncertainCompletion { .. })
                ));
            } else if completed {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(StorageError::Io { .. })));
            }
            assert_eq!(
                fake.ranges()
                    .iter()
                    .filter(|r| r.as_str() == "bytes */3")
                    .count(),
                1
            );
            assert_eq!(fake.file_count(), usize::from(completed));
            assert_eq!(fake.deleted(), 0);
        }
    }
}

#[tokio::test]
async fn unreadable_success_metadata_retains_possibly_completed_file() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| f.malformed_final = true);
    assert!(matches!(
        storage
            .write(
                &key("object"),
                input(b"abc".to_vec()),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::UncertainCompletion { .. })
    ));
    assert_eq!(fake.file_count(), 1);
    assert_eq!(fake.deleted(), 0);
    assert_eq!(fake.cancelled_sessions(), 0);
}

#[tokio::test]
async fn truncated_final_response_body_uses_status_query() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| f.final_body_failure = true);
    let result = storage
        .write(
            &key("object"),
            input(b"abc".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await;
    assert!(
        result.is_ok(),
        "truncated final reply must be settled: {result:?}"
    );
    assert_eq!(
        fake.ranges()
            .iter()
            .filter(|r| r.as_str() == "bytes */3")
            .count(),
        1
    );
    assert_eq!(fake.file_count(), 1);
}

#[tokio::test]
async fn partially_acknowledged_final_chunk_resends_the_tail() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| f.final_partial_ack = Some(1));
    let stored = storage
        .write(
            &key("object"),
            input(b"abc".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    let id = fake.id_at(&stored.object_ref.key).unwrap();
    assert_eq!(fake.content(&id).unwrap(), b"abc");
    assert_eq!(fake.ranges(), ["bytes 0-2/3", "bytes 1-2/3"]);
    assert_eq!(fake.cancelled_sessions(), 0);
}

#[tokio::test]
async fn unconfirmed_mismatch_cleanup_is_uncertain() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| {
        f.bad_checksum = true;
        f.delete_failure = true;
    });
    assert!(matches!(
        storage
            .write(
                &key("object"),
                input(b"abc".to_vec()),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::UncertainCompletion { .. })
    ));
    assert_eq!(fake.file_count(), 1);
}

#[tokio::test]
async fn declared_sizes_fail_before_uploading_excess_bytes() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let requests = fake.requests();
    let oversized = ExpectedIntegrity {
        size_bytes: Some(32 * 1024 * 1024 + 1),
        sha256: None,
    };
    assert!(matches!(
        storage
            .write(&key("object"), input(b"abc".to_vec()), oversized)
            .await,
        Err(StorageError::SizeLimit { .. })
    ));
    assert_eq!(fake.requests(), requests);
    let short = ExpectedIntegrity {
        size_bytes: Some(10),
        sha256: None,
    };
    assert!(matches!(
        storage
            .write(&key("object"), input(vec![1; 9 * 1024 * 1024]), short)
            .await,
        Err(StorageError::IntegrityMismatch)
    ));
    assert!(fake.ranges().is_empty());
    assert_eq!(fake.cancelled_sessions(), 1);
}

#[tokio::test]
async fn foreign_session_uri_is_never_followed() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.faults(|f| f.foreign_location = true);
    assert!(matches!(
        storage
            .write(
                &key("object"),
                input(b"abc".to_vec()),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::Io { .. })
    ));
    assert!(fake.ranges().is_empty());
}

#[tokio::test]
async fn ranged_reads_send_range_and_require_partial_content() {
    use hypgmx_file_storage::ByteRange;
    let fake = FakeDrive::start().await;
    // Uncached, so single-shot faults are observed rather than resolved again.
    let storage = storage_with_cache(
        &fake,
        DrivePathCacheConfig {
            capacity: 0,
            ..Default::default()
        },
    )
    .await;
    let bytes: Vec<u8> = (0..1_000u32).map(|i| (i % 251) as u8).collect();
    let folder = fake.insert_folder("folder", "nested");
    fake.insert_file(
        &folder,
        "object",
        false,
        "application/octet-stream",
        bytes.clone(),
    );
    let reference = reference("nested/object");
    let ranged = |offset, length| ReadOptions {
        range: Some(ByteRange { offset, length }),
        ..Default::default()
    };
    let mut read = storage.read(&reference, ranged(100, 50)).await.unwrap();
    let mut actual = Vec::new();
    while let Some(chunk) = read.next().await {
        actual.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(actual, bytes[100..150]);
    assert_eq!(fake.media_ranges(), ["bytes=100-149"]);
    // Zero-length ranges are served without downloading.
    let media = fake.media_requests();
    let mut read = storage.read(&reference, ranged(10, 0)).await.unwrap();
    assert!(read.next().await.is_none());
    assert_eq!(fake.media_requests(), media);
    // A server ignoring Range must never stream the whole object.
    fake.faults(|f| f.ignore_range = true);
    assert!(matches!(
        storage.read(&reference, ranged(0, 10)).await,
        Err(StorageError::Io { .. })
    ));
    fake.faults(|f| f.unsatisfiable_range = true);
    assert!(matches!(
        storage.read(&reference, ranged(0, 10)).await,
        Err(StorageError::InvalidInput(_))
    ));
    assert_eq!(
        fake.media_ranges(),
        ["bytes=100-149", "bytes=0-9", "bytes=0-9"]
    );
    // A 206 for a different range is never streamed.
    fake.faults(|f| f.wrong_content_range = true);
    assert!(matches!(
        storage.read(&reference, ranged(0, 10)).await,
        Err(StorageError::Io { .. })
    ));
    // Other statuses map like unranged reads.
    fake.faults(|f| f.range_status = Some((404, "notFound")));
    assert!(matches!(
        storage.read(&reference, ranged(0, 10)).await,
        Err(StorageError::NotFound(_))
    ));
    fake.faults(|f| f.range_status = Some((403, "forbidden")));
    assert!(matches!(
        storage.read(&reference, ranged(0, 10)).await,
        Err(StorageError::PermissionDenied(_))
    ));
    // Bodies shorter or longer than the range fail as downstream errors.
    for fault in [
        (|f: &mut support::fake_drive::Faults| f.short_range_body = true) as fn(&mut _),
        |f| f.long_range_body = true,
    ] {
        fake.faults(fault);
        let mut read = storage.read(&reference, ranged(0, 10)).await.unwrap();
        let mut failure = None;
        while let Some(chunk) = read.next().await {
            if let Err(error) = chunk {
                failure = Some(error);
                break;
            }
        }
        assert!(matches!(failure, Some(StorageError::Io { .. })));
    }
}

fn range_options(offset: u64, length: u64) -> ReadOptions {
    ReadOptions {
        range: Some(hypgmx_file_storage::ByteRange { offset, length }),
        ..Default::default()
    }
}

#[tokio::test]
async fn writes_create_missing_folders_once() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let stored = storage
        .write(
            &key("a/b/c.jpg"),
            input(b"c".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(stored.object_ref, reference("a/b/c.jpg"));
    assert_eq!(fake.folders_created(), 2);
    let id = fake.id_at("a/b/c.jpg").unwrap();
    assert_eq!(fake.content(&id).unwrap(), b"c");
    // Siblings and shallower keys reuse the cached folders.
    let lists = fake.list_requests();
    for path in ["a/b/d.jpg", "a/e.jpg"] {
        storage
            .write(
                &key(path),
                input(b"x".to_vec()),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
    }
    assert_eq!(fake.folders_created(), 2);
    assert_eq!(
        fake.list_requests() - lists,
        2,
        "one file lookup per new key"
    );
    // An independent client finds the existing folders by name.
    let other = storage_with_cache(&fake, DrivePathCacheConfig::default()).await;
    other
        .write(
            &key("a/b/f.jpg"),
            input(b"f".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(fake.folders_created(), 2);
    assert_eq!(
        read_all(&storage, &reference("a/b/f.jpg"), ReadOptions::default()).await,
        b"f"
    );
    assert_eq!(fake.file_count(), 4);
}

#[tokio::test]
async fn concurrent_writes_share_new_folders() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let (first, second) = (key("shared/one"), key("shared/two"));
    let (a, b) = tokio::join!(
        storage.write(&first, input(b"1".to_vec()), ExpectedIntegrity::default()),
        storage.write(&second, input(b"2".to_vec()), ExpectedIntegrity::default()),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(fake.folders_created(), 1);
}

#[tokio::test]
async fn writes_replace_existing_files_in_place() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let folder = fake.insert_folder("folder", "a");
    let id = fake.insert_file(&folder, "c.jpg", false, "text/plain", b"old".to_vec());
    for content in [b"new".as_slice(), b"newer"] {
        let stored = storage
            .write(
                &key("a/c.jpg"),
                input(content.to_vec()),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        assert_eq!(stored.info.size_bytes, content.len() as u64);
        assert_eq!(fake.id_at("a/c.jpg").unwrap(), id);
        assert_eq!(fake.content(&id).unwrap(), content);
        assert_eq!(fake.file_count(), 1);
        assert_eq!(
            read_all(&storage, &reference("a/c.jpg"), ReadOptions::default()).await,
            content
        );
    }
    assert_eq!(fake.folders_created(), 0);
    // A replacement that fails server verification removes the object rather
    // than leaving unverified bytes at the key.
    fake.faults(|f| f.bad_checksum = true);
    assert!(matches!(
        storage
            .write(
                &key("a/c.jpg"),
                input(b"bad".to_vec()),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::IntegrityMismatch)
    ));
    assert_eq!(fake.file_count(), 0);
    assert!(matches!(
        storage.inspect(&reference("a/c.jpg")).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn paths_resolve_by_name_below_the_storage_folder() {
    use sha2::{Digest, Sha256};
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let a = fake.insert_folder("folder", "a");
    let b = fake.insert_folder(&a, "b");
    let bytes = b"nested".to_vec();
    let id = fake.insert_file(&b, "c.jpg", false, "text/plain", bytes.clone());
    // Same names outside the storage folder, trashed, or of the other kind are ignored.
    let outside = fake.insert_folder("elsewhere", "a");
    let outside = fake.insert_file(&outside, "c.jpg", false, "text/plain", b"outside".to_vec());
    fake.insert_file(&b, "c.jpg", true, "text/plain", b"trashed".to_vec());
    fake.insert_folder(&b, "c.jpg");
    fake.insert_file("folder", "a", false, "text/plain", b"file named a".to_vec());
    let object = reference("a/b/c.jpg");
    assert_eq!(
        read_all(&storage, &object, ReadOptions::default()).await,
        bytes
    );
    assert_eq!(
        read_all(&storage, &object, range_options(1, 3)).await,
        b"est"
    );
    let info = storage.inspect(&object).await.unwrap();
    assert_eq!(info.sha256, hex::encode(Sha256::digest(&bytes)));
    storage.delete(&object).await.unwrap();
    assert!(fake.content(&id).is_none());
    storage.delete(&object).await.unwrap();
    for missing in [
        "a/b/c.jpg",
        "a/b",
        "a/missing/c.jpg",
        "missing",
        "a/b/c.jpg/d",
    ] {
        assert!(
            matches!(
                storage.inspect(&reference(missing)).await,
                Err(StorageError::NotFound(_))
            ),
            "{missing}"
        );
        storage.delete(&reference(missing)).await.unwrap();
    }
    assert_eq!(fake.content(&outside).unwrap(), b"outside");
}

#[tokio::test]
async fn duplicate_names_are_ambiguous() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let a = fake.insert_folder("folder", "a");
    fake.insert_file(&a, "dup", false, "text/plain", b"1".to_vec());
    fake.insert_file(&a, "dup", false, "text/plain", b"2".to_vec());
    fake.insert_folder("folder", "twin");
    fake.insert_folder("folder", "twin");
    for path in ["a/dup", "twin/file"] {
        let ambiguous = |result| matches!(result, Err(StorageError::InvalidInput(_)));
        assert!(ambiguous(storage.inspect(&reference(path)).await.map(drop)));
        assert!(ambiguous(storage.delete(&reference(path)).await));
        assert!(ambiguous(
            storage
                .write(
                    &key(path),
                    input(b"x".to_vec()),
                    ExpectedIntegrity::default()
                )
                .await
                .map(drop)
        ));
    }
    assert_eq!(fake.media_requests(), 0);
    assert_eq!(fake.deleted(), 0);
    assert!(fake.ranges().is_empty());
    assert_eq!(fake.folders_created(), 0);
    assert_eq!(fake.file_count(), 2);
}

#[tokio::test]
async fn queries_escape_quotes_in_names() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let path = r"it's/a 'quoted' and name";
    storage
        .write(
            &key(path),
            input(b"q".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert!(fake.id_at(path).is_some());
    assert!(
        fake.queries()
            .iter()
            .any(|q| q.starts_with(r"name = 'it\'s' and 'folder' in parents"))
    );
    assert!(
        fake.queries()
            .iter()
            .any(|q| q.starts_with(r"name = 'a \'quoted\' and name' and"))
    );
    let fresh = storage_with_cache(&fake, DrivePathCacheConfig::default()).await;
    assert_eq!(
        read_all(&fresh, &reference(path), ReadOptions::default()).await,
        b"q"
    );
}

#[tokio::test]
async fn shortcuts_are_not_objects_and_are_never_replaced() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    fake.insert_file(
        "folder",
        "link",
        false,
        support::fake_drive::SHORTCUT,
        vec![],
    );
    assert!(matches!(
        storage.inspect(&reference("link")).await,
        Err(StorageError::NotFound(_))
    ));
    assert!(matches!(
        storage
            .write(
                &key("link"),
                input(b"x".to_vec()),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::InvalidInput(_))
    ));
    assert_eq!(fake.file_count(), 1);
}

async fn storage_with_cache(
    fake: &FakeDrive,
    path_cache: DrivePathCacheConfig,
) -> GoogleDriveStorage {
    GoogleDriveStorage::with_test_endpoint(
        GoogleDriveConfig {
            path_cache,
            ..config("folder")
        },
        &fake.root_url,
        "test-token",
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn cached_paths_skip_resolution_for_reads_and_inspection() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let folder = fake.insert_folder("folder", "a");
    fake.insert_file(&folder, "b", false, "text/plain", b"abcdef".to_vec());
    let reference = reference("a/b");
    let lists = fake.list_requests();
    let media = fake.media_requests();
    for offset in 0..3 {
        assert_eq!(
            read_all(&storage, &reference, range_options(offset, 1)).await,
            &b"abcdef"[offset as usize..offset as usize + 1]
        );
    }
    storage.inspect(&reference).await.unwrap();
    assert_eq!(fake.list_requests() - lists, 2, "one lookup per segment");
    assert_eq!(fake.media_requests() - media, 4);
    // The cached size rejects ranges past the end without a download.
    assert!(matches!(
        storage.read(&reference, range_options(6, 1)).await,
        Err(StorageError::InvalidInput(_))
    ));
    assert_eq!(fake.media_requests() - media, 4);
    assert_eq!(
        fake.metadata_requests(),
        1,
        "only the constructor folder check"
    );
}

#[tokio::test]
async fn path_cache_can_be_disabled_by_capacity_or_ttl() {
    for path_cache in [
        DrivePathCacheConfig {
            capacity: 0,
            ..Default::default()
        },
        DrivePathCacheConfig {
            ttl: Duration::ZERO,
            ..Default::default()
        },
    ] {
        let fake = FakeDrive::start().await;
        let storage = storage_with_cache(&fake, path_cache).await;
        let folder = fake.insert_folder("folder", "a");
        fake.insert_file(&folder, "b", false, "text/plain", b"abc".to_vec());
        let lists = fake.list_requests();
        for _ in 0..3 {
            read_all(&storage, &reference("a/b"), range_options(0, 1)).await;
        }
        assert_eq!(fake.list_requests() - lists, 6);
    }
    let fake = FakeDrive::start().await;
    let invalid = DrivePathCacheConfig {
        ttl: Duration::MAX,
        ..Default::default()
    };
    assert!(matches!(
        GoogleDriveStorage::with_test_endpoint(
            GoogleDriveConfig {
                path_cache: invalid,
                ..config("folder")
            },
            &fake.root_url,
            "test-token"
        )
        .await,
        Err(StorageError::InvalidInput(_))
    ));
}

/// Another client replaces `a/b` with a new file of different content.
fn replace_externally(fake: &FakeDrive, bytes: &[u8]) -> String {
    let old = fake.id_at("a/b").unwrap();
    let folder = fake.id_at("a").unwrap();
    fake.remove(&old);
    fake.insert_file(&folder, "b", false, "text/plain", bytes.to_vec())
}

#[tokio::test]
async fn stale_cached_ids_are_evicted_and_resolved_again_once() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    storage
        .write(
            &key("a/b"),
            input(b"first".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    // Unranged and ranged reads, and inspection.
    replace_externally(&fake, b"second");
    assert_eq!(
        read_all(&storage, &reference("a/b"), ReadOptions::default()).await,
        b"second"
    );
    replace_externally(&fake, b"third");
    assert_eq!(
        read_all(&storage, &reference("a/b"), range_options(1, 2)).await,
        b"hi"
    );
    replace_externally(&fake, b"fourth");
    assert_eq!(
        storage.inspect(&reference("a/b")).await.unwrap().size_bytes,
        6
    );
    // A cached size that is now too small is checked again before rejecting.
    let id = fake.id_at("a/b").unwrap();
    fake.set_content(&id, b"fourth, longer".to_vec());
    assert_eq!(
        read_all(&storage, &reference("a/b"), range_options(8, 6)).await,
        b"longer"
    );
    // Writes replace the file now at the key instead of failing on the stale ID.
    replace_externally(&fake, b"fifth");
    storage
        .write(
            &key("a/b"),
            input(b"sixth".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(fake.file_count(), 1);
    assert_eq!(fake.content(&fake.id_at("a/b").unwrap()).unwrap(), b"sixth");
    // Deletion removes the file now at the key.
    let replacement = replace_externally(&fake, b"seventh");
    storage.delete(&reference("a/b")).await.unwrap();
    assert!(fake.content(&replacement).is_none());
    assert_eq!(fake.file_count(), 0);
}

#[tokio::test]
async fn stale_cached_folders_are_resolved_again() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    storage
        .write(
            &key("a/b"),
            input(b"first".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    storage
        .write(
            &key("a/c"),
            input(b"c".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    // Another client deletes folder `a` and recreates the tree.
    let old = fake.id_at("a").unwrap();
    for path in ["a/b", "a/c"] {
        fake.remove(&fake.id_at(path).unwrap());
    }
    fake.remove(&old);
    let folder = fake.insert_folder("folder", "a");
    fake.insert_file(&folder, "c", false, "text/plain", b"recreated".to_vec());
    // Uploads into the deleted cached folder are retried against the current one.
    storage
        .write(
            &key("a/d"),
            input(b"d".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(fake.content(&fake.id_at("a/d").unwrap()).unwrap(), b"d");
    assert_eq!(fake.folders_created(), 1);
    // The cached file IDs are gone.
    assert_eq!(
        read_all(&storage, &reference("a/c"), ReadOptions::default()).await,
        b"recreated"
    );
    assert!(matches!(
        storage.inspect(&reference("a/b")).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn genuine_misses_are_not_retried() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    storage
        .write(
            &key("a/b"),
            input(b"abc".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    // Uncached: one lookup for the folder, then one for the missing file.
    let lists = fake.list_requests();
    assert!(matches!(
        storage.inspect(&reference("a/missing")).await,
        Err(StorageError::NotFound(_))
    ));
    // The cached folder made the miss worth one fresh walk.
    assert_eq!(fake.list_requests() - lists, 3);
    // Media failures for a freshly resolved file are returned, not retried.
    let fresh = storage_with_cache(
        &fake,
        DrivePathCacheConfig {
            capacity: 0,
            ..Default::default()
        },
    )
    .await;
    for status in [404, 416] {
        fake.faults(|f| f.media_status = Some((status, "notFound")));
        let lists = fake.list_requests();
        assert!(fresh.inspect(&reference("a/b")).await.is_err());
        assert_eq!(fake.list_requests() - lists, 2);
    }
}

#[tokio::test]
async fn media_failures_on_cached_ids_resolve_again() {
    for status in [404, 416] {
        let fake = FakeDrive::start().await;
        let storage = storage(&fake).await;
        fake.insert_file("folder", "object", false, "text/plain", b"abc".to_vec());
        let reference = reference("object");
        storage.inspect(&reference).await.unwrap();
        let lists = fake.list_requests();
        fake.faults(|f| f.media_status = Some((status, "notFound")));
        storage.inspect(&reference).await.unwrap();
        assert_eq!(fake.list_requests() - lists, 1);
        fake.faults(|f| f.range_status = Some((status, "notFound")));
        read_all(&storage, &reference, range_options(0, 1)).await;
        assert_eq!(fake.list_requests() - lists, 2);
    }
}

#[tokio::test]
async fn expired_paths_are_resolved_again() {
    let fake = FakeDrive::start().await;
    let storage = storage_with_cache(
        &fake,
        DrivePathCacheConfig {
            ttl: Duration::from_millis(50),
            ..Default::default()
        },
    )
    .await;
    let id = fake.insert_file("folder", "object", false, "text/plain", b"abc".to_vec());
    let reference = reference("object");
    storage.inspect(&reference).await.unwrap();
    fake.set_trashed(&id, true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(matches!(
        storage.inspect(&reference).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn concurrent_writes_to_one_new_key_replace_instead_of_duplicate() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for path_cache in [
        DrivePathCacheConfig::default(),
        DrivePathCacheConfig {
            capacity: 0,
            ..Default::default()
        },
    ] {
        let fake = FakeDrive::start().await;
        let storage = storage_with_cache(&fake, path_cache).await;
        let object = key("shared/object");
        let ready = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let source = {
            let ready = ready.clone();
            let release = release.clone();
            Box::pin(stream::once(async move {
                ready.notify_one();
                release.notified().await;
                Ok(bytes::Bytes::from_static(b"first"))
            }))
        };
        let first = storage.write(&object, source, ExpectedIntegrity::default());
        tokio::pin!(first);
        tokio::select! {
            result = &mut first => panic!("write completed before release: {result:?}"),
            _ = ready.notified() => {}
        }
        let consumed = Arc::new(AtomicBool::new(false));
        let observed = consumed.clone();
        let second = storage.write(
            &object,
            Box::pin(stream::once(async move {
                observed.store(true, Ordering::SeqCst);
                Ok(bytes::Bytes::from_static(b"second"))
            })),
            ExpectedIntegrity::default(),
        );
        tokio::pin!(second);
        // Drive both operations until the first completes, allowing the second
        // to race its lookup while the first upload is deliberately paused.
        let release_first = async {
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            assert!(!consumed.load(Ordering::SeqCst));
            release.notify_one();
        };
        let (a, b, ()) = tokio::join!(first, second, release_first);
        a.unwrap();
        b.unwrap();
        assert_eq!(fake.file_count(), 1);
        let fresh = storage_with_cache(&fake, path_cache).await;
        assert_eq!(
            read_all(&fresh, &reference("shared/object"), ReadOptions::default()).await,
            b"second"
        );
    }
}

#[tokio::test]
async fn stale_cached_parent_is_retried_when_creating_nested_folders() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    storage
        .write(
            &key("a/b"),
            input(b"first".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    fake.remove(&fake.id_at("a/b").unwrap());
    fake.remove(&fake.id_at("a").unwrap());
    fake.insert_folder("folder", "a");
    storage
        .write(
            &key("a/new/child"),
            input(b"second".to_vec()),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        read_all(&storage, &reference("a/new/child"), ReadOptions::default()).await,
        b"second"
    );
}
