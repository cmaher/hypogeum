use super::*;
use crate::{ByteStream, FileStorage, ObjectKey, ObjectRef, ReadOptions};
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
/// Regular files outside the staging area.
fn objects(root: &Path) -> usize {
    fn walk(path: &Path) -> usize {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    walk(&entry.path())
                } else {
                    usize::from(kind.is_file())
                }
            })
            .sum()
    }
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name() != STAGING)
        .map(|entry| {
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path())
            } else {
                1
            }
        })
        .sum()
}
fn key(key: &str) -> ObjectKey {
    ObjectKey::new(key).unwrap()
}
fn reference(key: String) -> ObjectRef {
    ObjectRef {
        version: 2,
        store_id: "fixtures".into(),
        key,
    }
}
async fn content(storage: &FilesystemStorage, reference: &ObjectRef) -> Vec<u8> {
    let chunks = storage
        .read(reference, ReadOptions::default())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    chunks
        .into_iter()
        .flat_map(|chunk| chunk.unwrap())
        .collect()
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
            .write(
                &key("a/object"),
                Box::pin(input),
                ExpectedIntegrity::default()
            )
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "staging"), 0);
    assert_eq!(objects(root.path()), 0);
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
            .write(
                &key("object"),
                Box::pin(input),
                ExpectedIntegrity::default(),
            )
            .await
    });
    ready.notified().await;
    assert_eq!(objects(root.path()), 0);
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
        storage
            .inspect(&reference(format!("staging/{stage}")))
            .await,
        Err(StorageError::InvalidReference(_))
    ));
    assert!(matches!(
        storage.inspect(&reference("object".into())).await,
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
    assert_eq!(objects(root.path()), 0);
}

#[tokio::test]
async fn injected_write_sync_and_install_failures_have_precise_outcomes() {
    for operation in [
        Operation::Create,
        Operation::Write,
        Operation::FileSync,
        Operation::CreateParent,
        Operation::CreateParentSync,
        Operation::Install,
        Operation::ParentSync,
        Operation::StagingSync,
    ] {
        let root = tempfile::tempdir().unwrap();
        let object = key("nested/whole");
        let ops = Arc::new(FsOps::default());
        let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
            .await
            .unwrap();
        ops.failures.lock().unwrap().push(operation);
        let result = storage
            .write(&object, bytes(b"whole"), ExpectedIntegrity::default())
            .await;
        let uncertain = matches!(
            operation,
            Operation::Install | Operation::ParentSync | Operation::StagingSync
        );
        assert_eq!(
            matches!(result, Err(StorageError::UncertainCompletion { .. })),
            uncertain,
            "{operation:?}: {result:?}"
        );
        assert!(result.is_err());
        assert_eq!(count(root.path(), "staging"), 0);
        let reference = ObjectRef::new("fixtures", &object);
        if matches!(operation, Operation::ParentSync | Operation::StagingSync) {
            assert_eq!(content(&storage, &reference).await, b"whole");
            assert_eq!(objects(root.path()), 1);
        } else {
            assert_eq!(objects(root.path()), 0);
        }
        // A domain-controlled retry of the same key replaces any installed object.
        let retry = FilesystemStorage::new(config(root.path())).await.unwrap();
        let retried = retry
            .write(&object, bytes(b"again"), ExpectedIntegrity::default())
            .await
            .unwrap();
        assert_eq!(retried.object_ref, reference);
        assert_eq!(content(&retry, &reference).await, b"again");
        assert_eq!(objects(root.path()), 1);
        assert_eq!(count(root.path(), "staging"), 0);
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
            .write(
                &key("partial"),
                bytes(b"partial"),
                ExpectedIntegrity::default()
            )
            .await
            .is_err()
    );
    assert_eq!(count(root.path(), "staging"), 1);
    assert_eq!(objects(root.path()), 0);
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
    assert!(matches!(
        independent.inspect(&reference("partial".into())).await,
        Err(StorageError::NotFound(_))
    ));
    assert_eq!(count(root.path(), "staging"), 1); // Initialization does not perform online cleanup.
    assert!(
        independent
            .write(
                &key("partial"),
                bytes(b"complete"),
                ExpectedIntegrity::default()
            )
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
        .write(
            &key("a/b"),
            bytes(b"complete"),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    ops.failures.lock().unwrap().push(Operation::ParentSync);
    assert!(matches!(
        storage.delete(&stored.object_ref).await,
        Err(StorageError::UncertainCompletion { .. })
    ));
    assert_eq!(objects(root.path()), 0);
    storage.delete(&stored.object_ref).await.unwrap();
    assert_eq!(objects(root.path()), 0);
}

#[tokio::test]
async fn independent_clients_see_only_completed_immutable_objects() {
    let root = tempfile::tempdir().unwrap();
    let a = FilesystemStorage::new(config(root.path())).await.unwrap();
    let b = FilesystemStorage::new(config(root.path())).await.unwrap();
    let (first_key, second_key) = (key("first"), key("second"));
    let (first, second) = tokio::join!(
        a.write(&first_key, bytes(b"first"), ExpectedIntegrity::default()),
        b.write(&second_key, bytes(b"second"), ExpectedIntegrity::default())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.object_ref, second.object_ref);
    assert_eq!(a.inspect(&second.object_ref).await.unwrap(), second.info);
    assert_eq!(b.inspect(&first.object_ref).await.unwrap(), first.info);
    fs::write(root.path().join(&first.object_ref.key), b"corrupted").unwrap();
    let chunks = b
        .read(&first.object_ref, ReadOptions::default())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let actual: Vec<u8> = chunks
        .into_iter()
        .flat_map(|chunk| chunk.unwrap())
        .collect();
    assert_eq!(actual, b"corrupted");
    let mut read = b
        .read(
            &first.object_ref,
            ReadOptions {
                expected: ExpectedIntegrity {
                    size_bytes: Some(first.info.size_bytes),
                    sha256: Some(first.info.sha256),
                },
                ..Default::default()
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
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"corrupted"))
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
        let reference = reference(format!("{kind}/object"));
        fs::create_dir(root.path().join(kind)).unwrap();
        let path = root.path().join(&reference.key);
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
            storage.read(&reference, ReadOptions::default()).await,
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
        // Writes never replace a non-regular entry.
        assert!(matches!(
            storage
                .write(
                    &key(&reference.key),
                    bytes(b"x"),
                    ExpectedIntegrity::default()
                )
                .await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    // A symlinked segment anywhere in the key is never followed.
    let elsewhere = tempfile::tempdir().unwrap();
    fs::create_dir(elsewhere.path().join("inner")).unwrap();
    fs::write(elsewhere.path().join("inner/object"), b"keep").unwrap();
    fs::create_dir(root.path().join("real")).unwrap();
    symlink(elsewhere.path(), root.path().join("real/linked")).unwrap();
    for key_text in ["real/linked/inner/object", "real/linked/inner/new"] {
        let reference = reference(key_text.into());
        assert!(matches!(
            storage.read(&reference, ReadOptions::default()).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage.delete(&reference).await,
            Err(StorageError::InvalidReference(_))
        ));
        assert!(matches!(
            storage
                .write(&key(key_text), bytes(b"x"), ExpectedIntegrity::default())
                .await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    assert_eq!(
        fs::read(elsewhere.path().join("inner/object")).unwrap(),
        b"keep"
    );
    assert!(!elsewhere.path().join("inner/new").exists());
    let layout_root = tempfile::tempdir().unwrap();
    symlink(
        root.path().join("staging"),
        layout_root.path().join("staging"),
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
            .write(
                &key("object"),
                Box::pin(input),
                ExpectedIntegrity::default()
            )
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
            .write(
                &key("object"),
                Box::pin(input),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::SizeLimit { limit: 1_000_000 })
    ));
    assert_eq!(objects(root.path()), 0);
    assert_eq!(count(root.path(), "staging"), 0);
}

#[tokio::test]
async fn malformed_reserved_keys_and_other_reference_versions_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    for key in [
        "",
        "../object",
        "a/../../object",
        "..\\object",
        "/tmp/object",
        "C:\\object",
        "a//b",
        "bad\0key",
        "staging",
        "staging/object",
    ] {
        let reference = reference(key.into());
        assert!(
            matches!(
                storage.inspect(&reference).await,
                Err(StorageError::InvalidReference(_))
            ),
            "{key:?}"
        );
    }
    for reserved in ["staging", "staging/object"] {
        assert!(matches!(
            storage
                .write(&key(reserved), bytes(b"x"), ExpectedIntegrity::default())
                .await,
            Err(StorageError::InvalidInput(_))
        ));
    }
    // Only the first segment is reserved.
    storage
        .write(&key("a/staging"), bytes(b"x"), ExpectedIntegrity::default())
        .await
        .unwrap();
    for version in [0, 1, 3, u32::MAX] {
        let reference = ObjectRef {
            version,
            ..reference("a/staging".into())
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
            &key("dir/abc"),
            input(),
            ExpectedIntegrity {
                size_bytes: Some(3),
                sha256: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(stored.info.size_bytes, 3);
    assert_eq!(std::fs::read(root.path().join("dir/abc")).unwrap(), b"abc");
    assert!(matches!(
        storage
            .write(
                &key("dir/abc"),
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
        storage
            .write(&key("dir/abc"), big, ExpectedIntegrity::default())
            .await,
        Err(StorageError::SizeLimit { .. })
    ));
    // Failed writes leave the existing object in place.
    assert_eq!(std::fs::read(root.path().join("dir/abc")).unwrap(), b"abc");
    assert_eq!(
        std::fs::read_dir(root.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn overwrite_replaces_atomically_for_open_and_new_readers() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let object = key("photos/a.jpg");
    let first = storage
        .write(&object, bytes(b"first"), ExpectedIntegrity::default())
        .await
        .unwrap();
    let open = storage
        .read(&first.object_ref, ReadOptions::default())
        .await
        .unwrap();
    let second = storage
        .write(&object, bytes(b"second"), ExpectedIntegrity::default())
        .await
        .unwrap();
    assert_eq!(second.object_ref, first.object_ref);
    assert_ne!(second.info, first.info);
    // A reader opened before the replacement keeps the previous object.
    let previous: Vec<u8> = open
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(|chunk| chunk.unwrap())
        .collect();
    assert_eq!(previous, b"first");
    assert_eq!(content(&storage, &second.object_ref).await, b"second");
    assert_eq!(
        storage.inspect(&second.object_ref).await.unwrap(),
        second.info
    );
    assert_eq!(objects(root.path()), 1);
    assert_eq!(count(root.path(), "staging"), 0);
}

#[tokio::test]
async fn nested_keys_create_parents_and_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let stored = storage
        .write(
            &key("a/b/c.jpg"),
            bytes(b"nested"),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(stored.object_ref.key, "a/b/c.jpg");
    assert_eq!(fs::read(root.path().join("a/b/c.jpg")).unwrap(), b"nested");
    // A sibling lands next to the object it was derived from.
    let sibling = storage
        .write(
            &key("a/b/c.thumb.jpg"),
            bytes(b"thumb"),
            ExpectedIntegrity::default(),
        )
        .await
        .unwrap();
    assert_eq!(content(&storage, &stored.object_ref).await, b"nested");
    assert_eq!(content(&storage, &sibling.object_ref).await, b"thumb");
    assert_eq!(
        storage.inspect(&stored.object_ref).await.unwrap(),
        stored.info
    );
    storage.delete(&stored.object_ref).await.unwrap();
    storage.delete(&stored.object_ref).await.unwrap();
    assert!(matches!(
        storage.inspect(&stored.object_ref).await,
        Err(StorageError::NotFound(_))
    ));
    assert_eq!(content(&storage, &sibling.object_ref).await, b"thumb");
    // Missing parents read as missing objects, and deletion stays idempotent.
    let missing = reference("no/such/parent".into());
    assert!(matches!(
        storage.inspect(&missing).await,
        Err(StorageError::NotFound(_))
    ));
    storage.delete(&missing).await.unwrap();
    // An object cannot also be a parent.
    assert!(matches!(
        storage
            .write(
                &key("a/b/c.thumb.jpg/x"),
                bytes(b"x"),
                ExpectedIntegrity::default()
            )
            .await,
        Err(StorageError::InvalidInput(_))
    ));
    let under_file = reference("a/b/c.thumb.jpg/x".into());
    assert!(matches!(
        storage.inspect(&under_file).await,
        Err(StorageError::NotFound(_))
    ));
    storage.delete(&under_file).await.unwrap();
    assert_eq!(count(root.path(), "staging"), 0);
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
                &key("round/trip"),
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
                ReadOptions {
                    expected: ExpectedIntegrity {
                        size_bytes: Some(stored.info.size_bytes),
                        sha256: Some(stored.info.sha256.clone()),
                    },
                    ..Default::default()
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
                ReadOptions {
                    expected: ExpectedIntegrity {
                        size_bytes: Some(500),
                        sha256: None,
                    },
                    ..Default::default()
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
        (1, "test", "x"),
        (2, "foreign", "x"),
        (2, "test", "../outside"),
        (2, "test", "/absolute"),
    ] {
        let reference = ObjectRef {
            version,
            store_id: store_id.into(),
            key: key.into(),
        };
        assert!(matches!(
            storage.read(&reference, ReadOptions::default()).await,
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
            .write(
                &key("object"),
                Box::pin(input),
                ExpectedIntegrity::default(),
            )
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
    let ops = Arc::new(FsOps {
        install_then_error: Some(io::ErrorKind::AlreadyExists),
        ..Default::default()
    });
    let storage = FilesystemStorage::construct(config(root.path()), ops)
        .await
        .unwrap();
    let result = storage
        .write(
            &key("object"),
            bytes(b"complete"),
            ExpectedIntegrity::default(),
        )
        .await;
    assert!(
        matches!(result, Err(StorageError::UncertainCompletion { .. })),
        "{result:?}"
    );
    assert_eq!(
        storage
            .inspect(&reference("object".into()))
            .await
            .unwrap()
            .size_bytes,
        8
    );
    assert_eq!(objects(root.path()), 1);
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
            .write(
                &key("object"),
                Box::pin(input),
                ExpectedIntegrity::default()
            )
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

#[tokio::test]
async fn read_limit_cannot_override_adapter_limit() {
    let root = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(config(root.path())).await.unwrap();
    let stored = storage
        .write(&key("object"), bytes(b"abc"), ExpectedIntegrity::default())
        .await
        .unwrap();
    let small = FilesystemStorage::new(FilesystemConfig {
        max_object_bytes: 2,
        ..config(root.path())
    })
    .await
    .unwrap();
    for max_bytes in [None, Some(u64::MAX)] {
        let mut read = small
            .read(
                &stored.object_ref,
                ReadOptions {
                    max_bytes,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            read.next().await,
            Some(Err(StorageError::SizeLimit { limit: 2 }))
        ));
        assert!(read.next().await.is_none());
    }
}

#[tokio::test]
async fn retry_syncs_existing_ancestors_before_installing() {
    let root = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::default());
    let storage = FilesystemStorage::construct(config(root.path()), ops.clone())
        .await
        .unwrap();
    let object = key("a/b/object");
    for _ in 0..2 {
        ops.failures
            .lock()
            .unwrap()
            .push(Operation::CreateParentSync);
        assert!(
            storage
                .write(&object, bytes(b"value"), ExpectedIntegrity::default())
                .await
                .is_err()
        );
        assert!(root.path().join("a").is_dir());
        assert!(!root.path().join("a/b").exists());
        assert!(!root.path().join("a/b/object").exists());
        assert!(ops.failures.lock().unwrap().is_empty());
    }
    let stored = storage
        .write(&object, bytes(b"value"), ExpectedIntegrity::default())
        .await
        .unwrap();
    assert_eq!(content(&storage, &stored.object_ref).await, b"value");
}
