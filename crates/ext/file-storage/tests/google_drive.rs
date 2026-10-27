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
use hypgmx_file_storage::{ByteStream, ExpectedIntegrity};
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
            storage.read(&reference, ExpectedIntegrity::default()).await,
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
            storage.read(&bad, ExpectedIntegrity::default()).await,
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
            ExpectedIntegrity {
                size_bytes: Some(1),
                sha256: None,
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
    let mut read = small
        .read(&reference, ExpectedIntegrity::default())
        .await
        .unwrap();
    assert!(matches!(
        read.next().await,
        Some(Err(StorageError::SizeLimit { limit: 1 }))
    ));
    assert!(read.next().await.is_none());
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
