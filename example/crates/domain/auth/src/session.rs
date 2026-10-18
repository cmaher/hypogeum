use async_trait::async_trait;
use sqlx::{AssertSqlSafe, PgPool};
use tower_sessions::SessionStore;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, Error, ExpiredDeletion};

/// PostgreSQL-backed session store implementing [`tower_sessions::SessionStore`].
///
/// ### Why raw SQL (`sqlx`) is used instead of SeaORM:
///
/// 1. **Upstream Infrastructure Contract**:
///    This struct is an HTTP middleware adapter satisfying [`SessionStore`] and
///    [`ExpiredDeletion`], rather than a domain repository. Sessions are opaque key-value
///    blobs (`(id, data BYTEA, expiry_date)`) serialized via MessagePack ([`rmp_serde`]),
///    not relational domain entities with business rules or relations.
///
/// 2. **Per-Request Performance & Minimal Overhead**:
///    Session loading and saving run on every authenticated HTTP request. Executing direct
///    parameterized SQL statements against [`PgPool`] avoids ORM entity reflection,
///    ActiveModel change tracking, and query generation overhead on the critical request path.
///
/// 3. **Dynamic Schema & Table Qualification**:
///    SeaORM statically binds schemas and tables via compile-time derive macros
///    (`#[sea_orm(schema_name = "...")]`). This store allows configuring the target schema
///    and table dynamically at runtime (e.g. per-environment or multi-tenant namespaces),
///    validating identifiers with [`hypgmx_rdbms::contract::validate_identifier`].
///
/// 4. **Database-Native Upsert and Expiration**:
///    Session management relies on native PostgreSQL atomic upserts (`ON CONFLICT (id) DO UPDATE`)
///    and server-evaluated timestamps (`statement_timestamp()`). Directly writing these 4 queries
///    avoids ORM type-mapping impedance (such as `time::OffsetDateTime` vs `chrono::DateTime`).
///
/// 5. **Dependency Decoupling (`sqlx 0.9` compatibility)**:
///    The official `tower-sessions-sqlx-store` crate was pinned to `sqlx 0.8`, which conflicted
///    with the workspace's `sqlx 0.9` dependency. Maintaining this focused store in-tree ensures
///    direct compatibility with the workspace connection pool without mismatched transitive dependencies.
#[derive(Clone, Debug)]
pub struct PostgresSessionStore {
    pool: PgPool,
    schema: String,
    table: String,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self::new_with_schema(pool, "exemplum")
    }

    pub fn new_with_schema(pool: PgPool, schema: impl Into<String>) -> Self {
        let schema = schema.into();
        hypgmx_rdbms::contract::validate_identifier(&schema)
            .unwrap_or_else(|e| panic!("invalid schema identifier {schema:?}: {e}"));
        hypgmx_rdbms::contract::validate_identifier("sessions")
            .unwrap_or_else(|e| panic!("invalid table identifier 'sessions': {e}"));
        Self {
            pool,
            schema,
            table: "sessions".into(),
        }
    }

    pub fn with_schema_name(mut self, schema: impl Into<String>) -> Self {
        let schema = schema.into();
        hypgmx_rdbms::contract::validate_identifier(&schema)
            .unwrap_or_else(|e| panic!("invalid schema identifier {schema:?}: {e}"));
        self.schema = schema;
        self
    }

    pub fn with_table_name(mut self, table: impl Into<String>) -> Self {
        let table = table.into();
        hypgmx_rdbms::contract::validate_identifier(&table)
            .unwrap_or_else(|e| panic!("invalid table identifier {table:?}: {e}"));
        self.table = table;
        self
    }

    pub async fn delete_expired(&self) -> session_store::Result<()> {
        <Self as ExpiredDeletion>::delete_expired(self).await
    }
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let query = format!(
            "INSERT INTO \"{}\".\"{}\" (id, data, expiry_date) VALUES ($1, $2, $3)",
            self.schema, self.table
        );
        let data = rmp_serde::to_vec(&record).map_err(|e| Error::Encode(e.to_string()))?;
        sqlx::query(AssertSqlSafe(query))
            .bind(record.id.to_string())
            .bind(data)
            .bind(record.expiry_date)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let query = format!(
            "INSERT INTO \"{}\".\"{}\" (id, data, expiry_date) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data, expiry_date = EXCLUDED.expiry_date",
            self.schema, self.table
        );
        let data = rmp_serde::to_vec(&record).map_err(|e| Error::Encode(e.to_string()))?;
        sqlx::query(AssertSqlSafe(query))
            .bind(record.id.to_string())
            .bind(data)
            .bind(record.expiry_date)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let query = format!(
            "SELECT data FROM \"{}\".\"{}\" WHERE id = $1 AND expiry_date > statement_timestamp()",
            self.schema, self.table
        );
        let row: Option<(Vec<u8>,)> = sqlx::query_as(AssertSqlSafe(query))
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;

        match row {
            Some((data,)) => {
                let record =
                    rmp_serde::from_slice(&data).map_err(|e| Error::Decode(e.to_string()))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        let query = format!(
            "DELETE FROM \"{}\".\"{}\" WHERE id = $1",
            self.schema, self.table
        );
        sqlx::query(AssertSqlSafe(query))
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for PostgresSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let query = format!(
            "DELETE FROM \"{}\".\"{}\" WHERE expiry_date <= statement_timestamp()",
            self.schema, self.table
        );
        sqlx::query(AssertSqlSafe(query))
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }
}
