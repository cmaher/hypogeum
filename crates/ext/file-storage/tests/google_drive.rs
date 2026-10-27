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
