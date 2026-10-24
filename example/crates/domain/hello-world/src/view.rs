//! Server-rendered greeting pages with progressive HTMX enhancement.
//!
//! The form posts to `/hello` with or without JavaScript. HTMX requests
//! receive the greeting section; ordinary submissions receive the full page.
//! Greeting is non-mutating, so POST renders directly instead of redirecting.

use crate::controller::HelloComponents;
use crate::service::HelloWorldError;
use askama::Template;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use hypgm_errors::{PublicError, PublicErrorInfo};
use hypgm_view::{
    AppForm, HTMX_SCRIPT_URL, HtmlError, HtmxRequest, VIEW_SCRIPT_URL, ViewMode, html_response,
    render_failure_response, with_classification,
};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct GreetingForm {
    #[serde(default)]
    pub name: String,
}

#[derive(Template)]
#[template(path = "page.html")]
struct PageTemplate<'a> {
    htmx_script_url: &'static str,
    view_script_url: &'static str,
    name: &'a str,
    message: &'a str,
    error: &'a str,
}

#[derive(Template)]
#[template(path = "greeting.html")]
struct GreetingTemplate<'a> {
    name: &'a str,
    message: &'a str,
    error: &'a str,
}

#[derive(Default)]
struct GreetingView {
    name: String,
    message: String,
    error: &'static str,
}

pub fn router(components: HelloComponents) -> Router<()> {
    Router::new()
        .route("/", get(page))
        .route("/hello", post(greet))
        .with_state(components)
}

async fn page() -> Response {
    render(ViewMode::FullPage, None, &GreetingView::default())
}

async fn greet(
    State(components): State<HelloComponents>,
    htmx: HtmxRequest,
    AppForm(form): AppForm<GreetingForm>,
) -> Result<Response, HtmlError> {
    match components.hello_service.say_hello(&form.name).await {
        Ok(message) => Ok(render(
            htmx.mode,
            None,
            &GreetingView {
                name: form.name,
                message,
                error: "",
            },
        )),
        Err(err @ HelloWorldError::Validation(message)) => Ok(render(
            htmx.mode,
            Some(err.public_error()),
            &GreetingView {
                name: form.name,
                message: String::new(),
                error: message,
            },
        )),
        Err(err) => Err(HtmlError::from(err)),
    }
}

fn render(mode: ViewMode, error: Option<PublicErrorInfo>, view: &GreetingView) -> Response {
    match mode {
        ViewMode::FullPage => respond(
            error,
            &PageTemplate {
                htmx_script_url: HTMX_SCRIPT_URL,
                view_script_url: VIEW_SCRIPT_URL,
                name: &view.name,
                message: &view.message,
                error: view.error,
            },
        ),
        ViewMode::Fragment => respond(
            error,
            &GreetingTemplate {
                name: &view.name,
                message: &view.message,
                error: view.error,
            },
        ),
    }
}

/// Renders the template directly so the body can be post-processed before
/// it is sent; `render_failure_response` keeps failures safe and logged.
fn respond<T: Template>(error: Option<PublicErrorInfo>, template: &T) -> Response {
    let body = match template.render() {
        // Browsers replace NUL while parsing; do so explicitly in echoed input.
        Ok(body) => body.replace('\0', "\u{FFFD}"),
        Err(err) => return render_failure_response(template, &err),
    };
    match error {
        Some(classification) => {
            with_classification(html_response(classification.status, body), classification)
        }
        None => html_response(StatusCode::OK, body),
    }
}
