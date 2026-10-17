use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn bundled_ui_and_empty_api_load_without_queues() {
    let router = tasks::board::Board::new().router();
    let index = router
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    assert!(
        index.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("text/html")
    );
    for endpoint in ["queues", "tasks", "workers", "overview"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/{endpoint}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "[]"
        );
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Fixture {
    value: String,
}

#[sqlx::test(migrations = false)]
#[ignore = "run by integration_suite"]
async fn submissions_keep_each_registered_queue_attempt_budget(
    _options: sqlx::postgres::PgPoolOptions,
    options: sqlx::postgres::PgConnectOptions,
) {
    let db = test_support::TestDatabase::provision(options).await;
    db.migrate().await;
    let pool = sqlx::PgPool::connect(&db.tasks_url).await.unwrap();
    let queue = |name, max_attempts| {
        tasks::Queue::<Fixture>::new(
            pool.clone(),
            name,
            tasks::WorkerConfig {
                max_attempts,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let two = queue("fixture-two", 2);
    let five = queue("fixture-five", 5);
    let router = tasks::board::Board::new()
        .register(&two)
        .unwrap()
        .register(&five)
        .unwrap()
        .router();
    for (name, header, expected) in [
        ("fixture-two", None, Some(2)),
        ("fixture-two", Some("20"), Some(2)),
        ("fixture-two", Some("1"), Some(1)),
        ("fixture-two", Some("0"), None),
        ("fixture-two", Some("bad"), None),
        ("fixture-five", None, Some(5)),
        ("fixture-five", Some("20"), Some(5)),
    ] {
        let value = format!("{name}-{}", header.unwrap_or("default"));
        let mut request = Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/queues/{name}/tasks"))
            .header("content-type", "application/json");
        if let Some(header) = header {
            request = request.header("x-task-max-attempts", header);
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::json!({"value":value}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if expected.is_some() { 200 } else { 400 },
            "{value}"
        );
        let stored: Option<i32> = sqlx::query_scalar("SELECT max_attempts FROM apalis.jobs WHERE job_type=$1 AND convert_from(job,'UTF8')::jsonb->>'value'=$2").bind(name).bind(&value).fetch_optional(&pool).await.unwrap();
        assert_eq!(stored, expected, "{value}");
    }
    pool.close().await;
}
