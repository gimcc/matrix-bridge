use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Puppet {
    pub id: i64,
    pub matrix_user_id: String,
    pub platform_id: String,
    pub external_user_id: String,
    pub display_name: Option<String>,
    pub avatar_mxc: Option<String>,
}

impl Database {
    /// Create or update a puppet user record.
    pub async fn upsert_puppet(
        &self,
        matrix_user_id: &str,
        platform_id: &str,
        external_user_id: &str,
        display_name: Option<&str>,
        avatar_mxc: Option<&str>,
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO puppets (matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(matrix_user_id) DO UPDATE SET
               display_name = excluded.display_name,
               avatar_mxc = excluded.avatar_mxc,
               updated_at = datetime('now')",
            rusqlite::params![matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Find a puppet by its Matrix user ID.
    pub async fn find_puppet_by_matrix_id(
        &self,
        matrix_user_id: &str,
    ) -> anyhow::Result<Option<Puppet>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc FROM puppets WHERE matrix_user_id = ?1",
        )?;
        let result = stmt.query_row(rusqlite::params![matrix_user_id], |row| {
            Ok(Puppet {
                id: row.get(0)?,
                matrix_user_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_user_id: row.get(3)?,
                display_name: row.get(4)?,
                avatar_mxc: row.get(5)?,
            })
        });
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Find a puppet by external platform user ID.
    pub async fn find_puppet_by_external_id(
        &self,
        platform_id: &str,
        external_user_id: &str,
    ) -> anyhow::Result<Option<Puppet>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc FROM puppets WHERE platform_id = ?1 AND external_user_id = ?2",
        )?;
        let result = stmt.query_row(rusqlite::params![platform_id, external_user_id], |row| {
            Ok(Puppet {
                id: row.get(0)?,
                matrix_user_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_user_id: row.get(3)?,
                display_name: row.get(4)?,
                avatar_mxc: row.get(5)?,
            })
        });
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
