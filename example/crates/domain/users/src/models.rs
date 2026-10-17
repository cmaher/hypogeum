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

pub(crate) fn validate_name(name: &str) -> Result<(), RepoError> {
    if name.is_empty() || name.trim() != name || name.chars().count() > 200 || name.contains('\0') {
        return Err(RepoError::InvalidInput(
            "Display name must be trimmed and contain 1..200 characters without NUL".into(),
        ));
    }
    Ok(())
}
