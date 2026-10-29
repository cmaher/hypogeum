//! Streamed byte storage addressed by caller-chosen keys, injected as
//! `Arc<dyn FileStorage>`.
//!
//! Keys are path-shaped [`ObjectKey`]s with S3-like semantics: a write creates or
//! atomically replaces the object at its key. Completion promises persistence
//! under the adapter's documented profile. Domain managers own retries,
//! validation, publication and deletion authorization; repos own metadata.
//! Partial consumption never verifies integrity. There is no general rename,
//! resume, listing or distributed transaction contract.
pub mod filesystem;
#[cfg(feature = "google-drive")]
pub mod google_drive;
pub mod models;
mod read_state;
pub use hypgm_errors::StorageError;
pub use models::{
    ByteRange, ExpectedIntegrity, ObjectInfo, ObjectKey, ObjectRef, ReadOptions, StoredObject,
};

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
    /// Create or replace the object at `key`. Readers observe the previous or
    /// the new object, never a partial one. Returns only after completed
    /// persistence and supplied integrity checks, which run before installation.
    async fn write(
        &self,
        key: &ObjectKey,
        input: ByteStream,
        expected: ExpectedIntegrity,
    ) -> Result<StoredObject, StorageError>;
    /// Stream bytes without integrity verification with `ReadOptions::default()`.
    /// Options can independently bound bytes and request size and/or SHA-256 checks.
    /// Errors may follow bytes. Requested checks require successful EOF; partial
    /// consumption makes no integrity claim. Adapter resource limits always apply.
    /// A range yields exactly its bytes; one extending past the object's end is
    /// `InvalidInput`, and combining a range with `expected` checks is rejected.
    async fn read(
        &self,
        reference: &ObjectRef,
        options: ReadOptions,
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
        let reference = ObjectRef::new("primary", &ObjectKey::new("a/b.jpg").unwrap());
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
