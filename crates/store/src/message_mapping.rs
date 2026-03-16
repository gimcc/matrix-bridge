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

    /// Find all message mappings for a Matrix event ID (across all platforms).
    pub async fn find_all_messages_by_matrix_id(
        &self,
        matrix_event_id: &str,
    ) -> anyhow::Result<Vec<MessageMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_event_id, platform_id, external_message_id, room_mapping_id \
             FROM message_mappings WHERE matrix_event_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![matrix_event_id], |row| {
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
        let limit = limit.clamp(1, 1000);
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
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

#[cfg(test)]
mod tests {
    use crate::db::Database;

    async fn setup_db() -> (Database, i64) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        // Create a room mapping to satisfy the foreign key constraint
        let room_id = db
            .create_room_mapping("!room:m.org", "telegram", "ext_room_1")
            .await
            .unwrap();
        (db, room_id)
    }

    #[tokio::test]
    async fn create_and_find_by_matrix_id() {
        let (db, room_id) = setup_db().await;
        let id = db
            .create_message_mapping("$event1:m.org", "telegram", "msg_ext_1", room_id)
            .await
            .unwrap();
        assert!(id > 0);

        let mapping = db
            .find_message_by_matrix_id("$event1:m.org")
            .await
            .unwrap()
            .expect("mapping should exist");
        assert_eq!(mapping.matrix_event_id, "$event1:m.org");
        assert_eq!(mapping.platform_id, "telegram");
        assert_eq!(mapping.external_message_id, "msg_ext_1");
        assert_eq!(mapping.room_mapping_id, room_id);
    }

    #[tokio::test]
    async fn find_by_matrix_id_returns_none_for_missing() {
        let (db, _) = setup_db().await;
        let result = db
            .find_message_by_matrix_id("$nonexistent:m.org")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_by_external_id() {
        let (db, room_id) = setup_db().await;
        db.create_message_mapping("$event1:m.org", "telegram", "msg_ext_1", room_id)
            .await
            .unwrap();

        let mapping = db
            .find_message_by_external_id("telegram", "msg_ext_1")
            .await
            .unwrap()
            .expect("mapping should exist");
        assert_eq!(mapping.matrix_event_id, "$event1:m.org");

        let none = db
            .find_message_by_external_id("telegram", "nonexistent")
            .await
            .unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn list_message_mappings_no_filter() {
        let (db, room_id) = setup_db().await;
        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();
        db.create_message_mapping("$e2:m.org", "telegram", "msg2", room_id)
            .await
            .unwrap();

        let all = db.list_message_mappings(None, None, 0, 100).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn list_message_mappings_with_platform_filter() {
        let (db, room_id) = setup_db().await;
        let room_id2 = db
            .create_room_mapping("!room2:m.org", "discord", "ext_room_2")
            .await
            .unwrap();
        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();
        db.create_message_mapping("$e2:m.org", "discord", "msg2", room_id2)
            .await
            .unwrap();

        let telegram = db
            .list_message_mappings(Some("telegram"), None, 0, 100)
            .await
            .unwrap();
        assert_eq!(telegram.len(), 1);
        assert_eq!(telegram[0].platform_id, "telegram");
    }

    #[tokio::test]
    async fn list_message_mappings_with_room_filter() {
        let (db, room_id) = setup_db().await;
        let room_id2 = db
            .create_room_mapping("!room2:m.org", "telegram", "ext_room_2")
            .await
            .unwrap();
        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();
        db.create_message_mapping("$e2:m.org", "telegram", "msg2", room_id2)
            .await
            .unwrap();

        let filtered = db
            .list_message_mappings(None, Some(room_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].room_mapping_id, room_id);
    }

    #[tokio::test]
    async fn list_message_mappings_with_both_filters() {
        let (db, room_id) = setup_db().await;
        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();

        let filtered = db
            .list_message_mappings(Some("telegram"), Some(room_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);

        let empty = db
            .list_message_mappings(Some("discord"), Some(room_id), 0, 100)
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn list_message_mappings_cursor_pagination() {
        let (db, room_id) = setup_db().await;
        for i in 0..5 {
            db.create_message_mapping(
                &format!("$e{}:m.org", i),
                "telegram",
                &format!("msg{}", i),
                room_id,
            )
            .await
            .unwrap();
        }

        let page1 = db.list_message_mappings(None, None, 0, 2).await.unwrap();
        assert_eq!(page1.len(), 2);

        let last_id = page1.last().unwrap().id;
        let page2 = db
            .list_message_mappings(None, None, last_id, 2)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
        assert!(page2[0].id > last_id);

        let last_id2 = page2.last().unwrap().id;
        let page3 = db
            .list_message_mappings(None, None, last_id2, 2)
            .await
            .unwrap();
        assert_eq!(page3.len(), 1);
    }

    #[tokio::test]
    async fn list_message_mappings_limit_capped_at_1000() {
        let (db, room_id) = setup_db().await;
        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();

        // Even with a huge limit, should not panic
        let result = db
            .list_message_mappings(None, None, 0, 999999)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn count_message_mappings() {
        let (db, room_id) = setup_db().await;
        assert_eq!(db.count_message_mappings().await.unwrap(), 0);

        db.create_message_mapping("$e1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();
        assert_eq!(db.count_message_mappings().await.unwrap(), 1);
    }
}
