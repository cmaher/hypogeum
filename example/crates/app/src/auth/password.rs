use argon2::{
    Argon2,
    password_hash::{
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
    },
};
use sha2::{Digest, Sha256};

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, stored_hash: &str) -> Result<bool, String> {
    let parsed_hash = match PasswordHash::new(stored_hash) {
        Ok(h) => h,
        Err(e) => return Err(e.to_string()),
    };
    let argon2 = Argon2::default();
    Ok(argon2
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn verify_api_key(raw_key: &str, stored_hash: &str) -> bool {
    let expected = hash_api_key(raw_key);
    expected == stored_hash
}

pub fn generate_api_key() -> (String, String) {
    let mut random_bytes = [0u8; 24];
    argon2::password_hash::rand_core::RngCore::fill_bytes(&mut OsRng, &mut random_bytes);
    let raw = format!("exmpl_key_{}", hex::encode(random_bytes));
    let hash = hash_api_key(&raw);
    (raw, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing_and_verification() {
        let password = "SuperSecretPassword123!";
        let hash = hash_password(password).expect("hashing should succeed");
        assert!(hash.starts_with("$argon2id$"));

        assert!(verify_password(password, &hash).expect("verification should succeed"));
        assert!(!verify_password("wrong_password", &hash).expect("verification should succeed"));
        assert!(verify_password(password, "invalid_hash_format").is_err());
    }

    #[test]
    fn test_api_key_hashing_and_generation() {
        let (raw, hash) = generate_api_key();
        assert!(raw.starts_with("exmpl_key_"));
        assert_eq!(hash, hash_api_key(&raw));
        assert!(verify_api_key(&raw, &hash));
        assert!(!verify_api_key("exmpl_key_invalid", &hash));
    }
}
