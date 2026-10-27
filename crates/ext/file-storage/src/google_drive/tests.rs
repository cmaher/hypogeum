use super::*;

fn config() -> GoogleDriveConfig {
    GoogleDriveConfig {
        store_id: "test".into(),
        folder_id: "folder".into(),
        client_id: "client".into(),
        client_secret: "secret-sentinel".into(),
        refresh_token: "refresh-sentinel".into(),
        scope: DriveScope::Full,
        max_object_bytes: 100,
    }
}

#[test]
fn configuration_redacts_credentials() {
    let debug = format!("{:?}", config());
    assert!(!debug.contains("secret-sentinel"));
    assert!(!debug.contains("refresh-sentinel"));
}

#[tokio::test]
async fn invalid_configuration_fails_before_authentication() {
    for case in 0..3 {
        let mut config = config();
        match case {
            0 => config.store_id.clear(),
            1 => config.folder_id.clear(),
            _ => config.max_object_bytes = 0,
        }
        assert!(matches!(
            GoogleDriveStorage::new(config).await,
            Err(StorageError::InvalidInput(_))
        ));
    }
}

#[test]
fn reference_validation() {
    let storage =
        GoogleDriveStorage::construct(config(), "token".to_string(), "http://localhost/").unwrap();
    let reference = ObjectRef {
        version: 1,
        store_id: "test".into(),
        key: "a-Z_09".into(),
    };
    assert_eq!(storage.resolve(&reference).unwrap(), "a-Z_09");
    for key in ["", "../file", "a/b", "a?b", "é", "a b"] {
        assert!(matches!(
            storage.resolve(&ObjectRef {
                key: key.into(),
                ..reference.clone()
            }),
            Err(StorageError::InvalidReference(_))
        ));
    }
    for bad in [
        ObjectRef {
            version: 2,
            ..reference.clone()
        },
        ObjectRef {
            store_id: "other".into(),
            ..reference
        },
    ] {
        assert!(matches!(
            storage.resolve(&bad),
            Err(StorageError::InvalidReference(_))
        ));
    }
}

#[test]
fn http_error_mapping_and_redaction() {
    for (status, reason, kind) in [
        (404, "notFound", 0),
        (401, "authError", 1),
        (403, "forbidden", 1),
        (403, "rateLimitExceeded", 2),
        (403, "userRateLimitExceeded", 2),
        (429, "", 2),
        (503, "", 2),
    ] {
        let err = error::status(status, Some(reason), "test");
        assert!(match kind {
            0 => matches!(err, StorageError::NotFound(_)),
            1 => matches!(err, StorageError::PermissionDenied(_)),
            _ => matches!(err, StorageError::Io { .. }),
        });
    }
    let err = error::status(403, Some("https://secret-session/token"), "test");
    assert!(!format!("{err:?}").contains("secret-session"));
}
