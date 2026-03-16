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
               platform_id = excluded.platform_id,
               external_user_id = excluded.external_user_id,
               display_name = excluded.display_name,
               avatar_mxc = excluded.avatar_mxc,
               updated_at = datetime('now')",
            rusqlite::params![matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM puppets WHERE matrix_user_id = ?",
            rusqlite::params![matrix_user_id],
            |row| row.get(0),
        )?;
        Ok(id)
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

    /// List all puppet users.
    pub async fn list_all_puppets(&self) -> anyhow::Result<Vec<Puppet>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc FROM puppets",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Puppet {
                id: row.get(0)?,
                matrix_user_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_user_id: row.get(3)?,
                display_name: row.get(4)?,
                avatar_mxc: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List puppet users for a given platform.
    pub async fn list_puppets(&self, platform_id: &str) -> anyhow::Result<Vec<Puppet>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc FROM puppets WHERE platform_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![platform_id], |row| {
            Ok(Puppet {
                id: row.get(0)?,
                matrix_user_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_user_id: row.get(3)?,
                display_name: row.get(4)?,
                avatar_mxc: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List puppet users with cursor-based pagination.
    pub async fn list_puppets_paginated(
        &self,
        platform_id: Option<&str>,
        after_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<Puppet>> {
        let limit = limit.clamp(1, 1000);
        let conn = self.lock().await;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(p) =
            platform_id
        {
            (
                "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc \
                 FROM puppets WHERE id > ?1 AND platform_id = ?2 ORDER BY id LIMIT ?3".to_string(),
                vec![Box::new(after_id), Box::new(p.to_string()), Box::new(limit)],
            )
        } else {
            (
                "SELECT id, matrix_user_id, platform_id, external_user_id, display_name, avatar_mxc \
                 FROM puppets WHERE id > ?1 ORDER BY id LIMIT ?2".to_string(),
                vec![Box::new(after_id), Box::new(limit)],
            )
        };

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(Puppet {
                id: row.get(0)?,
                matrix_user_id: row.get(1)?,
                platform_id: row.get(2)?,
                external_user_id: row.get(3)?,
                display_name: row.get(4)?,
                avatar_mxc: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
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

#[cfg(test)]
mod tests {
    use crate::db::Database;

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn upsert_and_find_by_matrix_id() {
        let db = setup_db().await;
        db.upsert_puppet(
            "@puppet_user:m.org",
            "telegram",
            "tg_user_1",
            Some("Alice"),
            Some("mxc://m.org/avatar1"),
        )
        .await
        .unwrap();

        let puppet = db
            .find_puppet_by_matrix_id("@puppet_user:m.org")
            .await
            .unwrap()
            .expect("puppet should exist");
        assert_eq!(puppet.matrix_user_id, "@puppet_user:m.org");
        assert_eq!(puppet.platform_id, "telegram");
        assert_eq!(puppet.external_user_id, "tg_user_1");
        assert_eq!(puppet.display_name.as_deref(), Some("Alice"));
        assert_eq!(puppet.avatar_mxc.as_deref(), Some("mxc://m.org/avatar1"));
    }

    #[tokio::test]
    async fn upsert_updates_existing_puppet() {
        let db = setup_db().await;
        db.upsert_puppet("@puppet:m.org", "telegram", "tg_1", Some("Old Name"), None)
            .await
            .unwrap();

        // Update display_name and avatar
        db.upsert_puppet(
            "@puppet:m.org",
            "telegram",
            "tg_1",
            Some("New Name"),
            Some("mxc://m.org/new_avatar"),
        )
        .await
        .unwrap();

        let puppet = db
            .find_puppet_by_matrix_id("@puppet:m.org")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(puppet.display_name.as_deref(), Some("New Name"));
        assert_eq!(puppet.avatar_mxc.as_deref(), Some("mxc://m.org/new_avatar"));

        // Should still be just one puppet
        let all = db.list_all_puppets().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn find_by_matrix_id_returns_none_for_missing() {
        let db = setup_db().await;
        let result = db
            .find_puppet_by_matrix_id("@nonexistent:m.org")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_by_external_id() {
        let db = setup_db().await;
        db.upsert_puppet("@puppet:m.org", "telegram", "tg_1", Some("Bob"), None)
            .await
            .unwrap();

        let puppet = db
            .find_puppet_by_external_id("telegram", "tg_1")
            .await
            .unwrap()
            .expect("puppet should exist");
        assert_eq!(puppet.matrix_user_id, "@puppet:m.org");

        let none = db
            .find_puppet_by_external_id("telegram", "nonexistent")
            .await
            .unwrap();
        assert!(none.is_none());

        let none2 = db
            .find_puppet_by_external_id("discord", "tg_1")
            .await
            .unwrap();
        assert!(none2.is_none());
    }

    #[tokio::test]
    async fn list_all_puppets() {
        let db = setup_db().await;
        db.upsert_puppet("@p1:m.org", "telegram", "tg_1", None, None)
            .await
            .unwrap();
        db.upsert_puppet("@p2:m.org", "discord", "dc_1", None, None)
            .await
            .unwrap();

        let all = db.list_all_puppets().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn list_puppets_by_platform() {
        let db = setup_db().await;
        db.upsert_puppet("@p1:m.org", "telegram", "tg_1", None, None)
            .await
            .unwrap();
        db.upsert_puppet("@p2:m.org", "telegram", "tg_2", None, None)
            .await
            .unwrap();
        db.upsert_puppet("@p3:m.org", "discord", "dc_1", None, None)
            .await
            .unwrap();

        let telegram = db.list_puppets("telegram").await.unwrap();
        assert_eq!(telegram.len(), 2);

        let discord = db.list_puppets("discord").await.unwrap();
        assert_eq!(discord.len(), 1);

        let slack = db.list_puppets("slack").await.unwrap();
        assert!(slack.is_empty());
    }

    #[tokio::test]
    async fn puppet_with_null_optional_fields() {
        let db = setup_db().await;
        db.upsert_puppet("@p1:m.org", "telegram", "tg_1", None, None)
            .await
            .unwrap();

        let puppet = db
            .find_puppet_by_matrix_id("@p1:m.org")
            .await
            .unwrap()
            .unwrap();
        assert!(puppet.display_name.is_none());
        assert!(puppet.avatar_mxc.is_none());
    }

    #[tokio::test]
    async fn count_puppets() {
        let db = setup_db().await;
        assert_eq!(db.count_puppets().await.unwrap(), 0);

        db.upsert_puppet("@p1:m.org", "telegram", "tg_1", None, None)
            .await
            .unwrap();
        assert_eq!(db.count_puppets().await.unwrap(), 1);
    }

    /// Case-variant external IDs that map to the same matrix_user_id must
    /// collapse into a single row, with the identity columns updated.
    #[tokio::test]
    async fn upsert_rewrites_identity_on_matrix_user_id_conflict() {
        let db = setup_db().await;

        // First insert with original-case external ID.
        db.upsert_puppet(
            "@bot_tg_alice:m.org",
            "telegram",
            "Alice",
            Some("Alice"),
            None,
        )
        .await
        .unwrap();

        // Second insert with a case-variant external ID but same matrix_user_id.
        db.upsert_puppet(
            "@bot_tg_alice:m.org",
            "telegram",
            "alice",
            Some("alice v2"),
            None,
        )
        .await
        .unwrap();

        // Only one row should exist.
        let all = db.list_all_puppets().await.unwrap();
        assert_eq!(all.len(), 1);

        let puppet = db
            .find_puppet_by_matrix_id("@bot_tg_alice:m.org")
            .await
            .unwrap()
            .unwrap();
        // external_user_id must have been rewritten to the latest value.
        assert_eq!(puppet.external_user_id, "alice");
        assert_eq!(puppet.display_name.as_deref(), Some("alice v2"));
    }

    /// After an identity rewrite, find_puppet_by_external_id with the new ID
    /// must return the row, while the old ID must not.
    #[tokio::test]
    async fn find_by_external_id_after_identity_rewrite() {
        let db = setup_db().await;

        db.upsert_puppet("@bot_tg_alice:m.org", "telegram", "Alice", None, None)
            .await
            .unwrap();
        db.upsert_puppet("@bot_tg_alice:m.org", "telegram", "alice", None, None)
            .await
            .unwrap();

        // New external_user_id is findable.
        let found = db
            .find_puppet_by_external_id("telegram", "alice")
            .await
            .unwrap();
        assert!(found.is_some());

        // Old external_user_id is no longer findable (rewritten).
        let old = db
            .find_puppet_by_external_id("telegram", "Alice")
            .await
            .unwrap();
        assert!(old.is_none());
    }

    /// Clearing display_name and avatar_mxc (setting to None) must persist.
    #[tokio::test]
    async fn upsert_clears_profile_fields() {
        let db = setup_db().await;

        db.upsert_puppet(
            "@p:m.org",
            "telegram",
            "tg_1",
            Some("Bob"),
            Some("mxc://m.org/avatar"),
        )
        .await
        .unwrap();

        // Clear both fields.
        db.upsert_puppet("@p:m.org", "telegram", "tg_1", None, None)
            .await
            .unwrap();

        let puppet = db
            .find_puppet_by_matrix_id("@p:m.org")
            .await
            .unwrap()
            .unwrap();
        assert!(
            puppet.display_name.is_none(),
            "display_name should be cleared"
        );
        assert!(puppet.avatar_mxc.is_none(), "avatar_mxc should be cleared");
    }
}
