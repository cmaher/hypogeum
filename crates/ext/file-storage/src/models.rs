//! Backend-neutral data. Only the matching adapter interprets an object's key.
use hypgm_errors::StorageError;
use serde::{Deserialize, Serialize};

/// A complete byte object. Preserve the store alias when moving/restoring storage.
/// Unknown versions must be rejected by adapters, not guessed or migrated implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub version: u32,
    pub store_id: String,
    pub key: String,
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
