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

        info!("database migrations applied");
        Ok(())
    }

    /// Get a lock on the underlying connection.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }

    /// Count all room mappings.
    pub async fn count_room_mappings(&self) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        let count = conn
            .prepare("SELECT COUNT(*) FROM room_mappings")?
            .query_row([], |row| row.get(0))?;
        Ok(count)
    }

    /// Count all webhooks.
    pub async fn count_webhooks(&self) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        let count = conn
            .prepare("SELECT COUNT(*) FROM webhooks")?
            .query_row([], |row| row.get(0))?;
        Ok(count)
    }

    /// Count all message mappings.
    pub async fn count_message_mappings(&self) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        let count = conn
            .prepare("SELECT COUNT(*) FROM message_mappings")?
            .query_row([], |row| row.get(0))?;
        Ok(count)
    }

    /// Count all puppet users.
    pub async fn count_puppets(&self) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        let count = conn
            .prepare("SELECT COUNT(*) FROM puppets")?
            .query_row([], |row| row.get(0))?;
        Ok(count)
    }

    /// List distinct platform IDs that have room mappings.
    pub async fn list_active_platforms(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.lock().await;
        let mut stmt =
            conn.prepare("SELECT DISTINCT platform_id FROM room_mappings ORDER BY platform_id")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut platforms = Vec::new();
        for row in rows {
            platforms.push(row?);
        }
        Ok(platforms)
    }
}
