//! Upstream Board API and bundled UI, with a clean empty registration state.
use crate::Queue;
use apalis_board::axum::{
    framework::{ApiBuilder, RegisterRoute},
    ui::ServeUI,
};
use axum::{
    Json, Router,
    extract::Request,
    http::{HeaderValue, Method, StatusCode},
    middleware::{Next, from_fn},
    response::IntoResponse,
    routing::get,
};
use hypgm_errors::TasksError;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashSet;

pub struct Board {
    api: Router,
    queues: HashSet<String>,
}
impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}
impl Board {
    pub fn new() -> Self {
        Self {
            api: Router::new(),
            queues: HashSet::new(),
        }
    }
    pub fn register<Args>(mut self, queue: &Queue<Args>) -> Result<Self, TasksError>
    where
        Args: Serialize + DeserializeOwned + Unpin + Send + Sync + 'static,
    {
        let storage = queue.storage();
        let name = storage.config().queue.to_string();
        let register_root = self.queues.is_empty();
        if !self.queues.insert(name.clone()) {
            return Err(TasksError::Config(
                "Duplicate Board queue registration".into(),
            ));
        }
        let max_attempts = queue.config().max_attempts;
        let api = RegisterRoute::<_, Args>::register(
            ApiBuilder::new_with_router(Router::new(), register_root),
            &name,
            storage,
        )
        .build()
        .layer(from_fn(move |request: Request, next: Next| {
            enforce_submission_limit(request, next, max_attempts)
        }));
        self.api = self.api.merge(api);
        Ok(self)
    }
    pub fn router(self) -> Router {
        let api = if self.queues.is_empty() {
            let mut router = self.api;
            for path in ["/queues", "/tasks", "/workers", "/overview"] {
                router = router.route(path, get(empty_list));
            }
            router
        } else {
            self.api
        };
        Router::new()
            .nest("/api/v1", api)
            .fallback_service(ServeUI::new())
    }
}

fn submission_attempts(header: Option<&HeaderValue>, max_attempts: usize) -> Result<usize, ()> {
    let Some(header) = header else {
        return Ok(max_attempts);
    };
    let attempts = header
        .to_str()
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .ok_or(())?;
    Ok(attempts.min(max_attempts))
}

async fn empty_list() -> Json<Vec<serde_json::Value>> {
    Json(Vec::new())
}

async fn enforce_submission_limit(
    mut request: Request,
    next: Next,
    max_attempts: usize,
) -> axum::response::Response {
    if request.method() == Method::PUT {
        let attempts =
            match submission_attempts(request.headers().get("x-task-max-attempts"), max_attempts) {
                Ok(attempts) => attempts,
                Err(()) => {
                    return (StatusCode::BAD_REQUEST, "Invalid task attempt limit").into_response();
                }
            };
        request.headers_mut().insert(
            "x-task-max-attempts",
            HeaderValue::from_str(&attempts.to_string()).expect("numeric attempt limit"),
        );
    }
    next.run(request).await
}
