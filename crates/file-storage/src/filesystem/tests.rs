use super::*;
use crate::{ByteStream, FileStorage, ObjectRef};
use bytes::Bytes;
use futures_util::{StreamExt, stream};

fn config(root: &Path) -> FilesystemConfig {
    FilesystemConfig {
        root: root.into(),
        store_id: "fixtures".into(),
        max_object_bytes: 1_000_000,
    }
}
fn bytes(data: &'static [u8]) -> ByteStream {
    Box::pin(stream::iter([Ok(Bytes::from_static(data))]))
}
fn count(root: &Path, directory: &str) -> usize {
    fs::read_dir(root.join(directory)).unwrap().count()
}
fn reference(key: String) -> ObjectRef {
    ObjectRef {
        version: 1,
        store_id: "fixtures".into(),
        key,
    }
}

#[tokio::test]
async fn source_failure_cleans_partial_bytes() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let input = stream::iter([
        Ok(Bytes::from_static(b"partial")),
        Err(StorageError::io(
            "source",
            io::Error::other("broken source"),
        )),
    ]);
    assert!(
        storage
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "staging"), 0);
    assert_eq!(count(root.path(), "completed"), 0);
}

#[tokio::test]
async fn cancellation_never_publishes_partial_bytes() {
    let root = tempfile::tempdir().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let ops = Arc::new(FsOps {
        cleanup_observer: std::sync::Mutex::new(Some(sender)),
        ..Default::default()
    });
    let storage = Arc::new(
        FilesystemStorage::construct(config(root.path()), ops)
            .await
            .unwrap(),
    );
    let ready = Arc::new(tokio::sync::Notify::new());
    let signal = ready.clone();
    let mut first = true;
    let input = stream::poll_fn(move |_| {
        if first {
            first = false;
            std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"interrupted"))))
        } else {
            signal.notify_one();
            std::task::Poll::Pending
        }
    });
    let client = storage.clone();
    let writer = tokio::spawn(async move {
        client
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await
    });
    ready.notified().await;
    assert_eq!(count(root.path(), "completed"), 0);
    let stage = fs::read_dir(root.path().join("staging"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(matches!(
        storage.inspect(&reference(stage)).await,
        Err(StorageError::NotFound(_))
    ));
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    let executor = std::thread::current().id();
    let cleanup_thread = tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    })
    .await
    .unwrap();
    assert_ne!(cleanup_thread, executor);
    assert_eq!(count(root.path(), "staging"), 0);
    assert_eq!(count(root.path(), "completed"), 0);
}

#[tokio::test]
async fn injected_write_sync_and_install_failures_have_precise_outcomes() {
    for operation in [
        Operation::Create,
        Operation::Write,
        Operation::FileSync,
        Operation::Install,
        Operation::CompletedSync,
        Operation::StagingSync,
    ] {
        let root = tempfile::tempdir().unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let ops = Arc::new(FsOps {
            key: Some(key.clone()),
            ..Default::default()
        });
        let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
            .await
            .unwrap();
        ops.failures.lock().unwrap().push(operation);
        let result = storage
            .write(bytes(b"whole"), ExpectedIntegrity::default())
            .await;
        let uncertain = matches!(
            operation,
            Operation::Install | Operation::CompletedSync | Operation::StagingSync
        );
        assert_eq!(
            matches!(result, Err(StorageError::UncertainCompletion { .. })),
            uncertain,
            "{operation:?}: {result:?}"
        );
        assert!(result.is_err());
        assert_eq!(count(root.path(), "staging"), 0);
        if matches!(operation, Operation::CompletedSync | Operation::StagingSync) {
            assert_eq!(
                storage
                    .inspect(&reference(key.clone()))
                    .await
                    .unwrap()
                    .size_bytes,
                5
            );
            assert_eq!(count(root.path(), "completed"), 1);
        } else {
            assert_eq!(count(root.path(), "completed"), 0);
        }
        // An independent client generates a fresh key for a domain-controlled retry.
        let retry = FilesystemStorage::new(config(root.path())).await.unwrap();
        let completed_before = count(root.path(), "completed");
        let retried = retry
            .write(bytes(b"retry"), ExpectedIntegrity::default())
            .await
            .unwrap();
        assert_ne!(retried.object_ref.key, key);
        assert_eq!(count(root.path(), "completed"), completed_before + 1);
    }
}

