use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::info;

/// Thread-safe wrapper around a rusqlite Connection.
/// Uses tokio::sync::Mutex for async-compatible locking.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open (or create) the SQLite database at the given path.
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run all migrations to set up the schema.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch(include_str!("migrations/001_initial.sql"))?;
        conn.execute_batch(include_str!("migrations/002_webhooks.sql"))?;
        conn.execute_batch(include_str!(
            "migrations/003_message_mapping_multi_platform.sql"
        ))?;

        // Migration 004: Add exclude_sources column (idempotent).
        // SQLite has no "ALTER TABLE ... ADD COLUMN IF NOT EXISTS",
        // so we check the table schema first.
        let has_column: bool = conn
            .prepare(
                "SELECT COUNT(*) FROM pragma_table_info('webhooks') WHERE name = 'exclude_sources'",
            )?
            .query_row([], |row| row.get::<_, i64>(0))
            .map(|count| count > 0)?;
        if !has_column {
            conn.execute_batch(
                "ALTER TABLE webhooks ADD COLUMN exclude_sources TEXT NOT NULL DEFAULT ''",
            )?;
        }

        info!("database migrations applied");
        Ok(())
    }

    /// Get a lock on the underlying connection.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }
}
