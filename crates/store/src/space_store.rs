use serde::Serialize;

use crate::Database;

/// A platform-to-space mapping record.
#[derive(Debug, Clone, Serialize)]
pub struct PlatformSpace {
    pub id: i64,
    pub platform_id: String,
    pub matrix_space_id: String,
}

impl Database {
    /// Get the Matrix Space ID for a platform, if one exists.
    pub async fn get_platform_space(&self, platform_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.lock().await;
        let mut stmt =
            conn.prepare("SELECT matrix_space_id FROM platform_spaces WHERE platform_id = ?1")?;
        let result = stmt
            .query_row(rusqlite::params![platform_id], |row| row.get(0))
            .optional()?;
        Ok(result)
    }

    /// Store the Matrix Space ID for a platform.
    /// Uses INSERT OR REPLACE so it's idempotent.
    pub async fn set_platform_space(
        &self,
        platform_id: &str,
        matrix_space_id: &str,
    ) -> anyhow::Result<()> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO platform_spaces (platform_id, matrix_space_id)
             VALUES (?1, ?2)
             ON CONFLICT(platform_id) DO UPDATE SET matrix_space_id = excluded.matrix_space_id",
            rusqlite::params![platform_id, matrix_space_id],
        )?;
        Ok(())
    }

    /// List all platform space mappings.
    pub async fn list_platform_spaces(&self) -> anyhow::Result<Vec<PlatformSpace>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, platform_id, matrix_space_id FROM platform_spaces ORDER BY platform_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PlatformSpace {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                matrix_space_id: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// Delete the space mapping for a platform.
    pub async fn delete_platform_space(&self, platform_id: &str) -> anyhow::Result<bool> {
        let conn = self.lock().await;
        let affected = conn.execute(
            "DELETE FROM platform_spaces WHERE platform_id = ?1",
            rusqlite::params![platform_id],
        )?;
        Ok(affected > 0)
    }
}

/// Re-export the `optional` helper used by rusqlite.
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        assert!(db.get_platform_space("telegram").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_and_get_platform_space() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        db.set_platform_space("telegram", "!space1:example.com")
            .await
            .unwrap();
        let result = db.get_platform_space("telegram").await.unwrap();
        assert_eq!(result.as_deref(), Some("!space1:example.com"));
    }

    #[tokio::test]
    async fn set_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        db.set_platform_space("telegram", "!old:example.com")
            .await
            .unwrap();
        db.set_platform_space("telegram", "!new:example.com")
            .await
            .unwrap();
        let result = db.get_platform_space("telegram").await.unwrap();
        assert_eq!(result.as_deref(), Some("!new:example.com"));
    }

    #[tokio::test]
    async fn list_platform_spaces() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        db.set_platform_space("discord", "!d:example.com")
            .await
            .unwrap();
        db.set_platform_space("telegram", "!t:example.com")
            .await
            .unwrap();

        let spaces = db.list_platform_spaces().await.unwrap();
        assert_eq!(spaces.len(), 2);
        assert_eq!(spaces[0].platform_id, "discord");
        assert_eq!(spaces[1].platform_id, "telegram");
    }

    #[tokio::test]
    async fn delete_platform_space() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();

        db.set_platform_space("telegram", "!t:example.com")
            .await
            .unwrap();
        assert!(db.delete_platform_space("telegram").await.unwrap());
        assert!(!db.delete_platform_space("telegram").await.unwrap());
        assert!(db.get_platform_space("telegram").await.unwrap().is_none());
    }
}
