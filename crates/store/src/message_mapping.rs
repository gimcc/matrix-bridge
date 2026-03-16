use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMapping {
    pub id: i64,
    pub matrix_event_id: String,
    pub platform_id: String,
    pub external_message_id: String,
    pub room_mapping_id: i64,
}

impl Database {
    /// Create a message mapping between a Matrix event and external message.
    pub async fn create_message_mapping(
        &self,
        matrix_event_id: &str,
        platform_id: &str,
        external_message_id: &str,
        room_mapping_id: i64,
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO message_mappings (matrix_event_id, platform_id, external_message_id, room_mapping_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![matrix_event_id, platform_id, external_message_id, room_mapping_id],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Find a message mapping by Matrix event ID.
    pub async fn find_message_by_matrix_id(
        &self,
        matrix_event_id: &str,
    ) -> anyhow::Result<Option<MessageMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id FROM message_mappings WHERE matrix_event_id = ?1",
        )?;
        let result = stmt.query_row(rusqlite::params![matrix_event_id], |row| {
            Ok(MessageMapping {
                id: row.get(0)?,
                matrix_event_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_message_id: row.get(3)?,
                room_mapping_id: row.get(4)?,
            })
        });
        match result {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Find a message mapping by external message ID.
    pub async fn find_message_by_external_id(
        &self,
        platform_id: &str,
        external_message_id: &str,
    ) -> anyhow::Result<Option<MessageMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id FROM message_mappings WHERE platform_id = ?1 AND external_message_id = ?2",
        )?;
        let result = stmt.query_row(rusqlite::params![platform_id, external_message_id], |row| {
            Ok(MessageMapping {
                id: row.get(0)?,
                matrix_event_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_message_id: row.get(3)?,
                room_mapping_id: row.get(4)?,
            })
        });
        match result {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
