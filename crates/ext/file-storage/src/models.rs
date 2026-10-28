//! Backend-neutral data. Each adapter maps an object key to a path below its root.
use hypgm_errors::StorageError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A caller-chosen, path-shaped object key such as `photos/2026/a.jpg`.
///
/// Keys are relative `/`-separated segments. Empty, `.` and `..` segments,
/// backslashes and control characters are rejected, as are segments longer
/// than [`ObjectKey::MAX_SEGMENT_BYTES`] and keys longer than
/// [`ObjectKey::MAX_BYTES`]. Adapters may reserve further names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectKey(String);

impl ObjectKey {
    pub const MAX_BYTES: usize = 1024;
    pub const MAX_SEGMENT_BYTES: usize = 255;

    pub fn new(key: impl Into<String>) -> Result<Self, StorageError> {
        let key = key.into();
        let invalid =
            |reason: &str| Err(StorageError::InvalidInput(format!("object key {reason}")));
        if key.is_empty() {
            return invalid("must be nonempty");
        }
        if key.len() > Self::MAX_BYTES {
            return invalid("exceeds 1024 bytes");
        }
        if key.chars().any(|c| c == '\\' || c.is_control()) {
            return invalid("must not contain backslashes or control characters");
        }
        for segment in key.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return invalid("segments must be nonempty, relative names");
            }
            if segment.len() > Self::MAX_SEGMENT_BYTES {
                return invalid("segments must not exceed 255 bytes");
            }
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Nonempty path segments; all but the last name parent folders.
    pub fn segments(&self) -> std::str::Split<'_, char> {
        self.0.split('/')
    }
}

impl TryFrom<String> for ObjectKey {
    type Error = StorageError;
    fn try_from(key: String) -> Result<Self, StorageError> {
        Self::new(key)
    }
}

impl TryFrom<&str> for ObjectKey {
    type Error = StorageError;
    fn try_from(key: &str) -> Result<Self, StorageError> {
        Self::new(key)
    }
}

impl std::str::FromStr for ObjectKey {
    type Err = StorageError;
    fn from_str(key: &str) -> Result<Self, StorageError> {
        Self::new(key)
    }
}

impl From<ObjectKey> for String {
    fn from(key: ObjectKey) -> String {
        key.0
    }
}

impl AsRef<str> for ObjectKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A complete byte object. Preserve the store alias when moving/restoring storage.
/// Unknown versions must be rejected by adapters, not guessed or migrated implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub version: u32,
    pub store_id: String,
    /// An [`ObjectKey`]-valid string.
    pub key: String,
}

impl ObjectRef {
    /// Version 2 references carry caller-chosen [`ObjectKey`]s. Version 1
    /// (adapter-generated keys) is no longer accepted.
    pub const VERSION: u32 = 2;

    pub fn new(store_id: impl Into<String>, key: &ObjectKey) -> Self {
        Self {
            version: Self::VERSION,
            store_id: store_id.into(),
            key: key.as_str().to_owned(),
        }
    }

    /// The validated key, when this reference belongs to `store_id`.
    pub(crate) fn key_in(&self, store_id: &str) -> Result<ObjectKey, StorageError> {
        if self.version != Self::VERSION || self.store_id != store_id {
            return Err(StorageError::InvalidReference(
                "unknown version or store".into(),
            ));
        }
        ObjectKey::new(self.key.as_str())
            .map_err(|_| StorageError::InvalidReference("malformed object key".into()))
    }
}

/// Integrity of actual bytes, with lowercase hexadecimal full-object SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredObject {
    pub object_ref: ObjectRef,
    pub info: ObjectInfo,
}

/// Optional full-object expectations. Verification requires successful EOF.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedIntegrity {
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
}

/// Read policy. Defaults to no integrity verification and the adapter's byte limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadOptions {
    /// Full-object checks performed only at successful EOF. Neither is required.
    pub expected: ExpectedIntegrity,
    /// Maximum bytes allowed, independent of expected size. The adapter's configured
    /// limit still applies; the smaller limit wins. Zero permits only empty objects.
    pub max_bytes: Option<u64>,
    /// Stream only `[offset, offset + length)`. Ranged reads make no integrity
    /// claim, so they reject `expected` checks; byte limits apply to `length`.
    pub range: Option<ByteRange>,
}

/// A byte slice of an object. The whole slice must lie within the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub offset: u64,
    pub length: u64,
}

impl ExpectedIntegrity {
    pub fn validate(&self) -> Result<(), StorageError> {
        if let Some(hash) = &self.sha256
            && (hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(StorageError::InvalidInput(
                "SHA-256 must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(())
    }

    pub fn verify(&self, actual: &ObjectInfo) -> Result<(), StorageError> {
        self.validate()?;
        if self
            .size_bytes
            .is_some_and(|size| size != actual.size_bytes)
            || self
                .sha256
                .as_ref()
                .is_some_and(|hash| hash != &actual.sha256)
        {
            return Err(StorageError::IntegrityMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_keys_accept_nested_relative_paths() {
        for key in [
            "a",
            "a/b/c.jpg",
            ".hidden/..dots/x.",
            "unicode/é/名前",
            "spaces and 'quotes'/x y",
            &"s".repeat(255),
            &vec!["s".repeat(255); 4].join("/"),
        ] {
            assert_eq!(ObjectKey::new(key).unwrap().as_str(), key);
        }
        let key = ObjectKey::new("a/b/c").unwrap();
        assert_eq!(key.segments().collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn object_keys_reject_malformed_paths() {
        let long_key = vec!["s".repeat(255); 5].join("/");
        assert!(long_key.len() > ObjectKey::MAX_BYTES);
        for key in [
            "",
            "/",
            "/absolute",
            "trailing/",
            "a//b",
            ".",
            "..",
            "a/./b",
            "a/../b",
            "../escape",
            "a\\b",
            "nul\0byte",
            "line\nbreak",
            "tab\tchar",
            "del\u{7f}",
            "c1\u{85}",
            &"s".repeat(256),
            &format!("a/{}", "s".repeat(256)),
            &long_key,
        ] {
            assert!(
                matches!(ObjectKey::new(key), Err(StorageError::InvalidInput(_))),
                "{key:?}"
            );
        }
    }

    #[test]
    fn object_keys_validate_when_deserialized() {
        let key: ObjectKey = serde_json::from_str("\"a/b\"").unwrap();
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"a/b\"");
        assert!(serde_json::from_str::<ObjectKey>("\"../b\"").is_err());
    }

    #[test]
    fn references_require_version_two_store_and_valid_key() {
        let key = ObjectKey::new("a/b").unwrap();
        let reference = ObjectRef::new("store", &key);
        assert_eq!(reference.version, 2);
        assert_eq!(reference.key_in("store").unwrap(), key);
        for bad in [
            ObjectRef {
                version: 1,
                ..reference.clone()
            },
            ObjectRef {
                store_id: "other".into(),
                ..reference.clone()
            },
            ObjectRef {
                key: "a//b".into(),
                ..reference
            },
        ] {
            assert!(matches!(
                bad.key_in("store"),
                Err(StorageError::InvalidReference(_))
            ));
        }
    }
}
