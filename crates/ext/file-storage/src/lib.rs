//! Immutable streamed byte storage, injected as `Arc<dyn FileStorage>`.
//!
//! Completion promises persistence under the adapter's documented profile. Domain
//! managers own retries, validation, publication and deletion authorization; repos
//! own metadata. Partial consumption never verifies integrity. There is no general
//! rename, path, resume, listing or distributed transaction contract.
pub mod filesystem;
pub mod models;
pub use errors::StorageError;
pub use models::{ExpectedIntegrity, ObjectInfo, ObjectRef, StoredObject};

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use std::pin::Pin;

/// Pull-driven stream. Producers should provide bounded chunks; adapters process
/// oversized input chunks in bounded slices, without collecting an entire object.
/// An already allocated input chunk remains owned until processed.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send + 'static>>;

#[async_trait]
pub trait FileStorage: Send + Sync {
    /// Returns only after completed persistence and supplied integrity checks.
    async fn write(
        &self,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError>;
    /// Errors may follow bytes. Consume through successful EOF to verify integrity.
    async fn read(
        &self,
        reference: &ObjectRef,
        expected: ExpectedIntegrity,
    ) -> Result<ByteStream, StorageError>;
    /// Compute actual full-object size and SHA-256 (not metadata-only inspection).
    async fn inspect(&self, reference: &ObjectRef) -> Result<ObjectInfo, StorageError>;
    /// Idempotent when missing. Caller owns authorization and reference tracking.
    async fn delete(&self, reference: &ObjectRef) -> Result<(), StorageError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_round_trip_and_integrity_rejects_invalid_hashes() {
        let reference = ObjectRef {
            version: 1,
            store_id: "primary".into(),
            key: "opaque".into(),
        };
        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(serde_json::from_str::<ObjectRef>(&json).unwrap(), reference);
        assert!(
            ExpectedIntegrity {
                size_bytes: None,
                sha256: Some("invalid".into())
            }
            .validate()
            .is_err()
        );
        assert!(
            ExpectedIntegrity {
                size_bytes: Some(0),
                sha256: Some("a".repeat(64))
            }
            .validate()
            .is_ok()
        );
    }
}
