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
    /// Create or update a room mapping (upsert).
    ///
    /// If a mapping already exists for the same `(platform_id, external_room_id)`,
    /// the `matrix_room_id` is updated. If a mapping exists for
    /// `(matrix_room_id, platform_id)`, the `external_room_id` is updated.
    /// Message mappings referencing the old row are migrated to the new one.
    ///
    /// Returns the row ID of the upserted mapping.
    pub async fn create_room_mapping(
        &self,
        matrix_room_id: &str,
        platform_id: &str,
        external_room_id: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        let tx = conn.unchecked_transaction()?;

        // Check for an existing mapping on either unique constraint.
        let existing_by_matrix: Option<i64> = tx
            .prepare("SELECT id FROM room_mappings WHERE matrix_room_id = ?1 AND platform_id = ?2")?
            .query_row(rusqlite::params![matrix_room_id, platform_id], |row| {
                row.get(0)
            })
            .ok();

        let existing_by_external: Option<i64> = tx
            .prepare(
                "SELECT id FROM room_mappings WHERE platform_id = ?1 AND external_room_id = ?2",
            )?
            .query_row(rusqlite::params![platform_id, external_room_id], |row| {
                row.get(0)
            })
            .ok();

        let id = match (existing_by_matrix, existing_by_external) {
            // Both constraints match the same row — no-op.
            (Some(id_m), Some(id_e)) if id_m == id_e => id_m,

            // Both match but different rows — merge: migrate message_mappings,
            // delete the conflicting row, then update the surviving one.
            (Some(id_m), Some(id_e)) => {
                tx.execute(
                    "UPDATE message_mappings SET room_mapping_id = ?1 WHERE room_mapping_id = ?2",
                    rusqlite::params![id_m, id_e],
                )?;
                tx.execute(
                    "DELETE FROM room_mappings WHERE id = ?1",
                    rusqlite::params![id_e],
                )?;
                tx.execute(
                    "UPDATE room_mappings SET external_room_id = ?1 WHERE id = ?2",
                    rusqlite::params![external_room_id, id_m],
                )?;
                id_m
            }

            // Only matrix_room_id+platform match — update external_room_id.
            (Some(id), None) => {
                tx.execute(
                    "UPDATE room_mappings SET external_room_id = ?1 WHERE id = ?2",
                    rusqlite::params![external_room_id, id],
                )?;
                id
            }

            // Only platform+external match — update matrix_room_id.
            (None, Some(id)) => {
                tx.execute(
                    "UPDATE room_mappings SET matrix_room_id = ?1 WHERE id = ?2",
                    rusqlite::params![matrix_room_id, id],
                )?;
                id
            }

            // No existing mapping — insert new.
            (None, None) => {
                tx.execute(
                    "INSERT INTO room_mappings (matrix_room_id, platform_id, external_room_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![matrix_room_id, platform_id, external_room_id],
                )?;
                tx.last_insert_rowid()
            }
        };

        tx.commit()?;
        Ok(id)
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

    /// List all room mappings across all platforms.
    pub async fn list_all_room_mappings(&self) -> anyhow::Result<Vec<RoomMapping>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_room_id, platform_id, external_room_id FROM room_mappings",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
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
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List room mappings with cursor-based pagination.
    pub async fn list_room_mappings_paginated(
        &self,
        platform_id: Option<&str>,
        after_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<RoomMapping>> {
        let limit = limit.clamp(1, 1000);
        let conn = self.lock().await;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if let Some(p) = platform_id {
                (
                    "SELECT id, matrix_room_id, platform_id, external_room_id \
                 FROM room_mappings WHERE id > ?1 AND platform_id = ?2 ORDER BY id LIMIT ?3"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(p.to_string()), Box::new(limit)],
                )
            } else {
                (
                    "SELECT id, matrix_room_id, platform_id, external_room_id \
                 FROM room_mappings WHERE id > ?1 ORDER BY id LIMIT ?2"
                        .to_string(),
                    vec![Box::new(after_id), Box::new(limit)],
                )
            };

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(RoomMapping {
                id: row.get(0)?,
                matrix_room_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_room_id: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
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
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
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

#[cfg(test)]
mod tests {
    use crate::db::Database;

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn create_and_retrieve_room_mapping() {
        let db = setup_db().await;
        let id = db
            .create_room_mapping("!room:matrix.org", "telegram", "ext_room_1")
            .await
            .unwrap();
        assert!(id > 0);

        let mapping = db
            .find_room_by_matrix_id("!room:matrix.org", "telegram")
            .await
            .unwrap()
            .expect("mapping should exist");
        assert_eq!(mapping.matrix_room_id, "!room:matrix.org");
        assert_eq!(mapping.platform_id, "telegram");
        assert_eq!(mapping.external_room_id, "ext_room_1");
    }

    #[tokio::test]
    async fn find_room_by_external_id() {
        let db = setup_db().await;
        db.create_room_mapping("!room:matrix.org", "telegram", "ext_room_1")
            .await
            .unwrap();

        let mapping = db
            .find_room_by_external_id("telegram", "ext_room_1")
            .await
            .unwrap()
            .expect("mapping should exist");
        assert_eq!(mapping.matrix_room_id, "!room:matrix.org");

        let none = db
            .find_room_by_external_id("telegram", "nonexistent")
            .await
            .unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn find_room_by_matrix_id_returns_none_for_missing() {
        let db = setup_db().await;
        let result = db
            .find_room_by_matrix_id("!missing:matrix.org", "telegram")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_room_mappings_by_platform() {
        let db = setup_db().await;
        db.create_room_mapping("!r1:m.org", "telegram", "ext1")
            .await
            .unwrap();
        db.create_room_mapping("!r2:m.org", "telegram", "ext2")
            .await
            .unwrap();
        db.create_room_mapping("!r3:m.org", "discord", "ext3")
            .await
            .unwrap();

        let telegram = db.list_room_mappings("telegram").await.unwrap();
        assert_eq!(telegram.len(), 2);

        let discord = db.list_room_mappings("discord").await.unwrap();
        assert_eq!(discord.len(), 1);
    }

    #[tokio::test]
    async fn list_all_room_mappings() {
        let db = setup_db().await;
        db.create_room_mapping("!r1:m.org", "telegram", "ext1")
            .await
            .unwrap();
        db.create_room_mapping("!r2:m.org", "discord", "ext2")
            .await
            .unwrap();

        let all = db.list_all_room_mappings().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn find_all_mappings_by_matrix_id() {
        let db = setup_db().await;
        db.create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();
        db.create_room_mapping("!room:m.org", "discord", "ext2")
            .await
            .unwrap();

        let mappings = db
            .find_all_mappings_by_matrix_id("!room:m.org")
            .await
            .unwrap();
        assert_eq!(mappings.len(), 2);
    }

    #[tokio::test]
    async fn delete_room_mapping() {
        let db = setup_db().await;
        let id = db
            .create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();

        let deleted = db.delete_room_mapping(id).await.unwrap();
        assert!(deleted);

        let result = db
            .find_room_by_matrix_id("!room:m.org", "telegram")
            .await
            .unwrap();
        assert!(result.is_none());

        // Delete non-existent returns false
        let deleted_again = db.delete_room_mapping(id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn upsert_updates_external_room_id_when_matrix_id_matches() {
        let db = setup_db().await;
        let id1 = db
            .create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();
        // Same matrix_room_id + platform, different external_room_id => update
        let id2 = db
            .create_room_mapping("!room:m.org", "telegram", "ext2")
            .await
            .unwrap();
        assert_eq!(id1, id2);

        let mapping = db
            .find_room_by_matrix_id("!room:m.org", "telegram")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mapping.external_room_id, "ext2");
    }

    #[tokio::test]
    async fn upsert_updates_matrix_room_id_when_external_id_matches() {
        let db = setup_db().await;
        let id1 = db
            .create_room_mapping("!room1:m.org", "telegram", "ext1")
            .await
            .unwrap();
        // Same platform + external_room_id, different matrix_room_id => update
        let id2 = db
            .create_room_mapping("!room2:m.org", "telegram", "ext1")
            .await
            .unwrap();
        assert_eq!(id1, id2);

        let mapping = db
            .find_room_by_external_id("telegram", "ext1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mapping.matrix_room_id, "!room2:m.org");
    }

    #[tokio::test]
    async fn upsert_noop_when_both_match_same_row() {
        let db = setup_db().await;
        let id1 = db
            .create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();
        // Exact same data => no-op, same ID
        let id2 = db
            .create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn count_room_mappings() {
        let db = setup_db().await;
        assert_eq!(db.count_room_mappings().await.unwrap(), 0);

        db.create_room_mapping("!r1:m.org", "telegram", "ext1")
            .await
            .unwrap();
        assert_eq!(db.count_room_mappings().await.unwrap(), 1);

        db.create_room_mapping("!r2:m.org", "discord", "ext2")
            .await
            .unwrap();
        assert_eq!(db.count_room_mappings().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn list_active_platforms() {
        let db = setup_db().await;
        let platforms = db.list_active_platforms().await.unwrap();
        assert!(platforms.is_empty());

        db.create_room_mapping("!r1:m.org", "telegram", "ext1")
            .await
            .unwrap();
        db.create_room_mapping("!r2:m.org", "discord", "ext2")
            .await
            .unwrap();
        db.create_room_mapping("!r3:m.org", "telegram", "ext3")
            .await
            .unwrap();

        let platforms = db.list_active_platforms().await.unwrap();
        assert_eq!(platforms, vec!["discord", "telegram"]);
    }

    #[tokio::test]
    async fn delete_room_mapping_cascades_message_mappings() {
        let db = setup_db().await;
        let room_id = db
            .create_room_mapping("!room:m.org", "telegram", "ext1")
            .await
            .unwrap();

        // Create message mappings referencing this room
        db.create_message_mapping("$ev1:m.org", "telegram", "msg1", room_id)
            .await
            .unwrap();
        db.create_message_mapping("$ev2:m.org", "telegram", "msg2", room_id)
            .await
            .unwrap();
        assert_eq!(db.count_message_mappings().await.unwrap(), 2);

        // Deleting the room mapping should cascade-delete its message mappings
        let deleted = db.delete_room_mapping(room_id).await.unwrap();
        assert!(deleted);
        assert_eq!(db.count_message_mappings().await.unwrap(), 0);
    }
}
