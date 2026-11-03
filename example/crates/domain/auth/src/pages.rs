//! Self-service flow pages rendered from Kratos UI nodes, plus logout.
use askama::Template;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use hypgm_errors::PublicErrorInfo;
use hypgm_view::{html_error_response, render_template};
use hypgmx_kratos::{Flow, FlowKind, KratosError, UiNodeAttributes, UiText, session_cookie};
use serde::Deserialize;
use uuid::Uuid;

use crate::{AuthComponents, PendingAccount};
use hypgm_authn::{OptionalAuth, Principal};

pub fn router(components: AuthComponents) -> Router<()> {
    Router::new()
        .route("/login", get(login))
        .route("/registration", get(registration))
        .route("/settings", get(settings))
        .route("/error", get(error))
        .route("/registration-pending", get(pending))
        .route("/logout", post(logout))
        .with_state(components)
}

#[derive(Deserialize)]
struct FlowQuery {
    flow: Option<String>,
}

#[derive(Deserialize)]
struct ErrorQuery {
    id: Option<String>,
}

struct Message {
    kind: String,
    text: String,
}

impl From<UiText> for Message {
    fn from(text: UiText) -> Self {
        Self {
            kind: text.kind,
            text: text.text,
        }
    }
}

struct Field {
    name: String,
    input_type: String,
    value: Option<String>,
    required: bool,
    disabled: bool,
    autocomplete: Option<String>,
    label: String,
    messages: Vec<Message>,
}

/// One `<form>` per method group (`password`, `profile`, ...); every form
/// also carries the `default` group, which holds the CSRF token.
struct Form {
    action: String,
    method: String,
    fields: Vec<Field>,
}

#[derive(Template)]
#[template(path = "flow.html")]
struct FlowPage {
    kind: &'static str,
    title: &'static str,
    messages: Vec<Message>,
    forms: Vec<Form>,
}

#[derive(Template)]
#[template(path = "notice.html")]
struct NoticePage<'a> {
    title: &'a str,
    message: &'a str,
    signed_in: bool,
}

/// Signed in, approved or pending: the notice offers settings and logout.
fn signed_in(principal: Option<Principal>, pending: Option<Extension<PendingAccount>>) -> bool {
    principal.is_some() || pending.is_some()
}

async fn login(
    components: State<AuthComponents>,
    query: Query<FlowQuery>,
    headers: HeaderMap,
) -> Response {
    flow_page(components, query, headers, FlowKind::Login, "Log in").await
}

async fn registration(
    components: State<AuthComponents>,
    query: Query<FlowQuery>,
    headers: HeaderMap,
) -> Response {
    flow_page(
        components,
        query,
        headers,
        FlowKind::Registration,
        "Register",
    )
    .await
}

async fn settings(
    components: State<AuthComponents>,
    query: Query<FlowQuery>,
    headers: HeaderMap,
) -> Response {
    flow_page(components, query, headers, FlowKind::Settings, "Settings").await
}

async fn flow_page(
    State(components): State<AuthComponents>,
    Query(query): Query<FlowQuery>,
    headers: HeaderMap,
    kind: FlowKind,
    title: &'static str,
) -> Response {
    let browser = &components.browser_url;
    // No (or a malformed) flow id: let Kratos start a browser flow, which
    // redirects back here with `?flow=<id>`.
    let Some(id) = query.flow.and_then(|flow| Uuid::parse_str(&flow).ok()) else {
        let start = format!("self-service/{}/browser", kind.as_str());
        return Redirect::to(&browser.join(&start)).into_response();
    };
    let cookie = cookie_header(&headers);
    let flow = match components.flows.get_flow(kind, id, cookie.as_ref()).await {
        Ok(flow) => flow,
        Err(KratosError::FlowGone) => {
            return Redirect::to(&format!("/{}", kind.as_str())).into_response();
        }
        Err(KratosError::Unauthenticated) => return Redirect::to("/login").into_response(),
        Err(error) => {
            tracing::error!(%error, flow = kind.as_str(), "Flow lookup failed");
            return internal_error();
        }
    };
    if !browser.contains(&flow.ui.action) {
        tracing::error!(
            flow = kind.as_str(),
            "Flow action is outside the identity service"
        );
        return internal_error();
    }
    render_template(
        StatusCode::OK,
        &FlowPage {
            kind: kind.as_str(),
            title,
            messages: flow
                .ui
                .messages
                .iter()
                .cloned()
                .map(Message::from)
                .collect(),
            forms: forms(flow),
        },
    )
}

