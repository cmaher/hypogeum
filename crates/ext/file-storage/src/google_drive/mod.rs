//! Personal Google Drive storage using an operator's OAuth grant.
//!
//! Objects are immutable files directly inside the configured folder. Domain
//! managers own retries. Cancelled upload futures abandon their resumable session
//! (Drive expires it); they never publish an incomplete object.
mod error;
#[cfg(test)]
mod tests;

use crate::{ObjectRef, StorageError};
use google_drive3::{DriveHub, hyper_rustls, hyper_util, yup_oauth2};
use std::fmt;

type Connector = hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriveScope {
    File,
    #[default]
    Full,
}

impl DriveScope {
    pub fn as_url(self) -> &'static str {
        match self {
            Self::File => "https://www.googleapis.com/auth/drive.file",
            Self::Full => "https://www.googleapis.com/auth/drive",
        }
    }
}

#[derive(Clone)]
pub struct GoogleDriveConfig {
    pub store_id: String,
    pub folder_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    pub scope: DriveScope,
    pub max_object_bytes: u64,
}

impl fmt::Debug for GoogleDriveConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleDriveConfig")
            .field("store_id", &self.store_id)
            .field("folder_id", &self.folder_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("scope", &self.scope)
            .field("max_object_bytes", &self.max_object_bytes)
            .finish()
    }
}

pub struct GoogleDriveStorage {
    hub: DriveHub<Connector>,
    store_id: String,
    folder_id: String,
    scope: DriveScope,
    max_object_bytes: u64,
    root_url: String,
}

impl GoogleDriveStorage {
    pub async fn new(config: GoogleDriveConfig) -> Result<Self, StorageError> {
        Self::validate_config(&config)?;
        let auth = yup_oauth2::AuthorizedUserAuthenticator::with_client(
            yup_oauth2::authorized_user::AuthorizedUserSecret {
                client_id: config.client_id.clone(),
                client_secret: config.client_secret.clone(),
                refresh_token: config.refresh_token.clone(),
                key_type: "authorized_user".into(),
            },
            yup_oauth2::client::CustomHyperClientBuilder::from(
                hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build(Self::connector()?),
            ),
        )
        .build()
        .await
        .map_err(|_| error::permission())?;
        let storage = Self::construct(config, auth, "https://www.googleapis.com/")?;
        storage.check_folder().await?;
        Ok(storage)
    }

    /// Fixed credentials and a loopback endpoint for protocol integration tests.
    #[doc(hidden)]
    pub async fn with_test_endpoint(
        config: GoogleDriveConfig,
        root_url: &str,
        access_token: &str,
    ) -> Result<Self, StorageError> {
        let storage = Self::construct(config, access_token.to_owned(), root_url)?;
        storage.check_folder().await?;
        Ok(storage)
    }

    fn client() -> Result<google_drive3::common::Client<Connector>, StorageError> {
        Ok(
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(Self::connector()?),
        )
    }

    fn connector() -> Result<Connector, StorageError> {
        Ok(hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .map_err(|_| error::io("initialize TLS"))?
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build())
    }

    fn validate_config(config: &GoogleDriveConfig) -> Result<(), StorageError> {
        if config.store_id.trim().is_empty()
            || config.folder_id.trim().is_empty()
            || config.max_object_bytes == 0
        {
            return Err(StorageError::InvalidInput(
                "store ID and folder ID must be nonempty and maximum bytes positive".into(),
            ));
        }
        Ok(())
    }

    fn construct(
        config: GoogleDriveConfig,
        auth: impl google_drive3::common::GetToken + 'static,
        root_url: &str,
    ) -> Result<Self, StorageError> {
        Self::validate_config(&config)?;
        let client = Self::client()?;
        let mut hub = DriveHub::new(client, auth);
        let root_url = format!("{}/", root_url.trim_end_matches('/'));
        hub.root_url(root_url.clone());
        hub.base_url(format!("{root_url}drive/v3/"));
        Ok(Self {
            hub,
            store_id: config.store_id,
            folder_id: config.folder_id,
            scope: config.scope,
            max_object_bytes: config.max_object_bytes,
            root_url,
        })
    }

    async fn check_folder(&self) -> Result<(), StorageError> {
        let (_, folder) = self
            .hub
            .files()
            .get(&self.folder_id)
            .param("fields", "id,mimeType,trashed")
            .supports_all_drives(true)
            .add_scope(self.scope.as_url())
            .doit()
            .await
            .map_err(|e| error::api(e, "check storage folder"))?;
        if folder.mime_type.as_deref() != Some(FOLDER_MIME) || folder.trashed == Some(true) {
            return Err(StorageError::InvalidInput(
                "storage folder must be a non-trashed Drive folder".into(),
            ));
        }
        Ok(())
    }

    fn resolve<'a>(&self, reference: &'a ObjectRef) -> Result<&'a str, StorageError> {
        if reference.version != 1
            || reference.store_id != self.store_id
            || reference.key.is_empty()
            || !reference
                .key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(StorageError::InvalidReference(
                "unknown version, store or malformed Drive ID".into(),
            ));
        }
        Ok(&reference.key)
    }
}
