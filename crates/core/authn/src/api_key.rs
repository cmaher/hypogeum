use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Generates a new API key with the given prefix and returns `(raw_key, sha256_hex_hash)`.
///
/// Format: `<prefix>_<hex-encoded-24-random-bytes>`
pub fn generate_api_key(prefix: &str) -> (String, String) {
    let prefix = prefix.trim_end_matches('_');
    let mut random_bytes = [0u8; 24];
    argon2::password_hash::rand_core::RngCore::fill_bytes(
        &mut argon2::password_hash::rand_core::OsRng,
        &mut random_bytes,
    );
    let raw = format!("{prefix}_{}", hex::encode(random_bytes));
    let hash = hash_api_key(&raw);
    (raw, hash)
}

/// Computes the SHA-256 hash of a raw API key as a lowercase hexadecimal string.
pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Validates whether a raw key conforms to the expected prefix and hexadecimal payload.
pub fn validate_api_key_prefix(raw_key: &str, expected_prefix: &str) -> bool {
    let prefix = if expected_prefix.ends_with('_') {
        expected_prefix.to_string()
    } else {
        format!("{expected_prefix}_")
    };
    if !raw_key.starts_with(&prefix) {
        return false;
    }
    let suffix = &raw_key[prefix.len()..];
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit())
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
        let (raw, hash) = generate_api_key("test_key");
        assert!(raw.starts_with("test_key_"));
        assert_eq!(hash, hash_api_key(&raw));
        assert!(verify_api_key(&raw, &hash));
        assert!(!verify_api_key("test_key_wrong", &hash));
    }

    #[test]
    fn test_api_key_generation_strips_trailing_underscore() {
        let (raw, hash) = generate_api_key("app_key_");
        assert!(raw.starts_with("app_key_"));
        assert!(!raw.starts_with("app_key__"));
        assert!(verify_api_key(&raw, &hash));
    }

    #[test]
    fn test_validate_api_key_prefix() {
        let (raw, _) = generate_api_key("exmpl_key");
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
