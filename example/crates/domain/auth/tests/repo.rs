use exemplum_dmn_auth::repo::{AuthRepo, InMemoryAuthRepo};
use uuid::Uuid;

#[tokio::test]
async fn duplicate_ids_and_active_identifiers_are_rejected_without_replacing_identity() {
    let repo = InMemoryAuthRepo::new();
    let id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let original = repo
        .create(
            id,
            user_id,
            "password",
            "alice",
            "hash",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert!(
        repo.create(
            id,
            Uuid::new_v4(),
            "password",
            "bob",
            "other",
            serde_json::json!({})
        )
        .await
        .is_err()
    );
    assert_eq!(repo.find_active_by_id(id).await.unwrap(), Some(original));
    assert!(
        repo.create(
            Uuid::new_v4(),
            user_id,
            "password",
            "alice",
            "other",
            serde_json::json!({})
        )
        .await
        .is_err()
    );
    repo.revoke(id).await.unwrap();
    assert!(
        repo.create(
            Uuid::new_v4(),
            user_id,
            "password",
            "alice",
            "new",
            serde_json::json!({})
        )
        .await
        .is_ok()
    );
    assert!(
        repo.create(
            Uuid::new_v4(),
            user_id,
            "api_key",
            "alice",
            "key",
            serde_json::json!({})
        )
        .await
        .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_registration_accepts_only_one_active_identifier() {
    let repo = InMemoryAuthRepo::new();
    let user_id = Uuid::new_v4();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(17));
    let mut registrations = Vec::new();
    for _ in 0..16 {
        let repo = repo.clone();
        let barrier = barrier.clone();
        registrations.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.create(
                Uuid::new_v4(),
                user_id,
                "password",
                "alice",
                "hash",
                serde_json::json!({}),
            )
            .await
        }));
    }
    barrier.wait().await;
    let mut accepted = 0;
    for registration in registrations {
        accepted += usize::from(registration.await.unwrap().is_ok());
    }
    assert_eq!(accepted, 1);
}
