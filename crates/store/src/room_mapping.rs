use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMapping {
    pub id: i64,
    pub matrix_room_id: String,
    pub platform_id: String,
    pub external_room_id: String,
}

impl Database {
    /// Create a new room mapping. Returns the inserted row ID.
    pub async fn create_room_mapping(
        &self,
        matrix_room_id: &str,
        platform_id: &str,
        external_room_id: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO room_mappings (matrix_room_id, platform_id, external_room_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![matrix_room_id, platform_id, external_room_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Find a room mapping by Matrix room ID and platform.
    pub async fn find_room_by_matrix_id(
        &self,
        matrix_room_id: &str,
        platform_id: &str,
    ) -> anyhow::Result<Option<RoomMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_room_id, platform_id, external_room_id FROM room_mappings WHERE matrix_room_id = ?1 AND platform_id = ?2",
        )?;
        let result = stmt.query_row(rusqlite::params![matrix_room_id, platform_id], |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        });
        match result {
            Ok(mapping) => Ok(Some(mapping)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Find a room mapping by external platform room ID.
    pub async fn find_room_by_external_id(
        &self,
        platform_id: &str,
        external_room_id: &str,
    ) -> anyhow::Result<Option<RoomMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_room_id, platform_id, external_room_id FROM room_mappings WHERE platform_id = ?1 AND external_room_id = ?2",
        )?;
        let result = stmt.query_row(rusqlite::params![platform_id, external_room_id], |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        });
        match result {
            Ok(mapping) => Ok(Some(mapping)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List all room mappings for a given platform.
    pub async fn list_room_mappings(&self, platform_id: &str) -> anyhow::Result<Vec<RoomMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_room_id, platform_id, external_room_id FROM room_mappings WHERE platform_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![platform_id], |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        })?;
        let mut mappings = Vec::new();
        for row in rows {
            mappings.push(row?);
        }
        Ok(mappings)
    }

    /// List all room mappings for a given Matrix room (across all platforms).
    pub async fn find_all_mappings_by_matrix_id(
        &self,
        matrix_room_id: &str,
    ) -> anyhow::Result<Vec<RoomMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_room_id, platform_id, external_room_id FROM room_mappings WHERE matrix_room_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![matrix_room_id], |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        })?;
        let mut mappings = Vec::new();
        for row in rows {
            mappings.push(row?);
        }
        Ok(mappings)
    }

    /// Delete a room mapping by ID.
    pub async fn delete_room_mapping(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock().await;
        let changed = conn.execute(
            "DELETE FROM room_mappings WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(changed > 0)
    }
}
