#![cfg(feature = "google-drive")]
mod support;
use hypgmx_file_storage::{
    StorageError,
    google_drive::{DriveScope, GoogleDriveConfig, GoogleDriveStorage},
};
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
        let id = fake.insert_file(vec![], trashed, mime, vec![]);
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
use hypgmx_file_storage::{ByteStream, ExpectedIntegrity, ReadOptions};
fn input(bytes: Vec<u8>) -> ByteStream {
    Box::pin(stream::iter([Ok(Bytes::from(bytes))]))
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
            .write(input(bytes.clone()), ExpectedIntegrity::default())
            .await
            .unwrap();
        assert_eq!(fake.content(&stored.object_ref.key).unwrap(), bytes);
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
        let err = storage.write(source, expected).await.unwrap_err();
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
                .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
            .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
    let mut write = Box::pin(storage.write(Box::pin(source), ExpectedIntegrity::default()));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    tokio::select! { result = &mut write => panic!("unexpected completion: {result:?}"), _ = async { while fake.ranges().is_empty() { tokio::task::yield_now().await; } } => {} }
    drop(write);
    assert_eq!(fake.file_count(), 0);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

use hypgmx_file_storage::{FileStorage, ObjectRef};
fn reference(key: &str) -> ObjectRef {
    ObjectRef {
        version: 1,
        store_id: "test".into(),
        key: key.into(),
    }
}

#[tokio::test]
async fn folder_guard_hides_foreign_trashed_and_folder_objects() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let ids = [
        fake.insert_file(
            vec!["other-folder".into()],
            false,
            "text/plain",
            b"secret".to_vec(),
        ),
        fake.insert_file(
            vec!["folder".into()],
            true,
            "text/plain",
            b"trashed".to_vec(),
        ),
        fake.insert_folder(),
    ];
    for id in ids {
        let reference = reference(&id);
        assert!(matches!(
            storage.read(&reference, ReadOptions::default()).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::NotFound(_))
        ));
        storage.delete(&reference).await.unwrap();
        assert!(fake.content(&id).is_some());
    }
    assert_eq!(fake.media_requests(), 0);
    assert_eq!(fake.deleted(), 0);
}

#[tokio::test]
async fn folder_guard_accepts_files_nested_below_the_root() {
    use sha2::{Digest, Sha256};
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    for depth in 1..=3 {
        let parent = fake.insert_folder_chain("folder", depth);
        let bytes = format!("nested {depth}").into_bytes();
        let id = fake.insert_file(vec![parent], false, "text/plain", bytes.clone());
        let reference = reference(&id);
        let mut read = storage
            .read(&reference, ReadOptions::default())
            .await
            .unwrap();
        let mut actual = Vec::new();
        while let Some(chunk) = read.next().await {
            actual.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(actual, bytes);
        let info = storage.inspect(&reference).await.unwrap();
        assert_eq!(info.sha256, hex::encode(Sha256::digest(&bytes)));
        storage.delete(&reference).await.unwrap();
        assert!(fake.content(&id).is_none());
    }
}

#[tokio::test]
async fn folder_guard_rejects_nested_foreign_trashed_and_non_file_objects() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let outside = fake.insert_folder();
    let foreign = fake.insert_folder_chain(&outside, 2);
    let trashed = fake.insert_folder_chain("folder", 1);
    let below_trashed = fake.insert_folder_chain(&trashed, 1);
    fake.set_trashed(&trashed, true);
    let nested = fake.insert_folder_chain("folder", 2);
    let ids = [
        fake.insert_file(vec![foreign], false, "text/plain", b"foreign".to_vec()),
        fake.insert_file(vec![below_trashed], false, "text/plain", b"x".to_vec()),
        fake.insert_file(
            vec![nested.clone()],
            false,
            support::fake_drive::FOLDER,
            vec![],
        ),
        fake.insert_file(
            vec![nested.clone()],
            false,
            support::fake_drive::SHORTCUT,
            vec![],
        ),
        fake.insert_file(vec!["missing".into()], false, "text/plain", b"x".to_vec()),
        fake.insert_file(vec![], false, "text/plain", b"x".to_vec()),
    ];
    for id in ids {
        let reference = reference(&id);
        assert!(matches!(
            storage.read(&reference, ReadOptions::default()).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::NotFound(_))
        ));
        storage.delete(&reference).await.unwrap();
        assert!(fake.content(&id).is_some());
    }
    assert_eq!(fake.media_requests(), 0);
    assert_eq!(fake.deleted(), 0);
    // A trashed folder is never remembered as inside the root.
    fake.set_trashed(&trashed, false);
    let restored = fake.insert_file(
        vec![trashed.clone()],
        false,
        "text/plain",
        b"restored".to_vec(),
    );
    storage.inspect(&reference(&restored)).await.unwrap();
}

