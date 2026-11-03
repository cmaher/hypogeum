use crate::accounts::{
    Account, AccountError, AccountPage, AccountQuery, AccountRepo, ApprovalStatus, ManagedAccount,
};
use async_trait::async_trait;
use hypgm_errors::RepoError;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, SqlxPostgresConnector, TransactionTrait,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) mod entity {
    use crate::accounts::ApprovalStatus;
    use sea_orm::entity::prelude::*;
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(schema_name = "exemplum", table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub display_name: String,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
        pub approval_status: ApprovalStatus,
        pub is_admin: bool,
        pub approved_at: Option<DateTimeUtc>,
        pub username: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
impl From<entity::Model> for Account {
    fn from(value: entity::Model) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            created_at: value.created_at,
            updated_at: value.updated_at,
            approval_status: value.approval_status,
            is_admin: value.is_admin,
            approved_at: value.approved_at,
        }
    }
}
impl From<entity::Model> for ManagedAccount {
    fn from(value: entity::Model) -> Self {
        Self {
            username: value.username.clone(),
            account: value.into(),
        }
    }
}
pub struct PgAccountRepo {
    db: DatabaseConnection,
}
impl PgAccountRepo {
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        }
    }
}
fn database(error: sea_orm::DbErr) -> AccountError {
    RepoError::database(error).into()
}

#[async_trait]
impl AccountRepo for PgAccountRepo {
    async fn ensure_account(
        &self,
        id: Uuid,
        username: &str,
        display_name: &str,
    ) -> Result<Account, AccountError> {
        // INSERT ... ON CONFLICT (id) DO NOTHING: concurrent first requests
        // for one identity converge on a single pending row.
        entity::Entity::insert(entity::ActiveModel {
            id: Set(id),
            display_name: Set(display_name.to_owned()),
            username: Set(Some(username.to_owned())),
            approval_status: Set(ApprovalStatus::Pending),
            is_admin: Set(false),
            ..Default::default()
        })
        .on_conflict(
            OnConflict::column(entity::Column::Id)
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(&self.db)
        .await
        .map_err(database)?;
        self.get(id).await
    }
    async fn promote_admin(&self, username: &str) -> Result<Account, AccountError> {
        let tx = self.db.begin().await.map_err(database)?;
        let account = entity::Entity::find()
            .filter(entity::Column::Username.eq(username))
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database)?
            .ok_or(RepoError::NotFound)?;
        let account = if account.is_admin && account.approval_status == ApprovalStatus::Approved {
            account
        } else {
            let approved_at = account.approved_at.or_else(|| Some(chrono::Utc::now()));
            let mut account: entity::ActiveModel = account.into();
            account.is_admin = Set(true);
            account.approval_status = Set(ApprovalStatus::Approved);
            account.approved_at = Set(approved_at);
            account.update(&tx).await.map_err(database)?
        };
        tx.commit().await.map_err(database)?;
        Ok(account.into())
    }
    async fn approve(&self, id: Uuid) -> Result<ManagedAccount, AccountError> {
        let tx = self.db.begin().await.map_err(database)?;
        let account = entity::Entity::find_by_id(id)
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database)?
            .ok_or(RepoError::NotFound)?;
        let account = if account.approval_status == ApprovalStatus::Pending {
            let mut account: entity::ActiveModel = account.into();
            account.approval_status = Set(ApprovalStatus::Approved);
            account.approved_at = Set(Some(chrono::Utc::now()));
            account.update(&tx).await.map_err(database)?
        } else {
            account
        };
        tx.commit().await.map_err(database)?;
        Ok(account.into())
    }
    async fn get(&self, id: Uuid) -> Result<Account, AccountError> {
        entity::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(database)?
            .map(Into::into)
            .ok_or_else(|| RepoError::NotFound.into())
    }
    async fn list(&self, query: AccountQuery) -> Result<AccountPage, AccountError> {
        let page = query.page;
        let mut select = entity::Entity::find()
            .order_by_desc(entity::Column::CreatedAt)
            .order_by_desc(entity::Column::Id)
            .limit(u64::from(page.limit()) + 1);
        if let Some(status) = query.status {
            select = select.filter(entity::Column::ApprovalStatus.eq(status));
        }
        if let Some(after) = page.after() {
            // Keyset pagination: resume strictly after the cursor's (created_at, id).
            let cursor = entity::Entity::find_by_id(after)
                .one(&self.db)
                .await
                .map_err(database)?
                .ok_or_else(|| RepoError::InvalidInput("Unknown page cursor".into()))?;
            select = select.filter(
                Condition::any()
                    .add(entity::Column::CreatedAt.lt(cursor.created_at))
                    .add(
                        entity::Column::CreatedAt
                            .eq(cursor.created_at)
                            .and(entity::Column::Id.lt(cursor.id)),
                    ),
            );
        }
        let mut accounts = select.all(&self.db).await.map_err(database)?;
        let more = accounts.len() > page.limit() as usize;
        accounts.truncate(page.limit() as usize);
        let next_after = if more {
            accounts.last().map(|account| account.id)
        } else {
            None
        };
        Ok(AccountPage {
            accounts: accounts.into_iter().map(Into::into).collect(),
            next_after,
        })
    }
}
