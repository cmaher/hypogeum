use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::AuthError;

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::Internal(format!("Failed to hash password: {e}")))
}

pub fn verify_password(password: &str, stored_hash: &str) -> Result<bool, AuthError> {
    let parsed_hash = match PasswordHash::new(stored_hash) {
        Ok(h) => h,
        Err(e) => {
            return Err(AuthError::Internal(format!(
                "Invalid password hash format: {e}"
            )));
        }
    };
    let argon2 = Argon2::default();
    match argon2.verify_password(password.as_bytes(), &parsed_hash) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(_) => Err(AuthError::Internal("Password verifier failed".into())),
    }
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
}

#[cfg(test)]
mod verifier_failures {
    use super::*;
    use hypgm_errors::{PublicError, PublicErrorInfo};

    #[test]
    fn invalid_verifier_inputs_are_internal_errors() {
        let password = "private-password";
        let hash = hash_password(password).unwrap();
        for invalid in [
            "malformed-PHC".to_string(),
            hash.replace("m=19456", "m=1"),
            hash.replace("argon2id", "unsupported"),
            hash.replace("v=19", "v=99"),
        ] {
            let err = verify_password(password, &invalid).expect_err("invalid verifier input");
            assert!(matches!(err, AuthError::Internal(_)));
            assert_eq!(err.public_error(), PublicErrorInfo::INTERNAL_ERROR);
            assert!(!err.to_string().contains(password));
            assert!(!err.to_string().contains(&invalid));
        }
    }
}
