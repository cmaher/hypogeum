use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    BearerJwt,
    Session,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub auth_id: Uuid,
    pub user_id: Uuid,
    pub method: AuthMethod,
}

impl Principal {
    pub const fn new(auth_id: Uuid, user_id: Uuid, method: AuthMethod) -> Self {
        Self {
            auth_id,
            user_id,
            method,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_principal_distinguishes_auth_id_and_user_id() {
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        assert_ne!(auth_id, user_id);

        let principal = Principal::new(auth_id, user_id, AuthMethod::BearerJwt);
        assert_eq!(principal.auth_id, auth_id);
        assert_eq!(principal.user_id, user_id);
        assert_eq!(principal.method, AuthMethod::BearerJwt);
    }

    #[test]
    fn test_principal_serde() {
        let auth_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let principal = Principal::new(auth_id, user_id, AuthMethod::Session);

        let serialized = serde_json::to_string(&principal).expect("serialization should succeed");
        let deserialized: Principal =
            serde_json::from_str(&serialized).expect("deserialization should succeed");

        assert_eq!(principal, deserialized);
    }

    #[test]
    fn test_auth_method_serde() {
        let methods = [
            (AuthMethod::BearerJwt, "\"bearer_jwt\""),
            (AuthMethod::Session, "\"session\""),
            (AuthMethod::ApiKey, "\"api_key\""),
        ];

        for (method, expected_json) in methods {
            let json = serde_json::to_string(&method).unwrap();
            assert_eq!(json, expected_json);
            let parsed: AuthMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, method);
        }
    }
}
