use super::UserRepo;
use crate::models::{PageRequest, User, UserPage, validate_name};
use async_trait::async_trait;
use errors::RepoError;
use sea_query::{Expr, ExprTrait, Order, PostgresQueryBuilder, Query};
use sea_query_sqlx::{SqlxBinder, SqlxValues};
use sqlx::PgPool;
use uuid::Uuid;

const COLUMNS: [&str; 4] = ["id", "display_name", "created_at", "updated_at"];

pub struct PgUserRepo {
    pool: PgPool,
}
impl PgUserRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn one(&self, statement: (String, SqlxValues)) -> Result<User, RepoError> {
        let (sql, values) = statement;
        // Audited SeaQuery output, with all caller data bound as parameters.
        sqlx::query_as_with(sqlx::AssertSqlSafe(sql.as_str()), values)
            .fetch_optional(&self.pool)
            .await
            .map_err(RepoError::database)?
            .ok_or(RepoError::NotFound)
    }
}

#[async_trait]
impl UserRepo for PgUserRepo {
    async fn create(&self, id: Uuid, name: &str) -> Result<User, RepoError> {
        validate_name(name)?;
        let statement = Query::insert()
            .into_table(("app", "users"))
            .columns(["id", "display_name"])
            .values_panic([id.into(), name.into()])
            .returning(Query::returning().columns(COLUMNS))
            .build_sqlx(PostgresQueryBuilder);
        self.one(statement).await
    }

    async fn get(&self, id: Uuid) -> Result<User, RepoError> {
        self.one(
            Query::select()
                .columns(COLUMNS)
                .from(("app", "users"))
                .and_where(Expr::col("id").eq(id))
                .build_sqlx(PostgresQueryBuilder),
        )
        .await
    }

    async fn list(&self, page: PageRequest) -> Result<UserPage, RepoError> {
        let mut query = Query::select();
        query
            .columns(COLUMNS)
            .from(("app", "users"))
            .order_by("id", Order::Asc)
            .limit(u64::from(page.limit()) + 1);
        if let Some(after) = page.after() {
            query.and_where(Expr::col("id").gt(after));
        }
        let (sql, values) = query.build_sqlx(PostgresQueryBuilder);
        let mut users: Vec<User> = sqlx::query_as_with(sqlx::AssertSqlSafe(sql.as_str()), values)
            .fetch_all(&self.pool)
            .await
            .map_err(RepoError::database)?;
        let more = users.len() > page.limit() as usize;
        users.truncate(page.limit() as usize);
        let next_after = if more {
            users.last().map(|user| user.id)
        } else {
            None
        };
        Ok(UserPage { users, next_after })
    }

    async fn update_name(&self, id: Uuid, name: &str) -> Result<User, RepoError> {
        validate_name(name)?;
        self.one(
            Query::update()
                .table(("app", "users"))
                .value("display_name", name)
                .value("updated_at", Expr::current_timestamp())
                .and_where(Expr::col("id").eq(id))
                .returning(Query::returning().columns(COLUMNS))
                .build_sqlx(PostgresQueryBuilder),
        )
        .await
    }

    async fn delete(&self, id: Uuid) -> Result<(), RepoError> {
        let (sql, values) = Query::delete()
            .from_table(("app", "users"))
            .and_where(Expr::col("id").eq(id))
            .build_sqlx(PostgresQueryBuilder);
        let result = sqlx::query_with(sqlx::AssertSqlSafe(sql.as_str()), values)
            .execute(&self.pool)
            .await
            .map_err(RepoError::database)?;
        if result.rows_affected() == 0 {
            Err(RepoError::NotFound)
        } else {
            Ok(())
        }
    }
}

pub async fn check_ready(pool: &PgPool) -> Result<(), RepoError> {
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(pool)
        .await
        .map_err(RepoError::database)?;
    if role != "foundation_users" {
        return Err(RepoError::InvalidInput(
            "Use foundation_users runtime credentials".into(),
        ));
    }
    let version: i64 = sqlx::query_scalar("SELECT app.schema_version()")
        .fetch_one(pool)
        .await
        .map_err(RepoError::database)?;
    if version != 1 {
        return Err(RepoError::InvalidInput(
            "Users schema is not ready; run explicit migrations".into(),
        ));
    }
    Ok(())
}
