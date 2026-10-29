//! Key resolution by name, from the storage folder down. Every segment but the
//! last is a folder; the last is a non-folder file.
use super::{FOLDER_MIME, GoogleDriveStorage, SHORTCUT_MIME, error, write::response_error};
use crate::{ObjectKey, StorageError};
use bytes::Bytes;
use google_drive3::{
    common,
    hyper::{Request, header},
};
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Resolved {
    pub(super) id: String,
    /// The file's reported size; `None` for folders.
    pub(super) size: Option<u64>,
}

/// A resolved file and whether any part of its path came from the cache, in
/// which case a stale ID is worth one fresh resolution.
pub(super) struct Located {
    pub(super) file: Resolved,
    pub(super) cached: bool,
}

pub(super) struct Found {
    pub(super) resolved: Resolved,
    pub(super) mime: Option<String>,
}

pub(super) fn missing() -> StorageError {
    StorageError::NotFound(io::Error::new(
        io::ErrorKind::NotFound,
        "no Drive object at key",
    ))
}

/// Quote a value for a Drive query string literal.
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Drive IDs are interpolated into request URLs: accept only their alphabet.
pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Folder segments with their cache keys (`prefix/`), outermost first.
fn folders(key: &ObjectKey) -> impl Iterator<Item = (&str, &str)> {
    let key = key.as_str();
    key.match_indices('/').scan(0, move |start, (end, _)| {
        let segment = &key[*start..end];
        *start = end + 1;
        Some((segment, &key[..=end]))
    })
}

pub(super) fn file_name(key: &ObjectKey) -> &str {
    key.segments()
        .next_back()
        .expect("object keys are nonempty")
}

impl GoogleDriveStorage {
    /// Resolve `key` to a file. Without `fresh`, cached path entries are used,
    /// and a miss on a partly cached path is resolved again from the root.
    pub(super) async fn locate(
        &self,
        key: &ObjectKey,
        fresh: bool,
    ) -> Result<Located, StorageError> {
        let mut cached = false;
        if let Some(file) = self.walk(key, fresh, &mut cached).await? {
            return Ok(Located { file, cached });
        }
        if !cached {
            return Err(missing());
        }
        self.forget_path(key).await;
        let mut cached = false;
        let file = self
            .walk(key, true, &mut cached)
            .await?
            .ok_or_else(missing)?;
        Ok(Located { file, cached })
    }

    async fn walk(
        &self,
        key: &ObjectKey,
        fresh: bool,
        cached: &mut bool,
    ) -> Result<Option<Resolved>, StorageError> {
        if !fresh && let Some(file) = self.cached(key.as_str()).await {
            *cached = true;
            return Ok(Some(file));
        }
        let Some(parent) = self.parent_folder(key, fresh, false, cached).await? else {
            return Ok(None);
        };
        let Some(found) = self.find(&parent, file_name(key), false).await? else {
            return Ok(None);
        };
        // Shortcuts are not objects.
        if found.mime.as_deref() == Some(SHORTCUT_MIME) {
            return Ok(None);
        }
        self.remember(key.as_str(), &found.resolved).await;
        Ok(Some(found.resolved))
    }

    /// The ID of the folder holding `key`'s file. Missing folders are created
    /// when `create` is set, and otherwise resolve to `None`.
    pub(super) async fn parent_folder(
        &self,
        key: &ObjectKey,
        fresh: bool,
        create: bool,
        cached: &mut bool,
    ) -> Result<Option<String>, StorageError> {
        let mut parent = self.folder_id.clone();
        for (segment, prefix) in folders(key) {
            if !fresh && let Some(folder) = self.cached(prefix).await {
                *cached = true;
                parent = folder.id;
                continue;
            }
            let folder = match self.find(&parent, segment, true).await? {
                Some(found) => found.resolved,
                None if create => Resolved {
                    id: self.create_folder(&parent, segment).await?,
                    size: None,
                },
                None => return Ok(None),
            };
            self.remember(prefix, &folder).await;
            parent = folder.id;
        }
        Ok(Some(parent))
    }