#[tokio::test]
async fn failed_cleanup_leaves_only_unreadable_staging_debris() {
    let root = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::default());
    let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
        .await
        .unwrap();
    ops.failures
        .lock()
        .unwrap()
        .extend([Operation::FileSync, Operation::Cleanup]);
    assert!(
        storage
            .write(bytes(b"partial"), ExpectedIntegrity::default())
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "staging"), 1);
    assert_eq!(count(root.path(), "completed"), 0);
    let independent = FilesystemStorage::new(config(root.path())).await.unwrap();
    let debris = fs::read_dir(root.path().join("staging"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(matches!(
        independent.inspect(&reference(debris)).await,
        Err(StorageError::NotFound(_))
    ));
    assert_eq!(count(root.path(), "staging"), 1); // Initialization does not perform online cleanup.
    assert!(
        independent
            .write(bytes(b"complete"), ExpectedIntegrity::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn constructor_sync_failure_and_invalid_configuration_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::default());
    ops.failures.lock().unwrap().push(Operation::LayoutSync);
    assert!(
        FilesystemStorage::construct(config(root.path()), ops)
            .await
            .is_err()
    );
    for (store_id, max_object_bytes) in [("", 1), ("  ", 1), ("valid", 0)] {
        let invalid = FilesystemConfig {
            store_id: store_id.into(),
            max_object_bytes,
            ..config(root.path())
        };
        assert!(matches!(
            FilesystemStorage::new(invalid).await,
            Err(StorageError::InvalidInput(_))
        ));
    }
}

#[tokio::test]
async fn delete_sync_uncertainty_is_retryable_without_recreating_bytes() {
    let root = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::default());
    let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
        .await
        .unwrap();
    let stored = storage
        .write(bytes(b"complete"), ExpectedIntegrity::default())
        .await
        .unwrap();
    ops.failures.lock().unwrap().push(Operation::CompletedSync);
    assert!(matches!(
        storage.delete(&stored.object_ref).await,
        Err(StorageError::UncertainCompletion { .. })
    ));
    assert_eq!(count(root.path(), "completed"), 0);
    storage.delete(&stored.object_ref).await.unwrap();
    assert_eq!(count(root.path(), "completed"), 0);
}

#[tokio::test]
async fn independent_clients_see_only_completed_immutable_objects() {
    let root = tempfile::tempdir().unwrap();
    let a = FilesystemStorage::new(config(root.path())).await.unwrap();
    let b = FilesystemStorage::new(config(root.path())).await.unwrap();
    let (first, second) = tokio::join!(
        a.write(bytes(b"first"), ExpectedIntegrity::default()),
        b.write(bytes(b"second"), ExpectedIntegrity::default())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.object_ref, second.object_ref);
    assert_eq!(a.inspect(&second.object_ref).await.unwrap(), second.info);
    assert_eq!(b.inspect(&first.object_ref).await.unwrap(), first.info);
    fs::write(
        root.path().join("completed").join(&first.object_ref.key),
        b"corrupted",
    )
    .unwrap();
    let mut read = b
        .read(
            &first.object_ref,
            ExpectedIntegrity {
                size_bytes: Some(first.info.size_bytes),
                sha256: Some(first.info.sha256),
            },
        )
        .await
        .unwrap();
    assert!(read.next().await.unwrap().is_ok());
    assert!(matches!(
        read.next().await,
        Some(Err(StorageError::IntegrityMismatch))
    ));
    let actual = b.inspect(&first.object_ref).await.unwrap();
    assert_eq!(actual.size_bytes, 9);
    assert_eq!(
        actual.sha256,
        format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"corrupted"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_directories_and_fifos_are_rejected_without_following_or_blocking() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, b"keep").unwrap();
    for kind in ["symlink", "directory", "fifo"] {
        let reference = reference(uuid::Uuid::new_v4().to_string());
        let path = root.path().join("completed").join(&reference.key);
        match kind {
            "symlink" => symlink(&outside, &path).unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            "fifo" => {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                // SAFETY: path is a valid, live, NUL-terminated string.
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            storage.read(&reference, ExpectedIntegrity::default()).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.delete(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
    }
    assert_eq!(fs::read(outside).unwrap(), b"keep");
    let layout_root = tempfile::tempdir().unwrap();
    symlink(
        root.path().join("completed"),
        layout_root.path().join("completed"),
    )
    .unwrap();
    assert!(matches!(
        FilesystemStorage::new(config(layout_root.path())).await,
        Err(StorageError::InvalidInput(_))
    ));
}

#[test]
fn shared_io_errors_preserve_classification_and_causes() {
    use std::error::Error;
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::Unsupported,
        io::ErrorKind::Other,
    ] {
        let error = StorageError::io("fixture", io::Error::new(kind, "cause"));
        match kind {
            io::ErrorKind::NotFound => assert!(matches!(error, StorageError::NotFound(_))),
            io::ErrorKind::PermissionDenied => {
                assert!(matches!(error, StorageError::PermissionDenied(_)))
            }
            io::ErrorKind::Unsupported => {
                assert!(matches!(error, StorageError::Unsupported { .. }))
            }
            _ => assert!(matches!(error, StorageError::Io { .. })),
        }
        assert_eq!(error.source().unwrap().to_string(), "cause");
    }
}

#[tokio::test]
async fn write_backpressure_does_not_collect_input_before_processing() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::default());
    let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
        .await
        .unwrap();
    ops.failures.lock().unwrap().push(Operation::Write);
    let polled = Arc::new(AtomicUsize::new(0));
    let count = polled.clone();
    let input = stream::iter((0..1000).map(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Bytes::from_static(b"chunk"))
    }));
    assert!(
        storage
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await
            .is_err()
    );
    assert_eq!(polled.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn byte_limit_is_enforced_with_unknown_length_and_oversized_chunks() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let input = stream::iter([Ok(Bytes::from(vec![0; 2_000_000]))]);
    assert!(matches!(
        storage
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await,
        Err(StorageError::SizeLimit { limit: 1_000_000 })
    ));
    assert_eq!(count(root.path(), "completed"), 0);
    assert_eq!(count(root.path(), "staging"), 0);
}

