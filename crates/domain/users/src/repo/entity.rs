use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use uuid::Uuid;

use crate::models::User;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(schema_name = "hypogeum", table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl From<Model> for User {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

impl From<&Model> for User {
    fn from(model: &Model) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name.clone(),
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

impl From<User> for Model {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            display_name: user.display_name,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

impl From<&User> for Model {
    fn from(user: &User) -> Self {
        Self {
            id: user.id,
            display_name: user.display_name.clone(),
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lossless_conversion() {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let user = User {
            id,
            display_name: "Test User".to_string(),
            created_at: now,
            updated_at: now,
        };

        let model = Model::from(user.clone());
        assert_eq!(model.id, user.id);
        assert_eq!(model.display_name, user.display_name);
        assert_eq!(model.created_at, user.created_at);
        assert_eq!(model.updated_at, user.updated_at);

        let roundtrip = User::from(model);
        assert_eq!(roundtrip, user);

        let ref_model = Model::from(&user);
        assert_eq!(ref_model.id, user.id);
        let ref_user = User::from(&ref_model);
        assert_eq!(ref_user, user);
    }
}
