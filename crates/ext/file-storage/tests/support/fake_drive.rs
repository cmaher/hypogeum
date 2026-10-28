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
pub const SHORTCUT: &str = "application/vnd.google-apps.shortcut";

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
    pub final_partial_ack: Option<usize>,
    pub delete_failure: bool,
    pub foreign_location: bool,
    pub ignore_range: bool,
    pub unsatisfiable_range: bool,
    pub media_status: Option<(u16, &'static str)>,
    pub range_status: Option<(u16, &'static str)>,
    pub wrong_content_range: bool,
    pub short_range_body: bool,
    pub long_range_body: bool,
}

struct File {
    name: String,
    parents: Vec<String>,
    trashed: bool,
    mime: String,
    bytes: Vec<u8>,
}
struct Session {
    name: String,
    parents: Vec<String>,
    /// The existing file a replacement upload installs into.
    target: Option<String>,
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
    metadata_requests: usize,
    list_requests: usize,
    queries: Vec<String>,
    folders_created: usize,
    media_ranges: Vec<String>,
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
                name: "storage".into(),
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
        parent: &str,
        name: &str,
        trashed: bool,
        mime: &str,
        bytes: Vec<u8>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.data.lock().unwrap().files.insert(
            id.clone(),
            File {
                name: name.into(),
                parents: vec![parent.into()],
                trashed,
                mime: mime.into(),
                bytes,
            },
        );
        id
    }
    pub fn insert_folder(&self, parent: &str, name: &str) -> String {
        self.insert_file(parent, name, false, FOLDER, vec![])
    }
    /// The ID of the single live item named by `path` below the storage folder;
    /// the last segment may name a file or a folder.
    pub fn id_at(&self, path: &str) -> Option<String> {
        let data = self.data.lock().unwrap();
        let mut parent = "folder".to_owned();
        let segments: Vec<&str> = path.split('/').collect();
        for (index, segment) in segments.iter().enumerate() {
            let folder = index + 1 < segments.len();
            let mut found = data.files.iter().filter(|(_, file)| {
                !file.trashed
                    && file.name == *segment
                    && file.parents.contains(&parent)
                    && (!folder || file.mime == FOLDER)
            });
            let (id, _) = found.next()?;
            assert!(found.next().is_none(), "duplicate {segment}");
            parent = id.clone();
        }
        Some(parent)
    }
    pub fn remove(&self, id: &str) {
        self.data.lock().unwrap().files.remove(id);
    }
    pub fn set_parents(&self, id: &str, parents: Vec<String>) {
        self.data.lock().unwrap().files.get_mut(id).unwrap().parents = parents;
    }
    pub fn set_content(&self, id: &str, bytes: Vec<u8>) {
        self.data.lock().unwrap().files.get_mut(id).unwrap().bytes = bytes;
    }
    pub fn set_trashed(&self, id: &str, trashed: bool) {
        self.data.lock().unwrap().files.get_mut(id).unwrap().trashed = trashed;
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
    pub fn metadata_requests(&self) -> usize {
        self.data.lock().unwrap().metadata_requests
    }
    pub fn list_requests(&self) -> usize {
        self.data.lock().unwrap().list_requests
    }
    /// `q` parameters of list requests, decoded.
    pub fn queries(&self) -> Vec<String> {
        self.data.lock().unwrap().queries.clone()
    }
    pub fn folders_created(&self) -> usize {
        self.data.lock().unwrap().folders_created
    }
    /// `Range` headers sent on media downloads.
    pub fn media_ranges(&self) -> Vec<String> {
        self.data.lock().unwrap().media_ranges.clone()
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
    let upload_target = match (method, path.strip_prefix("/upload/drive/v3/files")) {
        ("POST", Some("")) => Some(None),
        ("PATCH", Some(id)) if id.starts_with('/') => Some(Some(id[1..].to_owned())),
        _ => None,
    };
    if let Some(target) = upload_target {
        assert!(parts.uri.query().unwrap().contains("uploadType=resumable"));
        assert_eq!(
            parts.headers["x-upload-content-type"],
            "application/octet-stream"
        );
        let metadata: Value = serde_json::from_slice(&body).unwrap();
        let session = match &target {
            None => {
                let parents: Vec<String> =
                    serde_json::from_value(metadata["parents"].clone()).unwrap();
                if !parents.iter().all(|parent| data.files.contains_key(parent)) {
                    return failure(404, "notFound");
                }
                Session {
                    name: metadata["name"].as_str().unwrap().into(),
                    parents,
                    target: None,
                    bytes: vec![],
                    completed: None,
                }
            }
            Some(id) => {
                assert_eq!(metadata, json!({}));
                let Some(file) = data.files.get(id) else {
                    return failure(404, "notFound");
                };
                Session {
                    name: file.name.clone(),
                    parents: file.parents.clone(),
                    target: target.clone(),
                    bytes: vec![],
                    completed: None,
                }
            }
        };
        let id = uuid::Uuid::new_v4().to_string();
        data.sessions.insert(id.clone(), session);
        let root = if std::mem::take(&mut data.faults.foreign_location) {
            "https://foreign.invalid/"
        } else {
            components.root.as_str()
        };
        return (
            StatusCode::OK,
            [(header::LOCATION, format!("{root}upload/session/{id}"))],
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
        if let Some(accepted) = data.faults.final_partial_ack.take() {
            let session = data.sessions.get_mut(id).unwrap();
            session
                .bytes
                .extend_from_slice(&body[..accepted.min(body.len())]);
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
        let name = session.name.clone();
        let target = session.target.clone();
        let file_id = target
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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
        match target.and_then(|id| data.files.get_mut(&id)) {
            Some(file) => file.bytes = bytes,
            None => {
                data.files.insert(
                    file_id,
                    File {
                        name,
                        parents,
                        trashed: false,
                        mime: "application/octet-stream".into(),
                        bytes,
                    },
                );
            }
        }
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
    if path == "/drive/v3/files" && method == "GET" {
        data.list_requests += 1;
        let query: HashMap<String, String> = url_query(parts.uri.query().unwrap_or_default());
        assert_eq!(query["corpora"], "allDrives");
        let q = query["q"].clone();
        data.queries.push(q.clone());
        let clauses = parse_query(&q);
        let files: Vec<Value> = data
            .files
            .iter()
            .filter(|(_, file)| clauses.iter().all(|clause| clause.matches(file)))
            .map(|(id, file)| json!({"id": id, "mimeType": file.mime, "size": file.bytes.len().to_string()}))
            .collect();
        return Json(json!({"files": files, "incompleteSearch": false})).into_response();
    }
    if path == "/drive/v3/files" && method == "POST" {
        let metadata: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(metadata["mimeType"], FOLDER);
        let parents: Vec<String> = serde_json::from_value(metadata["parents"].clone()).unwrap();
        if !parents.iter().all(|parent| data.files.contains_key(parent)) {
            return failure(404, "notFound");
        }
        let id = uuid::Uuid::new_v4().to_string();
        data.folders_created += 1;
        data.files.insert(
            id.clone(),
            File {
                name: metadata["name"].as_str().unwrap().into(),
                parents,
                trashed: false,
                mime: FOLDER.into(),
                bytes: vec![],
            },
        );
        return Json(json!({"id": id})).into_response();
    }
    if let Some(id) = path.strip_prefix("/drive/v3/files/") {
        if method == "DELETE" {
            if std::mem::take(&mut data.faults.delete_failure) {
                return failure(503, "backendError");
            }
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
                if let Some((code, reason)) = data.faults.media_status.take() {
                    return failure(code, reason);
                }
                let Some(range) = parts.headers.get(header::RANGE) else {
                    return Response::new(Body::from(bytes));
                };
                let range = range.to_str().unwrap().to_owned();
                data.media_ranges.push(range.clone());
                if std::mem::take(&mut data.faults.ignore_range) {
                    return Response::new(Body::from(bytes));
                }
                let (start, end) = range
                    .strip_prefix("bytes=")
                    .and_then(|span| span.split_once('-'))
                    .map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()))
                    .unwrap();
                if std::mem::take(&mut data.faults.unsatisfiable_range) || start >= bytes.len() {
                    return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                }
                if let Some((code, reason)) = data.faults.range_status.take() {
                    return failure(code, reason);
                }
                let end = end.min(bytes.len() - 1);
                let label_start = if std::mem::take(&mut data.faults.wrong_content_range) {
                    start + 1
                } else {
                    start
                };
                let mut body = bytes[start..=end].to_vec();
                if std::mem::take(&mut data.faults.short_range_body) {
                    body.pop();
                }
                if std::mem::take(&mut data.faults.long_range_body) {
                    body.push(0);
                }
                return (
                    StatusCode::PARTIAL_CONTENT,
                    [(
                        header::CONTENT_RANGE,
                        format!("bytes {label_start}-{end}/{}", bytes.len()),
                    )],
                    body,
                )
                    .into_response();
            }
            data.metadata_requests += 1;
            let file = &data.files[id];
            return Json(json!({"id": id, "parents": file.parents, "trashed": file.trashed, "mimeType": file.mime, "size": file.bytes.len().to_string()})).into_response();
        }
    }
    failure(404, "notFound")
}

fn url_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                out.push(u8::from_str_radix(&value[index + 1..index + 3], 16).unwrap());
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

enum Clause {
    Name(String),
    Parent(String),
    NotTrashed,
    Mime { equal: bool, mime: String },
}

impl Clause {
    fn matches(&self, file: &File) -> bool {
        match self {
            Clause::Name(name) => file.name == *name,
            Clause::Parent(parent) => file.parents.contains(parent),
            Clause::NotTrashed => !file.trashed,
            Clause::Mime { equal, mime } => (file.mime == *mime) == *equal,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Literal(String),
}

/// Parses exactly the conjunctions the adapter sends, failing loudly otherwise.
fn parse_query(q: &str) -> Vec<Clause> {
    let mut tokens = Vec::new();
    let mut chars = q.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else if c == '\'' {
            chars.next();
            let mut literal = String::new();
            loop {
                match chars.next().expect("unterminated literal") {
                    '\\' => literal.push(chars.next().expect("dangling escape")),
                    '\'' => break,
                    c => literal.push(c),
                }
            }
            tokens.push(Token::Literal(literal));
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            tokens.push(Token::Word(word));
        }
    }
    let word = |w: &str| Token::Word(w.into());
    let mut clauses = Vec::new();
    let mut tokens = tokens.into_iter();
    loop {
        let clause = match tokens.next().expect("clause") {
            Token::Literal(parent) => {
                assert_eq!(tokens.next(), Some(word("in")));
                assert_eq!(tokens.next(), Some(word("parents")));
                Clause::Parent(parent)
            }
            Token::Word(field) if field == "name" => {
                assert_eq!(tokens.next(), Some(word("=")));
                let Some(Token::Literal(name)) = tokens.next() else {
                    panic!("name literal")
                };
                Clause::Name(name)
            }
            Token::Word(field) if field == "trashed" => {
                assert_eq!(tokens.next(), Some(word("=")));
                assert_eq!(tokens.next(), Some(word("false")));
                Clause::NotTrashed
            }
            Token::Word(field) if field == "mimeType" => {
                let equal = match tokens.next() {
                    Some(Token::Word(op)) if op == "=" => true,
                    Some(Token::Word(op)) if op == "!=" => false,
                    other => panic!("mimeType operator {other:?}"),
                };
                let Some(Token::Literal(mime)) = tokens.next() else {
                    panic!("mime literal")
                };
                Clause::Mime { equal, mime }
            }
            other => panic!("unsupported query token {other:?}"),
        };
        clauses.push(clause);
        match tokens.next() {
            None => return clauses,
            Some(token) => assert_eq!(token, word("and")),
        }
    }
}
