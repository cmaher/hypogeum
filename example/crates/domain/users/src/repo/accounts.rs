use crate::{
    accounts::{
        Account, AccountError, AccountPage, AccountQuery, AccountRepo, ApprovalStatus,
        ManagedAccount, normalize_username,
    },
    models::validate_name,
};
use async_trait::async_trait;
use hypgm_errors::RepoError;
use sea_orm::sea_query::{Alias, Expr, Query};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    ExprTrait, QueryFilter, QueryOrder, QuerySelect, Set, SqlxPostgresConnector, TransactionTrait,
};
use sqlx::PgPool;
use std::collections::HashMap;
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
pub struct PgAccountRepo {
    db: DatabaseConnection,
}
impl PgAccountRepo {
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        }
    }
    /// Create a new approved administrator. A username that already belongs to
    /// an account fails with `DuplicateUsername`; existing accounts change only
    /// through [`Self::promote_admin`].
    pub async fn create_admin(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError> {
        let username = normalize_username(username)?;
        self.insert_account(display_name.trim(), username, password_hash, true)
            .await
    }
    /// Explicitly make the account behind an active password identity an
    /// approved administrator. Credentials and profile data are never replaced.
    pub async fn promote_admin(&self, username: &str) -> Result<Account, AccountError> {
        let username = normalize_username(username)?;
        let tx = self.db.begin().await.map_err(database)?;
        let lookup = Query::select()
            .column(Alias::new("user_id"))
            .from((Alias::new("exemplum"), Alias::new("auth_identities")))
            .and_where(Expr::col(Alias::new("identity_type")).eq("password"))
            .and_where(Expr::col(Alias::new("identifier")).eq(username))
            .and_where(Expr::col(Alias::new("revoked_at")).is_null())
            .to_owned();
        let identity = tx
            .query_one(&lookup)
            .await
            .map_err(database)?
            .ok_or(RepoError::NotFound)?;
        let id: Uuid = identity.try_get("", "user_id").map_err(database)?;
        let account = entity::Entity::find_by_id(id)
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
    /// Atomically insert an account and its password identity. Administrators
    /// start approved; registrations start pending.
    async fn insert_account(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
        is_admin: bool,
    ) -> Result<Account, AccountError> {
        validate_name(display_name).map_err(|_| AccountError::Validation)?;
        if normalize_username(username)? != username
            || password_hash.is_empty()
            || password_hash.contains('\0')
        {
            return Err(AccountError::Validation);
        }
        let tx = self.db.begin().await.map_err(database)?;
        let account = entity::ActiveModel {
            id: Set(Uuid::new_v4()),
            display_name: Set(display_name.to_owned()),
            approval_status: Set(if is_admin {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Pending
            }),
            is_admin: Set(is_admin),
            approved_at: Set(is_admin.then(chrono::Utc::now)),
            ..Default::default()
        }
        .insert(&tx)
        .await
        .map_err(database)?;
        let insert = Query::insert()
            .into_table((Alias::new("exemplum"), Alias::new("auth_identities")))
            .columns(
                [
                    "id",
                    "user_id",
                    "identity_type",
                    "identifier",
                    "secret_hash",
                ]
                .map(Alias::new),
            )
            .values_panic([
                Uuid::new_v4().into(),
                account.id.into(),
                "password".into(),
                username.into(),
                password_hash.into(),
            ])
            .to_owned();
        if let Err(error) = tx.execute(&insert).await {
            let duplicate = match &error {
                sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(error)) => {
                    matches!(error.as_ref(), sqlx::Error::Database(error) if error.constraint() == Some("idx_auth_identities_identity_unique"))
                }
                _ => false,
            };
            tx.rollback().await.map_err(database)?;
            return Err(if duplicate {
                AccountError::DuplicateUsername
            } else {
                database(error)
            });
        }
        tx.commit().await.map_err(database)?;
        Ok(account.into())
    }
}
fn database(error: sea_orm::DbErr) -> AccountError {
    RepoError::database(error).into()
}

/// Pair accounts with their active password usernames in one lookup.
async fn with_usernames(
    db: &impl ConnectionTrait,
    accounts: Vec<entity::Model>,
) -> Result<Vec<ManagedAccount>, AccountError> {
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    let lookup = Query::select()
        .columns([Alias::new("user_id"), Alias::new("identifier")])
        .from((Alias::new("exemplum"), Alias::new("auth_identities")))
        .and_where(Expr::col(Alias::new("user_id")).is_in(accounts.iter().map(|a| a.id)))
        .and_where(Expr::col(Alias::new("identity_type")).eq("password"))
        .and_where(Expr::col(Alias::new("revoked_at")).is_null())
        .to_owned();
    let mut usernames = HashMap::new();
    for row in db.query_all(&lookup).await.map_err(database)? {
        let user_id: Uuid = row.try_get("", "user_id").map_err(database)?;
        let identifier: String = row.try_get("", "identifier").map_err(database)?;
        usernames.insert(user_id, identifier);
    }
    Ok(accounts
        .into_iter()
        .map(|account| ManagedAccount {
            username: usernames.remove(&account.id),
            account: account.into(),
        })
        .collect())
}

#[async_trait]
impl AccountRepo for PgAccountRepo {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<Account, AccountError> {
        self.insert_account(display_name, username, password_hash, false)
            .await
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
        let mut accounts = with_usernames(&tx, vec![account]).await?;
        tx.commit().await.map_err(database)?;
        Ok(accounts.remove(0))
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
            accounts: with_usernames(&self.db, accounts).await?,
            next_after,
        })
    }
}
