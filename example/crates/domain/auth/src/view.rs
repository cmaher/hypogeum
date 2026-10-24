//! Browser forms share registration and password authentication with JSON.
use crate::{AuthComponents, LoginRequest, RegistrationRequest};
use askama::Template;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use hypgm_authn::session::PrincipalAuthBackend;
use hypgm_view::{AppForm, HtmlError, render_error_template, render_template};

#[derive(Default, Template)]
#[template(path = "auth.html")]
struct AuthPage {
    register: bool,
    message: &'static str,
    display_name: String,
    username: String,
}
impl AuthPage {
    // Browser hints mirror the server-side password policy.
    fn password_min(&self) -> usize {
        exemplum_dmn_users::accounts::PASSWORD_MIN_CHARS
    }
    fn password_max(&self) -> usize {
        exemplum_dmn_users::accounts::PASSWORD_MAX_CHARS
    }
}

pub fn router(components: AuthComponents) -> Router<()> {
    Router::new()
        .route("/register", get(register_page).post(register))
        .route("/login", get(login_page).post(login))
        .with_state(components)
}
async fn register_page() -> Response {
    render_template(
        StatusCode::OK,
        &AuthPage {
            register: true,
            ..Default::default()
        },
    )
}
async fn login_page() -> Response {
    render_template(
        StatusCode::OK,
        &AuthPage {
            register: false,
            ..Default::default()
        },
    )
}
async fn register(
    State(components): State<AuthComponents>,
    AppForm(request): AppForm<RegistrationRequest>,
) -> Result<Response, HtmlError> {
    let page = AuthPage {
        register: true,
        display_name: request.display_name.clone(),
        username: request.username.clone(),
        ..Default::default()
    };
    if let Err(error) = components.register(request).await {
        return Ok(form_error(page, error));
    }
    Ok(Redirect::to("/registration-pending").into_response())
}
pub async fn pending() -> Response {
    render_template(
        StatusCode::OK,
        &AuthPage {
            register: false,
            message: "Registration received. Awaiting administrator approval before you can log in.",
            ..Default::default()
        },
    )
}
async fn login(
    State(components): State<AuthComponents>,
    mut session: axum_login::AuthSession<PrincipalAuthBackend>,
    AppForm(request): AppForm<LoginRequest>,
) -> Result<Response, HtmlError> {
    let page = AuthPage {
        username: request.username.clone(),
        ..Default::default()
    };
    let principal = match crate::controller::authenticate(&components, request).await {
        Ok(principal) => principal,
        Err(error) => return Ok(form_error(page, error)),
    };
    session
        .login(&principal)
        .await
        .map_err(|_| HtmlError::from_http(hypgm_http::HttpError::internal_error()))?;
    Ok(Redirect::to("/").into_response())
}

fn form_error(mut page: AuthPage, error: hypgm_http::HttpError) -> Response {
    let classification = error.classification();
    page.message = classification.message;
    render_error_template(classification, &page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn forms_reject_cross_site_and_never_echo_password() {
        let app = router(crate::AuthComponentsFactory::default().build())
            .layer(axum::middleware::from_fn(hypgm_view::cross_site_middleware));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/register")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("autocomplete=\"new-password\""));
        assert!(html.contains("minlength=\"8\" maxlength=\"128\""));
        let response = app.clone().oneshot(Request::builder().method("POST").uri("/register").header("content-type", "application/x-www-form-urlencoded").body(Body::from("display_name=Alice&username=alice&password=secret-echo-check&is_admin=true")).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!html.contains("secret-echo-check"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
