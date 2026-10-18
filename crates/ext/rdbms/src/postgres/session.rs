use async_trait::async_trait;
use sqlx::{AssertSqlSafe, PgPool};
use tower_sessions::SessionStore;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, Error};

#[derive(Clone, Debug)]
pub struct PostgresSessionStore {
    pool: PgPool,
    schema: String,
    table: String,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            schema: "public".into(),
            table: "sessions".into(),
        }
    }

    pub fn new_with_schema(pool: PgPool, schema: impl Into<String>) -> Self {
        Self {
            pool,
            schema: schema.into(),
            table: "sessions".into(),
        }
    }

    pub fn with_schema_name(mut self, schema: impl Into<String>) -> Self {
        self.schema = schema.into();
        self
    }

    pub fn with_table_name(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    fn check_identifiers(&self) -> Result<(), Error> {
        validate_identifier(&self.schema)?;
        validate_identifier(&self.table)?;
        Ok(())
    }
}

fn validate_identifier(ident: &str) -> Result<(), Error> {
    if ident.is_empty()
        || ident.len() > 63
        || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || !ident.chars().next().unwrap().is_ascii_alphabetic()
    {
        return Err(Error::Backend(format!("Invalid SQL identifier: {ident}")));
    }
    Ok(())
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        self.check_identifiers()?;
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
        self.check_identifiers()?;
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
        self.check_identifiers()?;
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
        self.check_identifiers()?;
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
