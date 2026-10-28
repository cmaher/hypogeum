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
        path_cache: DrivePathCacheConfig::default(),
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
