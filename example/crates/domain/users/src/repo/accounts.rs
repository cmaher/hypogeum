use crate::{
    accounts::{
        Account, AccountError, AccountPage, AccountRepo, ApprovalStatus, normalize_username,
    },
    models::{PageRequest, validate_name},
};
use async_trait::async_trait;
use hypgm_errors::RepoError;
use sea_orm::sea_query::{Alias, Query};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, ExprTrait,
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
        pub welcome_scheduled_at: Option<DateTimeUtc>,
        pub welcome_processed_at: Option<DateTimeUtc>,
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
            welcome_scheduled_at: value.welcome_scheduled_at,
            welcome_processed_at: value.welcome_processed_at,
        }
    }
}
pub struct PgAccountRepo {
    db: DatabaseConnection,
    pool: PgPool,
}
impl PgAccountRepo {
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone()),
            pool,
        }
    }
    /// Create an approved administrator, or promote an existing password identity.
    /// Existing identity credentials and profile data are never replaced.
    pub async fn bootstrap_admin(
        &self,
        display_name: &str,
        username: &str,
        password_hash: Option<&str>,
    ) -> Result<Account, AccountError> {
        let username = normalize_username(username)?;
        let display_name = display_name.trim();
        validate_name(display_name).map_err(|_| AccountError::Validation)?;
        let tx = self.db.begin().await.map_err(database)?;
        let lookup = Query::select()
            .column(Alias::new("user_id"))
            .from((Alias::new("exemplum"), Alias::new("auth_identities")))
            .and_where(sea_orm::sea_query::Expr::col(Alias::new("identity_type")).eq("password"))
            .and_where(sea_orm::sea_query::Expr::col(Alias::new("identifier")).eq(username))
            .and_where(sea_orm::sea_query::Expr::col(Alias::new("revoked_at")).is_null())
            .to_owned();
        let identity = tx.query_one(&lookup).await.map_err(database)?;
        let account = if let Some(identity) = identity {
            let id: Uuid = identity.try_get("", "user_id").map_err(database)?;
            let account = entity::Entity::find_by_id(id)
                .lock_exclusive()
                .one(&tx)
                .await
                .map_err(database)?
                .ok_or(RepoError::NotFound)?;
            if account.is_admin && account.approval_status == ApprovalStatus::Approved {
                account
            } else {
                let approved_at = account.approved_at.or_else(|| Some(chrono::Utc::now()));
                let mut account: entity::ActiveModel = account.into();
                account.is_admin = Set(true);
                account.approval_status = Set(ApprovalStatus::Approved);
                account.approved_at = Set(approved_at);
                account.update(&tx).await.map_err(database)?
            }
        } else {
            let hash = password_hash.ok_or(RepoError::NotFound)?;
            if hash.is_empty() || hash.contains('\0') {
                return Err(AccountError::Validation);
            }
            let account = entity::ActiveModel {
                id: Set(Uuid::new_v4()),
                display_name: Set(display_name.to_owned()),
                approval_status: Set(ApprovalStatus::Approved),
                is_admin: Set(true),
                approved_at: Set(Some(chrono::Utc::now())),
                welcome_scheduled_at: Set(None),
                welcome_processed_at: Set(None),
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
                    hash.into(),
                ])
                .to_owned();
            tx.execute(&insert).await.map_err(database)?;
            account
        };
        tx.commit().await.map_err(database)?;
        Ok(account.into())
    }
    /// Persist the welcome effect once; missing targets are successful no-ops.
    pub async fn process_welcome(&self, id: Uuid) -> Result<(), AccountError> {
        let tx = self.db.begin().await.map_err(database)?;
        if let Some(account) = entity::Entity::find_by_id(id)
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database)?
        {
            if account.approval_status != ApprovalStatus::Approved {
                return Err(AccountError::Validation);
            }
            if account.welcome_processed_at.is_none() {
                let mut account: entity::ActiveModel = account.into();
                account.welcome_processed_at = Set(Some(chrono::Utc::now()));
                account.update(&tx).await.map_err(database)?;
            }
        }
        tx.commit().await.map_err(database)?;
        Ok(())
    }
}
fn database(error: sea_orm::DbErr) -> AccountError {
    RepoError::database(error).into()
}

