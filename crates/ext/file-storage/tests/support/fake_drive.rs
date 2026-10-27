use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub const FOLDER: &str = "application/vnd.google-apps.folder";

#[derive(Default)]
pub struct Faults {
    pub next_status: Option<(u16, &'static str)>,
    pub partial_ack: Option<usize>,
    pub intermediate_failure: bool,
    pub final_failure: Option<bool>, // Whether the failed final PUT completed.
    pub query_failure: bool,
    pub bad_checksum: bool,
    pub omit_checksum: bool,
    pub bad_size: bool,
    pub delete_disconnect: bool,
    pub final_disconnect: Option<bool>,
    pub query_disconnect: bool,
    pub malformed_final: bool,
    pub final_body_failure: bool,
}

struct File {
    parents: Vec<String>,
    trashed: bool,
    mime: String,
    bytes: Vec<u8>,
}
struct Session {
    parents: Vec<String>,
    bytes: Vec<u8>,
    completed: Option<Value>,
}
#[derive(Default)]
struct Data {
    files: HashMap<String, File>,
    sessions: HashMap<String, Session>,
    faults: Faults,
    cancelled: usize,
    violations: usize,
    requests: usize,
    media_requests: usize,
    ranges: Vec<String>,
    deleted: usize,
}

#[derive(Clone)]
struct Components {
    data: Arc<Mutex<Data>>,
    root: String,
}

pub struct FakeDrive {
    pub root_url: String,
    data: Arc<Mutex<Data>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FakeDrive {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeDrive {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root_url = format!("http://{}/", listener.local_addr().unwrap());
        let data = Arc::new(Mutex::new(Data::default()));
        data.lock().unwrap().files.insert(
            "folder".into(),
            File {
                parents: vec![],
                trashed: false,
                mime: FOLDER.into(),
                bytes: vec![],
            },
        );
        let router = Router::new().fallback(handle).with_state(Components {
            data: data.clone(),
            root: root_url.clone(),
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            root_url,
            data,
            task,
        }
    }
    pub fn faults(&self, update: impl FnOnce(&mut Faults)) {
        update(&mut self.data.lock().unwrap().faults);
    }
    pub fn insert_file(
        &self,
        parents: Vec<String>,
        trashed: bool,
        mime: &str,
        bytes: Vec<u8>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.data.lock().unwrap().files.insert(
            id.clone(),
            File {
                parents,
                trashed,
                mime: mime.into(),
                bytes,
            },
        );
        id
    }
    pub fn insert_folder(&self) -> String {
        self.insert_file(vec![], false, FOLDER, vec![])
    }
    pub fn file_count(&self) -> usize {
        self.data
            .lock()
            .unwrap()
            .files
            .values()
            .filter(|f| f.mime != FOLDER)
            .count()
    }
    pub fn content(&self, id: &str) -> Option<Vec<u8>> {
        self.data
            .lock()
            .unwrap()
            .files
            .get(id)
            .map(|f| f.bytes.clone())
    }
    pub fn cancelled_sessions(&self) -> usize {
        self.data.lock().unwrap().cancelled
    }
    pub fn violations(&self) -> usize {
        self.data.lock().unwrap().violations
    }
    pub fn requests(&self) -> usize {
        self.data.lock().unwrap().requests
    }
    pub fn media_requests(&self) -> usize {
        self.data.lock().unwrap().media_requests
    }
    pub fn ranges(&self) -> Vec<String> {
        self.data.lock().unwrap().ranges.clone()
    }
    pub fn deleted(&self) -> usize {
        self.data.lock().unwrap().deleted
    }
}

fn failure(code: u16, reason: &str) -> Response {
    (StatusCode::from_u16(code).unwrap(), Json(json!({"error": {"code": code, "message": "secret-token-and-session", "errors": [{"reason": reason}]}}))).into_response()
}
fn resume(count: usize) -> Response {
    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();
    if count > 0 {
        response.headers_mut().insert(
            header::RANGE,
            format!("bytes=0-{}", count - 1).parse().unwrap(),
        );
    }
    response
}

async fn handle(State(components): State<Components>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 40 * 1024 * 1024).await.unwrap();
    let method = parts.method.as_str();
    let path = parts.uri.path();
    let mut data = components.data.lock().unwrap();
    data.requests += 1;
    if parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        != Some("Bearer test-token")
    {
        return failure(401, "authError");
    }
    if let Some((code, reason)) = data.faults.next_status.take() {
        return failure(code, reason);
    }
    if path == "/upload/drive/v3/files" && method == "POST" {
        assert!(parts.uri.query().unwrap().contains("uploadType=resumable"));
        assert_eq!(
            parts.headers["x-upload-content-type"],
            "application/octet-stream"
        );
        let metadata: Value = serde_json::from_slice(&body).unwrap();
        assert!(uuid::Uuid::parse_str(metadata["name"].as_str().unwrap()).is_ok());
        let id = uuid::Uuid::new_v4().to_string();
        data.sessions.insert(
            id.clone(),
            Session {
                parents: serde_json::from_value(metadata["parents"].clone()).unwrap(),
                bytes: vec![],
                completed: None,
            },
        );
        return (
            StatusCode::OK,
            [(
                header::LOCATION,
                format!("{}upload/session/{id}", components.root),
            )],
        )
            .into_response();
    }
    if let Some(id) = path.strip_prefix("/upload/session/") {
        if method == "DELETE" {
            data.sessions.remove(id);
            data.cancelled += 1;
            return StatusCode::from_u16(499).unwrap().into_response();
        }
        if method != "PUT" {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let range = parts.headers[header::CONTENT_RANGE].to_str().unwrap();
        data.ranges.push(range.to_owned());
        let range = range.strip_prefix("bytes ").unwrap();
        let (span, total) = range.split_once('/').unwrap();
        if span == "*"
            && (total != "0"
                || data
                    .sessions
                    .get(id)
                    .is_some_and(|s| !s.bytes.is_empty() || s.completed.is_some()))
        {
            if std::mem::take(&mut data.faults.query_failure) {
                return failure(503, "backendError");
            }
            if std::mem::take(&mut data.faults.query_disconnect) {
                drop(data);
                panic!("injected lost status response");
            }
            let Some(session) = data.sessions.get(id) else {
                return failure(404, "notFound");
            };
            return match &session.completed {
                Some(file) => Json(file.clone()).into_response(),
                None => resume(session.bytes.len()),
            };
        }
        let Some(session) = data.sessions.get(id) else {
            return failure(404, "notFound");
        };
        let (start, end) = if span == "*" {
            (0, 0)
        } else {
            let (a, b) = span.split_once('-').unwrap();
            (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap())
        };
        if start != session.bytes.len() || (!body.is_empty() && end + 1 - start != body.len()) {
            return failure(400, "invalidRange");
        }
        if total == "*" {
            if body.len() % (256 * 1024) != 0 || body.is_empty() {
                data.violations += 1;
                return failure(400, "invalidChunk");
            }
            if std::mem::take(&mut data.faults.intermediate_failure) {
                return failure(503, "backendError");
            }
            let accepted = data
                .faults
                .partial_ack
                .take()
                .unwrap_or(body.len())
                .min(body.len());
            let session = data.sessions.get_mut(id).unwrap();
            session.bytes.extend_from_slice(&body[..accepted]);
            return resume(session.bytes.len());
        }
        let final_failure = data.faults.final_failure.take();
        let disconnect = data.faults.final_disconnect.take();
        if final_failure == Some(false) {
            return failure(503, "backendError");
        }
        if disconnect == Some(false) {
            drop(data);
            panic!("injected lost final response");
        }
        let session = data.sessions.get_mut(id).unwrap();
        session.bytes.extend_from_slice(&body);
        assert_eq!(session.bytes.len(), total.parse::<usize>().unwrap());
        let bytes = session.bytes.clone();
        let parents = session.parents.clone();
        let file_id = uuid::Uuid::new_v4().to_string();
        let mut metadata = json!({"id": file_id, "size": bytes.len().to_string(), "sha256Checksum": hex::encode(Sha256::digest(&bytes))});
        if std::mem::take(&mut data.faults.bad_checksum) {
            metadata["sha256Checksum"] = json!("0".repeat(64));
        }
        if std::mem::take(&mut data.faults.omit_checksum) {
            metadata.as_object_mut().unwrap().remove("sha256Checksum");
        }
        if std::mem::take(&mut data.faults.bad_size) {
            metadata["size"] = json!("999999999");
        }
        data.files.insert(
            file_id,
            File {
                parents,
                trashed: false,
                mime: "application/octet-stream".into(),
                bytes,
            },
        );
        data.sessions.get_mut(id).unwrap().completed = Some(metadata.clone());
        if disconnect == Some(true) {
            drop(data);
            panic!("injected lost final response");
        }
        if final_failure == Some(true) {
            return failure(503, "backendError");
        }
        if std::mem::take(&mut data.faults.final_body_failure) {
            use futures_util::{StreamExt, stream};
            let body =
                stream::once(async { Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"{")) })
                    .chain(stream::once(async {
                        tokio::task::yield_now().await;
                        Err(std::io::Error::other("injected truncated response"))
                    }));
            return Response::new(Body::from_stream(body));
        }
        if std::mem::take(&mut data.faults.malformed_final) {
            return (StatusCode::OK, "invalid-json").into_response();
        }
        return Json(metadata).into_response();
    }
    if let Some(id) = path.strip_prefix("/drive/v3/files/") {
        if method == "DELETE" {
            let removed = data.files.remove(id).is_some();
            data.deleted += 1;
            if std::mem::take(&mut data.faults.delete_disconnect) {
                drop(data);
                panic!("injected lost delete response");
            }
            return if removed {
                StatusCode::NO_CONTENT.into_response()
            } else {
                failure(404, "notFound")
            };
        }
        if method == "GET" {
            let Some(file) = data.files.get(id) else {
                return failure(404, "notFound");
            };
            if parts.uri.query().is_some_and(|q| q.contains("alt=media")) {
                let bytes = file.bytes.clone();
                data.media_requests += 1;
                return Response::new(Body::from(bytes));
            }
            return Json(json!({"id": id, "parents": file.parents, "trashed": file.trashed, "mimeType": file.mime, "size": file.bytes.len().to_string()})).into_response();
        }
    }
    failure(404, "notFound")
}
