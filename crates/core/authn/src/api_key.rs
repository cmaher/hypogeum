use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// A generated credential: return `raw_key` to the caller and persist only `secret_hash`.
pub struct GeneratedApiKey {
    pub raw_key: String,
    pub secret_hash: String,
}

/// Generates a new API key with the given prefix and its SHA-256 hash.
///
/// Format: `<prefix>_<hex-encoded-24-random-bytes>`. All trailing underscores
/// on the supplied prefix are normalized to one separator.
pub fn generate_api_key(prefix: &str) -> GeneratedApiKey {
    let prefix = normalize_prefix(prefix);
    let mut random_bytes = [0u8; 24];
    argon2::password_hash::rand_core::RngCore::fill_bytes(
        &mut argon2::password_hash::rand_core::OsRng,
        &mut random_bytes,
    );
    let raw = format!("{prefix}{}", hex::encode(random_bytes));
    let hash = hash_api_key(&raw);
    GeneratedApiKey {
        raw_key: raw,
        secret_hash: hash,
    }
}

/// Computes the SHA-256 hash of a raw API key as a lowercase hexadecimal string.
pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Validates whether a raw key conforms to the expected prefix and hexadecimal payload.
pub fn validate_api_key_prefix(raw_key: &str, expected_prefix: &str) -> bool {
    let prefix = normalize_prefix(expected_prefix);
    if !raw_key.starts_with(&prefix) {
        return false;
    }
    let suffix = &raw_key[prefix.len()..];
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_prefix(prefix: &str) -> String {
    format!("{}_", prefix.trim_end_matches('_'))
}

/// Constant-time verification of an API key against a stored SHA-256 hex digest.
pub fn verify_api_key(raw_key: &str, stored_hash: &str) -> bool {
    let expected = hash_api_key(raw_key);
    if expected.len() != stored_hash.len() {
        return false;
    }
    expected.as_bytes().ct_eq(stored_hash.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_generation_and_hashing() {
        let GeneratedApiKey {
            raw_key: raw,
            secret_hash: hash,
        } = generate_api_key("test_key");
        assert!(raw.starts_with("test_key_"));
        assert_eq!(hash, hash_api_key(&raw));
        assert!(verify_api_key(&raw, &hash));
        assert!(!verify_api_key("test_key_wrong", &hash));
    }

    #[test]
    fn test_api_key_generation_strips_trailing_underscore() {
        let GeneratedApiKey {
            raw_key: raw,
            secret_hash: hash,
        } = generate_api_key("app_key_");
        assert!(raw.starts_with("app_key_"));
        assert!(!raw.starts_with("app_key__"));
        assert!(verify_api_key(&raw, &hash));
    }

    #[test]
    fn test_validate_api_key_prefix() {
        let GeneratedApiKey { raw_key: raw, .. } = generate_api_key("exmpl_key");
        assert!(validate_api_key_prefix(&raw, "exmpl_key"));
        assert!(validate_api_key_prefix(&raw, "exmpl_key_"));
        assert!(!validate_api_key_prefix(&raw, "other_key"));
        assert!(!validate_api_key_prefix(
            "exmpl_key_not_hex_!@#",
            "exmpl_key"
        ));
        assert!(!validate_api_key_prefix("exmpl_key_", "exmpl_key"));
    }
}

#[cfg(test)]
mod prefix_round_trips {
    use super::*;

    #[test]
    fn generation_and_validation_share_prefix_normalization() {
        for prefix in [
            "test_key",
            "test_key_",
            "test_key__",
            "test_key___",
            "",
            "_",
        ] {
            let generated = generate_api_key(prefix);
            assert!(
                validate_api_key_prefix(&generated.raw_key, prefix),
                "prefix: {prefix}"
            );
            assert!(!validate_api_key_prefix(&generated.raw_key, "other_key"));
            let normalized = prefix.trim_end_matches('_');
            assert!(!validate_api_key_prefix(&format!("{normalized}_"), prefix));
            assert!(!validate_api_key_prefix(
                &format!("{normalized}_not-hex"),
                prefix
            ));
        }
    }
}