#[tokio::test]
async fn folder_guard_caches_proven_ancestors() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let parent = fake.insert_folder_chain("folder", 3);
    let first = fake.insert_file(vec![parent.clone()], false, "text/plain", b"a".to_vec());
    let second = fake.insert_file(vec![parent], false, "text/plain", b"b".to_vec());
    let before = fake.metadata_requests();
    storage.inspect(&reference(&first)).await.unwrap();
    assert_eq!(fake.metadata_requests() - before, 4);
    let before = fake.metadata_requests();
    storage.inspect(&reference(&second)).await.unwrap();
    assert_eq!(fake.metadata_requests() - before, 1);
}

#[tokio::test]
async fn folder_guard_caps_walk_depth() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let deepest = fake.insert_folder_chain("folder", 32);
    let id = fake.insert_file(vec![deepest], false, "text/plain", b"a".to_vec());
    storage.inspect(&reference(&id)).await.unwrap();
    let too_deep = fake.insert_folder_chain("folder", 33);
    let id = fake.insert_file(vec![too_deep], false, "text/plain", b"a".to_vec());
    assert!(matches!(
        storage.inspect(&reference(&id)).await,
        Err(StorageError::NotFound(_))
    ));
    // Cycles end at the depth cap.
    let (a, b) = (fake.insert_folder(), fake.insert_folder());
    fake.set_parents(&a, vec![b.clone()]);
    fake.set_parents(&b, vec![a.clone()]);
    let id = fake.insert_file(vec![a], false, "text/plain", b"a".to_vec());
    let before = fake.metadata_requests();
    assert!(matches!(
        storage.inspect(&reference(&id)).await,
        Err(StorageError::NotFound(_))
    ));
    assert_eq!(fake.metadata_requests() - before, 33);
}

#[tokio::test]
async fn invalid_references_make_no_requests() {
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let requests = fake.requests();
    for bad in [
        reference("../file"),
        reference(""),
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
    let id = fake.insert_file(
        vec!["folder".into()],
        false,
        "application/octet-stream",
        bytes.clone(),
    );
    let reference = reference(&id);
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
    let id = fake.insert_file(vec!["folder".into()], false, "text/plain", b"abc".to_vec());
    let reference = reference(&id);
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
                .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
            .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
        .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
        .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
        .await
        .unwrap();
    assert_eq!(fake.content(&stored.object_ref.key).unwrap(), b"abc");
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
            .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
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
        storage.write(input(b"abc".to_vec()), oversized).await,
        Err(StorageError::SizeLimit { .. })
    ));
    assert_eq!(fake.requests(), requests);
    let short = ExpectedIntegrity {
        size_bytes: Some(10),
        sha256: None,
    };
    assert!(matches!(
        storage.write(input(vec![1; 9 * 1024 * 1024]), short).await,
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
            .write(input(b"abc".to_vec()), ExpectedIntegrity::default())
            .await,
        Err(StorageError::Io { .. })
    ));
    assert!(fake.ranges().is_empty());
}

#[tokio::test]
async fn ranged_reads_send_range_and_require_partial_content() {
    use hypgmx_file_storage::ByteRange;
    let fake = FakeDrive::start().await;
    let storage = storage(&fake).await;
    let bytes: Vec<u8> = (0..1_000u32).map(|i| (i % 251) as u8).collect();
    let id = fake.insert_file(
        vec!["folder".into()],
        false,
        "application/octet-stream",
        bytes.clone(),
    );
    let reference = reference(&id);
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
}
