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

    /// List message mappings with cursor-based pagination.
    ///
    /// - `platform_id`: optional platform filter.
    /// - `room_mapping_id`: optional room mapping filter.
    /// - `after_id`: return rows with `id > after_id` (cursor).
    /// - `limit`: max rows to return (capped at 1000).
    pub async fn list_message_mappings(
        &self,
        platform_id: Option<&str>,
        room_mapping_id: Option<i64>,
        after_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<MessageMapping>> {
        let limit = limit.min(1000);
        let conn = self.lock().await;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (platform_id, room_mapping_id) {
                (Some(p), Some(r)) => (
                    "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id \
                     FROM message_mappings WHERE id > ?1 AND platform_id = ?2 AND room_mapping_id = ?3 \
                     ORDER BY id LIMIT ?4"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(p.to_string()), Box::new(r), Box::new(limit)],
                ),
                (Some(p), None) => (
                    "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id \
                     FROM message_mappings WHERE id > ?1 AND platform_id = ?2 \
                     ORDER BY id LIMIT ?3"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(p.to_string()), Box::new(limit)],
                ),
                (None, Some(r)) => (
                    "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id \
                     FROM message_mappings WHERE id > ?1 AND room_mapping_id = ?2 \
                     ORDER BY id LIMIT ?3"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(r), Box::new(limit)],
                ),
                (None, None) => (
                    "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id \
                     FROM message_mappings WHERE id > ?1 \
                     ORDER BY id LIMIT ?2"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(limit)],
                ),
            };

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(MessageMapping {
                id: row.get(0)?,
                matrix_event_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_message_id: row.get(3)?,
                room_mapping_id: row.get(4)?,
            })
        })?;
        let mut mappings = Vec::new();
        for row in rows {
            mappings.push(row?);
        }
        Ok(mappings)
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
