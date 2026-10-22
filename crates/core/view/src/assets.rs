//! Explicit routes for the vendored HTMX release and shared configuration.
//!
//! Nest [`router`] at [`ASSETS_MOUNT`] so page layouts can reference
//! [`HTMX_SCRIPT_URL`] and [`VIEW_SCRIPT_URL`]. Only the listed files are
//! served; there is no filesystem access at runtime.

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// Pinned HTMX release vendored in `assets/htmx.min.js` (see `assets/LICENSE.htmx`).
pub const HTMX_VERSION: &str = "2.0.11";
pub const ASSETS_MOUNT: &str = "/assets";
pub const HTMX_SCRIPT_URL: &str = "/assets/htmx-2.0.11.min.js";
pub const VIEW_SCRIPT_URL: &str = "/assets/view.js";

const HTMX_SCRIPT_ROUTE: &str = "/htmx-2.0.11.min.js";
const VIEW_SCRIPT_ROUTE: &str = "/view.js";
const JAVASCRIPT_CONTENT_TYPE: &str = "text/javascript; charset=utf-8";

const HTMX_SCRIPT: &str = include_str!("../assets/htmx.min.js");
const VIEW_SCRIPT: &str = include_str!("../assets/view.js");

pub fn router() -> Router<()> {
    Router::new()
        .route(HTMX_SCRIPT_ROUTE, get(htmx_script))
        .route(VIEW_SCRIPT_ROUTE, get(view_script))
}

fn script(body: &'static str, cache_control: &'static str) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(JAVASCRIPT_CONTENT_TYPE),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(cache_control),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        body,
    )
        .into_response()
}

async fn htmx_script() -> Response {
    // The URL contains the pinned version, so the content never changes.
    script(HTMX_SCRIPT, "public, max-age=31536000, immutable")
}

async fn view_script() -> Response {
    script(VIEW_SCRIPT, "no-cache")
}