#[tokio::test]
async fn malformed_uuid_encodings_and_other_reference_versions_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let uuid = uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    for key in [
        uuid.simple().to_string(),
        uuid.to_string().to_uppercase(),
        "../completed/object".into(),
        "..\\completed\\object".into(),
        "/tmp/object".into(),
        "C:\\object".into(),
        "00000000-0000-0000-0000-000000000000".into(),
        "bad\0key".into(),
    ] {
        let reference = reference(key);
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
    }
    for version in [0, 2, u32::MAX] {
        let reference = ObjectRef {
            version,
            ..reference(uuid.to_string())
        };
        assert!(matches!(
            storage.inspect(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
    }
}

#[tokio::test]
async fn durable_write_checks_integrity_and_limits() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(FilesystemConfig {
        root: root.path().into(),
        store_id: "test".into(),
        max_object_bytes: 3,
    })
    .await
    .unwrap();
    let input = || Box::pin(stream::iter([Ok(Bytes::from_static(b"abc"))])) as crate::ByteStream;
    let stored = storage
        .write(
            input(),
            ExpectedIntegrity {
                size_bytes: Some(3),
                sha256: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(stored.info.size_bytes, 3);
    assert_eq!(
        std::fs::read(root.path().join("completed").join(stored.object_ref.key)).unwrap(),
        b"abc"
    );
    assert!(matches!(
        storage
            .write(
                input(),
                ExpectedIntegrity {
                    size_bytes: Some(2),
                    sha256: None
                }
            )
            .await,
        Err(StorageError::IntegrityMismatch)
    ));
    let big = Box::pin(stream::iter([Ok(Bytes::from_static(b"abcd"))]));
    assert!(matches!(
        storage.write(big, ExpectedIntegrity::default()).await,
        Err(StorageError::SizeLimit { .. })
    ));
    assert_eq!(
        std::fs::read_dir(root.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn collision_never_overwrites_completed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let ops = Arc::new(FsOps {
        key: Some(key.clone()),
        ..Default::default()
    });
    let storage = FilesystemStorage::construct(
        FilesystemConfig {
            root: root.path().into(),
            store_id: "test".into(),
            max_object_bytes: 10,
        },
        ops,
    )
    .await
    .unwrap();
    let input =
        |bytes| Box::pin(stream::iter([Ok(Bytes::from_static(bytes))])) as crate::ByteStream;
    storage
        .write(input(b"first"), ExpectedIntegrity::default())
        .await
        .unwrap();
    assert!(
        storage
            .write(input(b"second"), ExpectedIntegrity::default())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.path().join("completed").join(key)).unwrap(),
        b"first"
    );
}

#[tokio::test]
async fn round_trip_eof_integrity_and_idempotent_delete() {
    let root = tempfile::tempdir().unwrap();
    let storage: Arc<dyn FileStorage> = Arc::new(
        FilesystemStorage::new(FilesystemConfig {
            root: root.path().into(),
            store_id: "test".into(),
            max_object_bytes: 100,
        })
        .await
        .unwrap(),
    );
    for data in [b"abc".as_slice(), b""] {
        let stored = storage
            .write(
                Box::pin(stream::iter([Ok(Bytes::copy_from_slice(data))])),
                ExpectedIntegrity::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            storage.inspect(&stored.object_ref).await.unwrap(),
            stored.info
        );
        let mut read = storage
            .read(
                &stored.object_ref,
                ExpectedIntegrity {
                    size_bytes: Some(stored.info.size_bytes),
                    sha256: Some(stored.info.sha256.clone()),
                },
            )
            .await
            .unwrap();
        let mut actual = Vec::new();
        while let Some(chunk) = read.next().await {
            actual.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(actual, data);
        let wrong = storage
            .read(
                &stored.object_ref,
                ExpectedIntegrity {
                    size_bytes: Some(500),
                    sha256: None,
                },
            )
            .await
            .unwrap();
        assert!(wrong.collect::<Vec<_>>().await.iter().any(Result::is_err));
        storage.delete(&stored.object_ref).await.unwrap();
        storage.delete(&stored.object_ref).await.unwrap();
        assert!(matches!(
            storage.inspect(&stored.object_ref).await,
            Err(StorageError::NotFound(_))
        ));
    }
    for (version, store_id, key) in [
        (2, "test", "x"),
        (1, "foreign", "x"),
        (1, "test", "../outside"),
        (1, "test", "/absolute"),
    ] {
        let reference = ObjectRef {
            version,
            store_id: store_id.into(),
            key: key.into(),
        };
        assert!(matches!(
            storage.read(&reference, ExpectedIntegrity::default()).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.delete(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
    }
}

#[tokio::test]
async fn empty_chunks_allow_other_tasks_to_cancel_the_write() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let storage = Arc::new(FilesystemStorage::new(config(root.path())).await.unwrap());
    let ready = Arc::new(tokio::sync::Notify::new());
    let signal = ready.clone();
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let input = stream::iter((0..100_000).map(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
        signal.notify_one();
        Ok(Bytes::new())
    }));
    let writer = tokio::spawn(async move {
        storage
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await
    });
    ready.notified().await;
    let processed = polls.load(Ordering::SeqCst);
    writer.abort();
    let _ = writer.await;
    assert!(
        processed < 100_000,
        "write monopolized the executor through {processed} empty chunks"
    );
}

#[tokio::test]
async fn installed_object_with_replayed_exists_error_is_uncertain_and_retained() {
    let root = tempfile::tempdir().unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let ops = Arc::new(FsOps {
        key: Some(key.clone()),
        install_then_error: Some(io::ErrorKind::AlreadyExists),
        ..Default::default()
    });
    let storage = FilesystemStorage::construct(config(root.path()), ops)
        .await
        .unwrap();
    let result = storage
        .write(bytes(b"complete"), ExpectedIntegrity::default())
        .await;
    assert!(
        matches!(result, Err(StorageError::UncertainCompletion { .. })),
        "{result:?}"
    );
    assert_eq!(
        storage.inspect(&reference(key)).await.unwrap().size_bytes,
        8
    );
    assert_eq!(count(root.path(), "completed"), 1);
    assert_eq!(count(root.path(), "staging"), 0);
}

#[tokio::test]
async fn failed_write_cleans_staging_away_from_the_async_executor() {
    let root = tempfile::tempdir().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let ops = Arc::new(FsOps {
        cleanup_observer: std::sync::Mutex::new(Some(sender)),
        ..Default::default()
    });
    let storage = FilesystemStorage::construct(config(root.path()), ops)
        .await
        .unwrap();
    let input = stream::iter([Err(StorageError::io("source", io::Error::other("failed")))]);
    assert!(
        storage
            .write(Box::pin(input), ExpectedIntegrity::default())
            .await
            .is_err()
    );
    let executor = std::thread::current().id();
    let cleanup_thread = tokio::task::spawn_blocking(move || {
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    })
    .await
    .unwrap();
    assert_ne!(cleanup_thread, executor);
    assert_eq!(count(root.path(), "staging"), 0);
}
