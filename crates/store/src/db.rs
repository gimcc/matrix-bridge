use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::info;

/// Embedded migration scripts, ordered by version number.
/// Each entry is `(version, name, sql)`.
const MIGRATIONS: &[(i64, &str, &str)] =
    &[(1, "initial", include_str!("migrations/001_initial.sql"))];

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

    /// Run all pending migrations to set up/update the schema.
    ///
    /// Tracks applied versions in a `_schema_version` table.
    /// Each migration runs inside a transaction; already-applied
    /// migrations are skipped.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;

        // Create the version tracking table if it doesn't exist.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _schema_version (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;

        let current_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut applied = 0;
        for &(version, name, sql) in MIGRATIONS {
            if version <= current_version {
                continue;
            }

            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _schema_version (version, name) VALUES (?1, ?2)",
                rusqlite::params![version, name],
            )?;
            tx.commit()?;

            info!(version, name, "migration applied");
            applied += 1;
        }

        if applied == 0 {
            info!(current_version, "database schema up to date");
        } else {
            info!(
                applied,
                total = MIGRATIONS.len(),
                "database migrations complete"
            );
        }

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
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List all known platform IDs across room mappings and webhooks (deduplicated).
    pub async fn list_all_platforms(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT platform_id FROM (
                SELECT platform_id FROM room_mappings
                UNION
                SELECT platform_id FROM webhooks WHERE enabled = 1
             ) ORDER BY platform_id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrate_creates_schema_version_table() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        let conn = db.lock().await;
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM _schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        let expected_max: i64 = MIGRATIONS.last().map(|(v, _, _)| *v).unwrap_or(0);
        assert_eq!(version, expected_max);
    }

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();

        let conn = db.lock().await;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM _schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[tokio::test]
    async fn migrate_applies_all_tables() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        // Verify all tables exist by running count queries.
        assert_eq!(db.count_room_mappings().await.unwrap(), 0);
        assert_eq!(db.count_webhooks().await.unwrap(), 0);
        assert_eq!(db.count_message_mappings().await.unwrap(), 0);
        assert_eq!(db.count_puppets().await.unwrap(), 0);
    }
}
