//! One-time consent: set HYPOGEUM_GDRIVE_CLIENT_ID and CLIENT_SECRET (same
//! prefix). SCOPE defaults to full; file creates FOLDER_NAME (hypogeum-objects).
//! Run `cargo run -p hypgmx-gdrive-authorize --bin gdrive-authorize` and open the
//! displayed consent URL. Use a Desktop OAuth client and a Published consent
//! screen (Testing grants expire after seven days). No token files are written.
use anyhow::{Context, Result, anyhow, ensure};
use google_drive3::{DriveHub, common, hyper, hyper_rustls, hyper_util, yup_oauth2};
use hypgm_envconfig::{EnvSource, Secret};
use hypgmx_file_storage::google_drive::DriveScope;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;
use yup_oauth2::storage::{TokenInfo, TokenStorage, TokenStorageError};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scope {
    File,
    #[default]
    Full,
}
impl Scope {
    fn drive_scope(self) -> DriveScope {
        match self {
            Self::File => DriveScope::File,
            Self::Full => DriveScope::Full,
        }
    }
}
fn default_folder_name() -> String {
    "hypogeum-objects".into()
}

#[derive(Debug, Deserialize)]
struct Config {
    client_id: String,
    client_secret: Secret,
    #[serde(default)]
    scope: Scope,
    #[serde(default = "default_folder_name")]
    folder_name: String,
}

#[derive(Clone, Default)]
struct ConsentTokens(Arc<Mutex<BTreeMap<Vec<String>, TokenInfo>>>);
fn scope_key(scopes: &[&str]) -> Vec<String> {
    let mut key: Vec<_> = scopes.iter().map(|s| (*s).to_owned()).collect();
    key.sort_unstable();
    key.dedup();
    key
}
#[async_trait::async_trait]
impl TokenStorage for ConsentTokens {
    async fn set(&self, scopes: &[&str], token: TokenInfo) -> Result<(), TokenStorageError> {
        self.0.lock().await.insert(scope_key(scopes), token);
        Ok(())
    }
    async fn get(&self, scopes: &[&str]) -> Option<TokenInfo> {
        self.0.lock().await.get(&scope_key(scopes)).cloned()
    }
}

type Connector = hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;
fn connector() -> Result<Connector> {
    Ok(hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|_| anyhow!("Cannot initialize TLS roots"))?
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build())
}

#[tokio::main]
async fn main() -> Result<()> {
    let env = EnvSource::load()?;
    let config: Config = env.deserialize("HYPOGEUM_GDRIVE_")?;
    ensure!(
        !config.client_id.trim().is_empty(),
        "HYPOGEUM_GDRIVE_CLIENT_ID must be nonempty"
    );
    ensure!(
        !config.client_secret.expose().trim().is_empty(),
        "HYPOGEUM_GDRIVE_CLIENT_SECRET must be nonempty"
    );
    let scope = config.scope.drive_scope();
    let tokens = ConsentTokens::default();
    let secret = yup_oauth2::ApplicationSecret {
        client_id: config.client_id,
        client_secret: config.client_secret.expose().to_owned(),
        auth_uri: "https://accounts.google.com/o/oauth2/auth".into(),
        token_uri: "https://oauth2.googleapis.com/token".into(),
        redirect_uris: vec!["http://localhost".into()],
        ..Default::default()
    };
    let auth_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector()?);
    let auth = yup_oauth2::InstalledFlowAuthenticator::with_client(
        secret,
        yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect,
        yup_oauth2::client::CustomHyperClientBuilder::from(auth_client),
    )
    // InstalledFlow always requests offline access. This adds consent even
    // for existing grants so Google returns a refresh token again.
    .force_account_selection(true)
    .with_storage(Box::new(tokens.clone()))
    .build()
    .await
    .map_err(|_| anyhow!("Cannot initialize OAuth consent"))?;
    auth.token(&[scope.as_url()])
        .await
        .map_err(|_| anyhow!("OAuth consent failed"))?;
    let token = tokens.get(&[scope.as_url()]).await.and_then(|token| token.refresh_token)
        .filter(|token| !token.is_empty())
        .context("No refresh token returned: revoke the prior grant and retry; check that the OAuth consent screen is Published")?;
    if scope == DriveScope::File {
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector()?);
        let hub = DriveHub::new(client, auth);
        let id = create_folder(
            &hub,
            scope,
            &config.folder_name,
            "https://www.googleapis.com/",
        )
        .await?;
        println!("Folder ID: {id}");
    }
    println!("Refresh token: {token}");
    Ok(())
}

