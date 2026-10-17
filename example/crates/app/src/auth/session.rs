use sqlx::PgPool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

pub fn create_session_store(pool: PgPool) -> Result<PostgresStore, String> {
    PostgresStore::new(pool)
        .with_schema_name("exemplum")
        .map_err(|e| e.to_string())?
        .with_table_name("sessions")
        .map_err(|e| e.to_string())
}

pub fn init_session_store(pool: PgPool) -> PostgresStore {
    create_session_store(pool).expect("valid session store schema and table configuration")
}

pub fn create_session_layer(store: PostgresStore) -> SessionManagerLayer<PostgresStore> {
    SessionManagerLayer::new(store)
}