/// All `Cookie` headers joined, as Kratos expects for its CSRF check.
fn cookie_header(headers: &HeaderMap) -> Option<HeaderValue> {
    let values: Vec<&[u8]> = headers
        .get_all(header::COOKIE)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect();
    if values.is_empty() {
        return None;
    }
    HeaderValue::from_bytes(&values.join(&b"; "[..])).ok()
}

fn forms(flow: Flow) -> Vec<Form> {
    let mut groups: Vec<&str> = Vec::new();
    for node in &flow.ui.nodes {
        if node.group != "default" && !groups.contains(&node.group.as_str()) {
            groups.push(&node.group);
        }
    }
    if groups.is_empty() {
        groups.push("default");
    }
    groups
        .into_iter()
        .map(|group| Form {
            action: flow.ui.action.clone(),
            method: flow.ui.method.clone(),
            fields: flow
                .ui
                .nodes
                .iter()
                .filter(|node| node.group == group || node.group == "default")
                .filter_map(|node| {
                    let UiNodeAttributes::Input(input) = &node.attributes else {
                        return None;
                    };
                    Some(Field {
                        name: input.name.clone(),
                        input_type: input.input_type.clone(),
                        value: input.value_text(),
                        required: input.required,
                        disabled: input.disabled,
                        autocomplete: input.autocomplete.clone(),
                        label: node
                            .meta
                            .label
                            .as_ref()
                            .map_or_else(|| input.name.clone(), |label| label.text.clone()),
                        messages: node.messages.iter().cloned().map(Message::from).collect(),
                    })
                })
                .collect(),
        })
        .collect()
}

async fn error(
    State(components): State<AuthComponents>,
    OptionalAuth(principal): OptionalAuth,
    pending: Option<Extension<PendingAccount>>,
    Query(query): Query<ErrorQuery>,
) -> Response {
    let fallback = "Something went wrong. Please try again.";
    let id = query.id.and_then(|id| Uuid::parse_str(&id).ok());
    let detail = match id {
        Some(id) => components.flows.get_error(id).await.ok(),
        None => None,
    };
    let message = detail
        .and_then(|detail| detail.error.message)
        .unwrap_or_else(|| fallback.to_owned());
    render_template(
        StatusCode::OK,
        &NoticePage {
            title: "Error",
            message: &message,
            signed_in: signed_in(principal, pending),
        },
    )
}

async fn pending(
    OptionalAuth(principal): OptionalAuth,
    pending: Option<Extension<PendingAccount>>,
) -> Response {
    render_template(
        StatusCode::OK,
        &NoticePage {
            title: "Registration pending",
            message: "Registration received. An administrator must approve your account before you can continue.",
            signed_in: signed_in(principal, pending),
        },
    )
}

async fn logout(State(components): State<AuthComponents>, headers: HeaderMap) -> Response {
    let Some(cookie) = session_cookie(&headers) else {
        return Redirect::to("/login").into_response();
    };
    match components.sessions.logout_url(&cookie).await {
        Ok(url) if components.browser_url.contains(&url) => Redirect::to(&url).into_response(),
        Ok(_) => {
            tracing::error!("Logout URL is outside the identity service");
            internal_error()
        }
        Err(KratosError::Unauthenticated) => Redirect::to("/login").into_response(),
        Err(error) => {
            tracing::error!(%error, "Logout failed");
            internal_error()
        }
    }
}

fn internal_error() -> Response {
    html_error_response(PublicErrorInfo::INTERNAL_ERROR)
}