    /// The single non-trashed folder (or non-folder) named `name` in `parent`.
    pub(super) async fn find(
        &self,
        parent: &str,
        name: &str,
        folder: bool,
    ) -> Result<Option<Found>, StorageError> {
        const OPERATION: &str = "resolve Drive path";
        let query = format!(
            "name = {} and {} in parents and trashed = false and mimeType {} {}",
            quote(name),
            quote(parent),
            if folder { "=" } else { "!=" },
            quote(FOLDER_MIME),
        );
        let mut matches = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut call = self
                .hub
                .files()
                .list()
                .q(&query)
                .page_size(2)
                .corpora("allDrives")
                .include_items_from_all_drives(true)
                .supports_all_drives(true)
                .param(
                    "fields",
                    "nextPageToken,incompleteSearch,files(id,mimeType,size)",
                )
                .add_scope(self.scope.as_url());
            if let Some(token) = &page_token {
                call = call.page_token(token);
            }
            let (_, list) = call.doit().await.map_err(|e| error::api(e, OPERATION))?;
            // An incomplete search can prove neither absence nor uniqueness.
            if list.incomplete_search == Some(true) {
                return Err(error::io(OPERATION));
            }
            matches.extend(list.files.unwrap_or_default());
            if matches.len() > 1 {
                return Err(StorageError::InvalidInput(
                    "Drive path is ambiguous: duplicate names at one segment".into(),
                ));
            }
            match list.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }
        let Some(file) = matches.pop() else {
            return Ok(None);
        };
        let id = file
            .id
            .filter(|id| valid_id(id))
            .ok_or_else(|| error::io(OPERATION))?;
        Ok(Some(Found {
            resolved: Resolved {
                id,
                size: file.size.and_then(|size| u64::try_from(size).ok()),
            },
            mime: file.mime_type,
        }))
    }

    async fn create_folder(&self, parent: &str, name: &str) -> Result<String, StorageError> {
        const OPERATION: &str = "create Drive folder";
        let metadata = serde_json::json!({
            "name": name, "parents": [parent], "mimeType": FOLDER_MIME
        });
        let request = Request::post(format!(
            "{}drive/v3/files?supportsAllDrives=true&fields=id",
            self.root_url
        ))
        .header(header::CONTENT_TYPE, "application/json")
        .body(common::to_body(Some(Bytes::from(metadata.to_string()))))
        .map_err(|_| error::io(OPERATION))?;
        let response = self.request(request, OPERATION).await?;
        if !response.status().is_success() {
            return Err(response_error(response, OPERATION).await);
        }
        let bytes = super::write::response_bytes(response).await?;
        let folder: google_drive3::api::File =
            serde_json::from_slice(&bytes).map_err(|_| error::io(OPERATION))?;
        folder
            .id
            .filter(|id| valid_id(id))
            .ok_or_else(|| error::io(OPERATION))
    }

    pub(super) async fn cached(&self, path: &str) -> Option<Resolved> {
        self.paths.as_ref()?.get(path).await
    }

    pub(super) async fn remember(&self, path: &str, resolved: &Resolved) {
        if let Some(cache) = &self.paths {
            cache.insert(path.to_owned(), resolved.clone()).await;
        }
    }

    pub(super) async fn forget_file(&self, key: &ObjectKey) {
        if let Some(cache) = &self.paths {
            cache.invalidate(key.as_str()).await;
        }
    }

    /// Evict the file and every folder on its path.
    pub(super) async fn forget_path(&self, key: &ObjectKey) {
        if let Some(cache) = &self.paths {
            cache.invalidate(key.as_str()).await;
            for (_, prefix) in folders(key) {
                cache.invalidate(prefix).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_escape_quotes_and_backslashes() {
        assert_eq!(quote("it's"), r"'it\'s'");
        assert_eq!(quote(r"a\'b"), r"'a\\\'b'");
    }

    #[test]
    fn folder_prefixes_end_with_a_slash() {
        let key = ObjectKey::new("a/bc/d.jpg").unwrap();
        assert_eq!(
            folders(&key).collect::<Vec<_>>(),
            [("a", "a/"), ("bc", "a/bc/")]
        );
        assert_eq!(file_name(&key), "d.jpg");
        assert_eq!(folders(&ObjectKey::new("top").unwrap()).count(), 0);
    }
}
