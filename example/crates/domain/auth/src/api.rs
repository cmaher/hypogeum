use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LoginResponse {
    pub auth_id: Uuid,
    pub user_id: Uuid,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct TokenRequest {
    #[serde(default)]
    pub grant_type: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub expires_in: Option<usize>,
}

impl std::fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRequest")
            .field("grant_type", &self.grant_type)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("api_key", &"[redacted]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: usize,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"[redacted]")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_redacted(value: &impl std::fmt::Debug, secrets: &[&str]) {
        for output in [format!("{value:?}"), format!("{value:#?}")] {
            for secret in secrets {
                assert!(
                    !output.contains(secret),
                    "Debug output exposed a credential"
                );
            }
            assert!(output.contains("[redacted]"));
        }
    }

    #[test]
    fn login_request_debug_redacts_password() {
        let request = LoginRequest {
            username: "alice".into(),
            password: "private-password".into(),
        };
        assert_redacted(&request, &[&request.password]);
    }

    #[test]
    fn token_request_debug_redacts_client_secret_and_api_key() {
        let request = TokenRequest {
            client_secret: Some("private-client-secret".into()),
            api_key: Some("private-api-key".into()),
            ..Default::default()
        };
        assert_redacted(
            &request,
            &[
                request.client_secret.as_deref().unwrap(),
                request.api_key.as_deref().unwrap(),
            ],
        );
    }

    #[test]
    fn token_response_debug_redacts_access_token() {
        let response = TokenResponse {
            access_token: "private-access-token".into(),
            token_type: "Bearer".into(),
            expires_in: 3600,
        };
        assert_redacted(&response, &[&response.access_token]);
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationRequest {
    pub display_name: String,
    pub username: String,
    pub password: String,
}
impl std::fmt::Debug for RegistrationRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationRequest")
            .field("display_name", &self.display_name)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistrationResponse {
    pub id: Uuid,
    pub approval_status: exemplum_dmn_users::accounts::ApprovalStatus,
}