async fn create_folder(
    hub: &DriveHub<Connector>,
    scope: DriveScope,
    name: &str,
    root: &str,
) -> Result<String> {
    // google-drive3's FileCreateCall exposes only media uploads. Folder creation
    // requires the metadata-only endpoint, using the same hub and scoped auth.
    let token = hub
        .auth
        .get_token(&[scope.as_url()])
        .await
        .map_err(|_| anyhow!("Folder authentication failed"))?
        .context("No access token for folder creation")?;
    let metadata =
        serde_json::json!({"name": name, "mimeType": "application/vnd.google-apps.folder"});
    let request = hyper::Request::post(format!("{root}drive/v3/files?fields=id"))
        .header(hyper::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(common::to_body(Some(metadata.to_string())))
        .map_err(|_| anyhow!("Cannot prepare folder creation"))?;
    let response = hub
        .client
        .request(request)
        .await
        .map_err(|_| anyhow!("Folder creation failed; completion may be uncertain"))?;
    ensure!(
        response.status().is_success(),
        "Folder creation failed (HTTP {})",
        response.status().as_u16()
    );
    let body = common::to_bytes(response.into_body())
        .await
        .context("Cannot read created folder metadata")?;
    let file: google_drive3::api::File =
        serde_json::from_slice(&body).map_err(|_| anyhow!("Invalid created folder metadata"))?;
    file.id.context("Created folder response omitted its ID")
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_drive3::yup_oauth2::storage::TokenStorage;

    fn env() -> EnvSource {
        EnvSource::from_pairs([
            ("HYPOGEUM_GDRIVE_CLIENT_ID", "client"),
            ("HYPOGEUM_GDRIVE_CLIENT_SECRET", "secret-sentinel"),
        ])
    }
    #[test]
    fn config_defaults_and_redaction() {
        let config: Config = env().deserialize("HYPOGEUM_GDRIVE_").unwrap();
        assert_eq!(config.scope.drive_scope(), DriveScope::Full);
        assert_eq!(config.folder_name, "hypogeum-objects");
        assert!(!format!("{config:?}").contains("secret-sentinel"));
    }
    #[test]
    fn parses_scopes_and_rejects_invalid_scope() {
        for (value, scope) in [("full", DriveScope::Full), ("file", DriveScope::File)] {
            let env = EnvSource::from_pairs([
                ("HYPOGEUM_GDRIVE_CLIENT_ID", "client"),
                ("HYPOGEUM_GDRIVE_CLIENT_SECRET", "secret"),
                ("HYPOGEUM_GDRIVE_SCOPE", value),
            ]);
            assert_eq!(
                env.deserialize::<Config>("HYPOGEUM_GDRIVE_")
                    .unwrap()
                    .scope
                    .drive_scope(),
                scope
            );
        }
        let env = EnvSource::from_pairs([
            ("HYPOGEUM_GDRIVE_CLIENT_ID", "client"),
            ("HYPOGEUM_GDRIVE_CLIENT_SECRET", "secret"),
            ("HYPOGEUM_GDRIVE_SCOPE", "invalid"),
        ]);
        assert!(env.deserialize::<Config>("HYPOGEUM_GDRIVE_").is_err());
    }
    #[test]
    fn missing_client_id_names_environment_variable() {
        let env = EnvSource::from_pairs([("HYPOGEUM_GDRIVE_CLIENT_SECRET", "secret")]);
        assert!(
            env.deserialize::<Config>("HYPOGEUM_GDRIVE_")
                .unwrap_err()
                .to_string()
                .contains("HYPOGEUM_GDRIVE_CLIENT_ID")
        );
    }
    #[tokio::test]
    async fn captures_refresh_token_in_memory() {
        let storage = ConsentTokens::default();
        let scopes = [DriveScope::File.as_url()];
        assert!(storage.get(&scopes).await.is_none());
        storage
            .set(
                &scopes,
                yup_oauth2::storage::TokenInfo {
                    access_token: Some("access".into()),
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                    id_token: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            storage.get(&scopes).await.unwrap().refresh_token.as_deref(),
            Some("refresh")
        );
        assert!(storage.get(&[DriveScope::Full.as_url()]).await.is_none());
    }
    #[tokio::test]
    async fn creates_folder_with_metadata_only_and_redacts_server_errors() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode},
            routing::post,
        };
        for status in [StatusCode::OK, StatusCode::FORBIDDEN] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let root = format!("http://{}/", listener.local_addr().unwrap());
            let router = Router::new().route("/drive/v3/files", post(move |headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                assert_eq!(headers["authorization"], "Bearer fixed-token");
                assert_eq!(body["mimeType"], "application/vnd.google-apps.folder");
                assert_eq!(body["name"], "storage-folder");
                (status, Json(serde_json::json!({"id": "created-folder", "error": "secret-response"})))
            }));
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let client =
                hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build(connector().unwrap());
            let hub = DriveHub::new(client, "fixed-token".to_owned());
            let result = create_folder(&hub, DriveScope::File, "storage-folder", &root).await;
            server.abort();
            if status.is_success() {
                assert_eq!(result.unwrap(), "created-folder");
            } else {
                let error = format!("{:?}", result.unwrap_err());
                assert!(error.contains("403"));
                assert!(!error.contains("secret-response"));
                assert!(!error.contains("fixed-token"));
            }
        }
    }
}
