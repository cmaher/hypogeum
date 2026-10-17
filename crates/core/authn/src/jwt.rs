use crate::error::AuthError;
use crate::principal::{AuthMethod, Principal};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Represents JWT audience claim, which RFC 7519 §4.1.3 allows to be either
/// a single string or an array of strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Audience {
    Single(String),
    Multiple(Vec<String>),
}

impl Audience {
    pub fn contains(&self, expected: &str) -> bool {
        match self {
            Self::Single(s) => s == expected,
            Self::Multiple(list) => list.iter().any(|s| s == expected),
        }
    }
}

impl From<&str> for Audience {
    fn from(s: &str) -> Self {
        Self::Single(s.to_string())
    }
}

impl From<String> for Audience {
    fn from(s: String) -> Self {
        Self::Single(s)
    }
}

impl From<Vec<String>> for Audience {
    fn from(list: Vec<String>) -> Self {
        Self::Multiple(list)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    pub auth_id: Uuid,
    pub sub: Uuid,
    pub exp: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<Audience>,
}

impl Claims {
    pub fn new(auth_id: Uuid, sub: Uuid, exp: usize) -> Self {
        Self {
            auth_id,
            sub,
            exp,
            iss: None,
            aud: None,
        }
    }

    pub fn with_iss_aud<A: Into<Audience>>(
        auth_id: Uuid,
        sub: Uuid,
        exp: usize,
        iss: Option<String>,
        aud: Option<A>,
    ) -> Self {
        Self {
            auth_id,
            sub,
            exp,
            iss,
            aud: aud.map(Into::into),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JwtConfig {
    pub algorithm: Algorithm,
    pub issuer: Option<String>,
    pub audience: Option<Audience>,
    pub leeway_seconds: u64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::HS256,
            issuer: None,
            audience: None,
            leeway_seconds: 0,
        }
    }
}

#[derive(Clone)]
pub struct JwtValidator {
    decoding_key: DecodingKey,
    validation: Validation,
    config: JwtConfig,
}

impl JwtValidator {
    pub fn new(decoding_key: DecodingKey, config: JwtConfig) -> Self {
        let mut validation = Validation::new(config.algorithm);
        validation.leeway = config.leeway_seconds;
        if let Some(ref iss) = config.issuer {
            validation.set_issuer(&[iss]);
        }
        if let Some(ref aud) = config.audience {
            match aud {
                Audience::Single(s) => validation.set_audience(&[s]),
                Audience::Multiple(list) => validation.set_audience(list),
            }
        } else {
            validation.validate_aud = false;
        }

        Self {
            decoding_key,
            validation,
            config,
        }
    }

    pub fn from_hs256_secret(secret: &[u8]) -> Self {
        Self::from_hs256_secret_with_config(secret, JwtConfig::default())
    }

    pub fn from_hs256_secret_with_config(secret: &[u8], mut config: JwtConfig) -> Self {
        config.algorithm = Algorithm::HS256;
        Self::new(DecodingKey::from_secret(secret), config)
    }

    pub fn from_rs256_pem(pem: &[u8]) -> Result<Self, AuthError> {
        Self::from_rs256_pem_with_config(pem, JwtConfig::default())
    }

    pub fn from_rs256_pem_with_config(
        pem: &[u8],
        mut config: JwtConfig,
    ) -> Result<Self, AuthError> {
        let key = DecodingKey::from_rsa_pem(pem)
            .map_err(|e| AuthError::Internal(format!("Invalid RSA public key: {e}")))?;
        config.algorithm = Algorithm::RS256;
        Ok(Self::new(key, config))
    }

    pub fn config(&self) -> &JwtConfig {
        &self.config
    }

    pub fn decode_claims(&self, token: &str) -> Result<Claims, AuthError> {
        let trimmed = token.trim();
        let token =
            if let Some((scheme, rest)) = trimmed.split_once(|c: char| c.is_ascii_whitespace()) {
                if scheme.eq_ignore_ascii_case("bearer") {
                    rest.trim()
                } else {
                    trimmed
                }
            } else if trimmed.eq_ignore_ascii_case("bearer") {
                ""
            } else {
                trimmed
            };

        if token.is_empty() {
            return Err(AuthError::MissingCredentials);
        }

        let token_data = decode::<Claims>(token, &self.decoding_key, &self.validation).map_err(
            |err| match err.kind() {
                ErrorKind::ExpiredSignature => AuthError::ExpiredToken,
                _ => AuthError::InvalidToken(err.to_string()),
            },
        )?;

        Ok(token_data.claims)
    }

    pub fn verify_token(&self, token: &str) -> Result<Principal, AuthError> {
        let claims = self.decode_claims(token)?;
        Ok(Principal::new(
            claims.auth_id,
            claims.sub,
            AuthMethod::BearerJwt,
        ))
    }
}
