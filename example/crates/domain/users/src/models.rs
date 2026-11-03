use chrono::{DateTime, Utc};
use hypgm_errors::RepoError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub struct PageRequest {
    after: Option<Uuid>,
    limit: u32,
}
impl PageRequest {
    pub fn new(after: Option<Uuid>, limit: u32) -> Result<Self, RepoError> {
        if !(1..=100).contains(&limit) {
            return Err(RepoError::InvalidInput("Page size must be 1..100".into()));
        }
        Ok(Self { after, limit })
    }
    pub fn after(&self) -> Option<Uuid> {
        self.after
    }
    pub fn limit(&self) -> u32 {
        self.limit
    }
}
impl Default for PageRequest {
    fn default() -> Self {
        Self {
            after: None,
            limit: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserPage {
    pub users: Vec<User>,
    pub next_after: Option<Uuid>,
}

/// Display names hold 1..=200 characters, no control characters, and no
/// leading or trailing whitespace. This is exactly the identity schema's
/// `display_name` pattern (`\s`, `\p{Z}` and `\p{Cc}` together are Unicode
/// `White_Space` plus `Cc`), so a name Kratos accepts is always storable.
pub fn validate_name(name: &str) -> Result<(), RepoError> {
    let edge = |c: char| c.is_whitespace() || c.is_control();
    if name.is_empty()
        || name.chars().count() > 200
        || name.chars().any(char::is_control)
        || name.starts_with(edge)
        || name.ends_with(edge)
    {
        return Err(RepoError::InvalidInput(
            "Display name must contain 1..200 characters without control characters \
             or surrounding whitespace"
                .into(),
        ));
    }
    Ok(())
}
