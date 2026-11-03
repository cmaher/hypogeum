//! Typed subsets of Kratos v1 public-API responses. Unknown fields are ignored.

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// A Kratos session as returned by `GET /sessions/whoami`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub active: bool,
    pub identity: Identity,
}

/// A Kratos identity. Traits follow the configured identity schema.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Identity {
    pub id: Uuid,
    pub traits: Value,
}

/// The self-service flows whose UI this client fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Login,
    Registration,
    Settings,
}

impl FlowKind {
    /// Path segment used by Kratos (`/self-service/<segment>/...`).
    pub const fn as_str(self) -> &'static str {
        match self {
            FlowKind::Login => "login",
            FlowKind::Registration => "registration",
            FlowKind::Settings => "settings",
        }
    }
}

/// A login, registration or settings flow. Only the UI is modelled.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Flow {
    pub ui: UiContainer,
}

/// The form a flow asks the browser to render and submit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UiContainer {
    pub action: String,
    pub method: String,
    #[serde(default)]
    pub nodes: Vec<UiNode>,
    #[serde(default)]
    pub messages: Vec<UiText>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UiNode {
    /// Method group, e.g. `default`, `password` or `profile`.
    pub group: String,
    pub attributes: UiNodeAttributes,
    #[serde(default)]
    pub messages: Vec<UiText>,
    #[serde(default)]
    pub meta: UiNodeMeta,
}

/// Node attributes, discriminated by `node_type`. Only inputs are modelled.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "node_type", rename_all = "lowercase")]
pub enum UiNodeAttributes {
    Input(UiNodeInputAttributes),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UiNodeInputAttributes {
    pub name: String,
    /// HTML input type (`text`, `password`, `hidden`, `submit`, ...).
    #[serde(rename = "type")]
    pub input_type: String,
    pub value: Option<Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub disabled: bool,
    pub autocomplete: Option<String>,
}

impl UiNodeInputAttributes {
    /// The value as form text: strings verbatim, numbers/booleans formatted,
    /// `null`/absent as `None`.
    pub fn value_text(&self) -> Option<String> {
        match self.value.as_ref()? {
            Value::String(text) => Some(text.clone()),
            other => Some(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UiNodeMeta {
    pub label: Option<UiText>,
}

/// A UI message or label (`type` is `info`, `error` or `success`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UiText {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

/// A self-service error from `GET /self-service/errors`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FlowError {
    #[serde(default)]
    pub error: FlowErrorDetail,
}

/// User-facing part of a self-service error. `reason` and `debug` are
/// deliberately not parsed because they can carry internal detail.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FlowErrorDetail {
    pub message: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LogoutFlow {
    pub logout_url: String,
}