#[async_trait]
impl AccountRepo for PgAccountRepo {
    async fn register(
        &self,
        display_name: &str,
        username: &str,
        password_hash: &str,
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
            approval_status: Set(ApprovalStatus::Pending),
            is_admin: Set(false),
            approved_at: Set(None),
            welcome_scheduled_at: Set(None),
            welcome_processed_at: Set(None),
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
    async fn approve(&self, id: Uuid) -> Result<Account, AccountError> {
        use sea_orm::{
            QueryTrait,
            sea_query::{Expr, PostgresQueryBuilder},
        };
        use sea_query_sqlx::SqlxBinder;
        let mut tx = self.pool.begin().await.map_err(RepoError::database)?;
        // Upstream transactional enqueue refers to the hstore type without a schema.
        sqlx::query("SET LOCAL search_path = pg_catalog, apalis")
            .execute(&mut *tx)
            .await
            .map_err(RepoError::database)?;
        let mut select = entity::Entity::find_by_id(id)
            .select_only()
            .column(entity::Column::ApprovalStatus)
            .lock_exclusive();
        let (sql, args) = QueryTrait::query(&mut select).build_sqlx(PostgresQueryBuilder);
        // SeaQuery generates identifiers and placeholders; all input stays bound.
        let status: Option<(String,)> = sqlx::query_as_with(sqlx::AssertSqlSafe(sql), args)
            .fetch_optional(&mut *tx)
            .await
            .map_err(RepoError::database)?;
        let status = status.ok_or(RepoError::NotFound)?.0;
        if status == "pending" {
            let mut update = entity::Entity::update_many()
                .col_expr(
                    entity::Column::ApprovalStatus,
                    Expr::value(ApprovalStatus::Approved),
                )
                .col_expr(entity::Column::ApprovedAt, Expr::current_timestamp())
                .col_expr(
                    entity::Column::WelcomeScheduledAt,
                    Expr::current_timestamp(),
                )
                .filter(entity::Column::Id.eq(id));
            let (sql, args) = QueryTrait::query(&mut update).build_sqlx(PostgresQueryBuilder);
            sqlx::query_with(sqlx::AssertSqlSafe(sql), args)
                .execute(&mut *tx)
                .await
                .map_err(RepoError::database)?;
            let task = crate::jobs::welcome_task(id)
                .try_map_args(|args| serde_json::to_vec(&args))
                .map_err(RepoError::database)?;
            hypgmx_tasks::apalis_postgres::queries::push_tasks(
                &mut *tx,
                crate::jobs::WELCOME_QUEUE,
                vec![task],
            )
            .await
            .map_err(RepoError::database)?;
        }
        tx.commit().await.map_err(RepoError::database)?;
        self.get(id).await
    }
    async fn get(&self, id: Uuid) -> Result<Account, AccountError> {
        entity::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(database)?
            .map(Into::into)
            .ok_or_else(|| RepoError::NotFound.into())
    }
    async fn list(&self, page: PageRequest) -> Result<AccountPage, AccountError> {
        let mut query = entity::Entity::find()
            .order_by_asc(entity::Column::Id)
            .limit(u64::from(page.limit()) + 1);
        if let Some(after) = page.after() {
            query = query.filter(entity::Column::Id.gt(after));
        }
        let mut accounts: Vec<Account> = query
            .all(&self.db)
            .await
            .map_err(database)?
            .into_iter()
            .map(Into::into)
            .collect();
        let more = accounts.len() > page.limit() as usize;
        accounts.truncate(page.limit() as usize);
        let next_after = if more {
            accounts.last().map(|account| account.id)
        } else {
            None
        };
        Ok(AccountPage {
            accounts,
            next_after,
        })
    }
}
