use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: i64,
    pub platform_id: String,
    pub webhook_url: String,
    pub events: String,
    pub enabled: bool,
    /// Comma-separated allowlist of *non-matrix* source platform IDs whose
    /// messages are forwarded. Matrix user messages are always forwarded.
    /// - Empty (`""`) = only forward Matrix user messages (default).
    /// - `"*"` = forward all sources (Matrix + other platforms).
    /// - `"telegram,discord"` = forward Matrix + those platforms only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forward_sources: String,
    /// Comma-separated list of capabilities this integration supports.
    /// e.g. `"message,image,reaction,edit,redaction,command"`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capabilities: String,
    /// Matrix user ID of the integration operator.
    /// Auto-invited into portal rooms created for this platform.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
}

/// Check if messages from the given source platform should be forwarded.
///
/// The `"matrix"` source is always forwarded — it represents the bridge's
/// core functionality (real Matrix users talking to external platforms).
/// The allowlist controls forwarding of *other* platform sources:
/// - Empty iterator = deny all non-matrix sources.
/// - `"*"` entry = allow all sources.
/// - Specific entries = allow only those platforms.
pub fn should_forward_source<'a>(
    sources: impl IntoIterator<Item = &'a str>,
    source_platform: &str,
) -> bool {
    if source_platform == "matrix" {
        return true;
    }
    for s in sources {
        if s == "*" || s == source_platform {
            return true;
        }
    }
    false
}

impl Webhook {
    /// Check if messages from the given source platform should be forwarded.
    pub fn should_forward_source(&self, source_platform: &str) -> bool {
        should_forward_source(
            self.forward_sources.split(',').map(|p| p.trim()),
            source_platform,
        )
    }

    /// Check if the webhook is subscribed to a given event type.
    ///
    /// The `events` field is a comma-separated list (e.g. `"message,redaction"`).
    /// A wildcard `"*"` matches all event types.
    pub fn should_deliver_event(&self, event_type: &str) -> bool {
        self.events.split(',').any(|e| {
            let e = e.trim();
            e == "*" || e == event_type
        })
    }
}

impl Database {
    /// Register a webhook for a platform.
    pub async fn create_webhook(
        &self,
        platform_id: &str,
        webhook_url: &str,
        events: &str,
        forward_sources: &str,
        capabilities: &str,
        owner: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO webhooks (platform_id, webhook_url, events, forward_sources, capabilities, owner)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(platform_id, webhook_url) DO UPDATE SET
               events = excluded.events,
               forward_sources = excluded.forward_sources,
               capabilities = excluded.capabilities,
               owner = excluded.owner,
               enabled = 1",
            rusqlite::params![platform_id, webhook_url, events, forward_sources, capabilities, owner],
        )?;
        // Use SELECT to get the correct row ID — last_insert_rowid() is
        // unreliable in the ON CONFLICT UPDATE branch.
        let id: i64 = conn.query_row(
            "SELECT id FROM webhooks WHERE platform_id = ?1 AND webhook_url = ?2",
            rusqlite::params![platform_id, webhook_url],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// List all enabled webhooks for a platform.
    pub async fn list_webhooks(&self, platform_id: &str) -> anyhow::Result<Vec<Webhook>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, platform_id, webhook_url, events, enabled, forward_sources, capabilities, owner FROM webhooks WHERE platform_id = ?1 AND enabled = 1",
        )?;
        let rows = stmt.query_map(rusqlite::params![platform_id], |row| {
            Ok(Webhook {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                webhook_url: row.get(2)?,
                events: row.get(3)?,
                enabled: row.get(4)?,
                forward_sources: row.get(5)?,
                capabilities: row.get(6)?,
                owner: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List all webhooks (all platforms).
    pub async fn list_all_webhooks(&self) -> anyhow::Result<Vec<Webhook>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, platform_id, webhook_url, events, enabled, forward_sources, capabilities, owner FROM webhooks",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Webhook {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                webhook_url: row.get(2)?,
                events: row.get(3)?,
                enabled: row.get(4)?,
                forward_sources: row.get(5)?,
                capabilities: row.get(6)?,
                owner: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// List webhooks with cursor-based pagination.
    pub async fn list_webhooks_paginated(
        &self,
        platform_id: Option<&str>,
        after_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<Webhook>> {
        let limit = limit.clamp(1, 1000);
        let conn = self.lock().await;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(p) =
            platform_id
        {
            (
                "SELECT id, platform_id, webhook_url, events, enabled, forward_sources, capabilities, owner \
                 FROM webhooks WHERE id > ?1 AND platform_id = ?2 ORDER BY id LIMIT ?3".to_string(),
                vec![Box::new(after_id), Box::new(p.to_string()), Box::new(limit)],
            )
        } else {
            (
                "SELECT id, platform_id, webhook_url, events, enabled, forward_sources, capabilities, owner \
                 FROM webhooks WHERE id > ?1 ORDER BY id LIMIT ?2".to_string(),
                vec![Box::new(after_id), Box::new(limit)],
            )
        };

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(Webhook {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                webhook_url: row.get(2)?,
                events: row.get(3)?,
                enabled: row.get(4)?,
                forward_sources: row.get(5)?,
                capabilities: row.get(6)?,
                owner: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// Delete a webhook by ID.
    pub async fn delete_webhook(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock().await;
        let changed = conn.execute("DELETE FROM webhooks WHERE id = ?1", rusqlite::params![id])?;
        Ok(changed > 0)
    }

    /// Get the aggregated capabilities for a platform across all enabled webhooks.
    ///
    /// Returns a deduplicated, sorted list of capability strings.
    pub async fn get_platform_capabilities(
        &self,
        platform_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let webhooks = self.list_webhooks(platform_id).await?;
        let mut caps: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for wh in &webhooks {
            for cap in wh.capabilities.split(',') {
                let cap = cap.trim();
                if !cap.is_empty() {
                    caps.insert(cap.to_string());
                }
            }
        }
        Ok(caps.into_iter().collect())
    }

    /// Get deduplicated owner user IDs for a platform (non-empty owners only).
    pub async fn get_platform_owners(&self, platform_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT owner FROM webhooks WHERE platform_id = ?1 AND enabled = 1 AND owner != ''",
        )?;
        let rows = stmt.query_map(rusqlite::params![platform_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| anyhow::anyhow!(e))
    }

    /// Disable a webhook.
    pub async fn disable_webhook(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock().await;
        let changed = conn.execute(
            "UPDATE webhooks SET enabled = 0 WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::Webhook;
    use crate::db::Database;

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn create_and_list_webhook() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();
        assert!(id > 0);

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].webhook_url, "https://example.com/hook");
        assert_eq!(hooks[0].events, "message");
        assert!(hooks[0].enabled);
    }

    #[tokio::test]
    async fn list_webhooks_only_returns_enabled() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();
        db.disable_webhook(id).await.unwrap();

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert!(hooks.is_empty());
    }

    #[tokio::test]
    async fn list_all_webhooks_includes_disabled() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();
        db.disable_webhook(id).await.unwrap();

        let all = db.list_all_webhooks().await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].enabled);
    }

    #[tokio::test]
    async fn list_webhooks_filters_by_platform() {
        let db = setup_db().await;
        db.create_webhook("telegram", "https://example.com/tg", "message", "", "", "")
            .await
            .unwrap();
        db.create_webhook("discord", "https://example.com/dc", "message", "", "", "")
            .await
            .unwrap();

        let tg = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(tg.len(), 1);
        assert_eq!(tg[0].platform_id, "telegram");
    }

    #[tokio::test]
    async fn delete_webhook() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();

        let deleted = db.delete_webhook(id).await.unwrap();
        assert!(deleted);

        let hooks = db.list_all_webhooks().await.unwrap();
        assert!(hooks.is_empty());

        // Delete non-existent returns false
        let deleted_again = db.delete_webhook(id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn disable_webhook() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();

        let disabled = db.disable_webhook(id).await.unwrap();
        assert!(disabled);

        // Still in the DB but not in enabled list
        let enabled = db.list_webhooks("telegram").await.unwrap();
        assert!(enabled.is_empty());

        let all = db.list_all_webhooks().await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].enabled);
    }

    #[tokio::test]
    async fn disable_nonexistent_webhook() {
        let db = setup_db().await;
        let result = db.disable_webhook(9999).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn upsert_webhook_on_conflict() {
        let db = setup_db().await;
        db.create_webhook(
            "telegram",
            "https://example.com/hook",
            "message",
            "",
            "",
            "",
        )
        .await
        .unwrap();

        // Same platform + url, different events => update
        db.create_webhook(
            "telegram",
            "https://example.com/hook",
            "message,reaction",
            "*",
            "",
            "",
        )
        .await
        .unwrap();

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].events, "message,reaction");
        assert_eq!(hooks[0].forward_sources, "*");
    }

    #[tokio::test]
    async fn upsert_re_enables_disabled_webhook() {
        let db = setup_db().await;
        let id = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();
        db.disable_webhook(id).await.unwrap();

        // Re-create same webhook => should re-enable
        db.create_webhook(
            "telegram",
            "https://example.com/hook",
            "message",
            "",
            "",
            "",
        )
        .await
        .unwrap();

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].enabled);
    }

    #[tokio::test]
    async fn webhook_with_forward_sources() {
        let db = setup_db().await;
        db.create_webhook(
            "telegram",
            "https://example.com/hook",
            "message",
            "discord,slack",
            "message,image,reaction",
            "@admin:example.com",
        )
        .await
        .unwrap();

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks[0].forward_sources, "discord,slack");
        assert_eq!(hooks[0].capabilities, "message,image,reaction");
    }

    #[tokio::test]
    async fn count_webhooks() {
        let db = setup_db().await;
        assert_eq!(db.count_webhooks().await.unwrap(), 0);

        db.create_webhook("telegram", "https://example.com/h1", "message", "", "", "")
            .await
            .unwrap();
        assert_eq!(db.count_webhooks().await.unwrap(), 1);
    }

    // Tests for the Webhook::should_forward_source method
    #[test]
    fn should_forward_source_empty_denies_non_matrix() {
        let wh = Webhook {
            id: 1,
            platform_id: "telegram".to_string(),
            webhook_url: "https://example.com".to_string(),
            events: "message".to_string(),
            enabled: true,
            forward_sources: "".to_string(),
            capabilities: "".to_string(),
            owner: "".to_string(),
        };
        // "matrix" is always forwarded (bridge core functionality).
        assert!(wh.should_forward_source("matrix"));
        assert!(!wh.should_forward_source("discord"));
        assert!(!wh.should_forward_source("telegram"));
    }

    #[test]
    fn should_forward_source_wildcard_allows_all() {
        let wh = Webhook {
            id: 1,
            platform_id: "telegram".to_string(),
            webhook_url: "https://example.com".to_string(),
            events: "message".to_string(),
            enabled: true,
            forward_sources: "*".to_string(),
            capabilities: "".to_string(),
            owner: "".to_string(),
        };
        assert!(wh.should_forward_source("discord"));
        assert!(wh.should_forward_source("anything"));
    }

    #[test]
    fn should_forward_source_specific_list() {
        let wh = Webhook {
            id: 1,
            platform_id: "webhook".to_string(),
            webhook_url: "https://example.com".to_string(),
            events: "message".to_string(),
            enabled: true,
            forward_sources: "telegram,discord".to_string(),
            capabilities: "".to_string(),
            owner: "".to_string(),
        };
        assert!(wh.should_forward_source("telegram"));
        assert!(wh.should_forward_source("discord"));
        assert!(!wh.should_forward_source("slack"));
    }

    #[tokio::test]
    async fn upsert_returns_correct_id_on_conflict() {
        let db = setup_db().await;
        let id1 = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();

        // Same platform+url => upsert update branch
        let id2 = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message,reaction",
                "",
                "message,image",
                "",
            )
            .await
            .unwrap();

        // Must return the same row ID, not last_insert_rowid() of a different row
        assert_eq!(id1, id2, "upsert must return the same ID on conflict");

        // The returned id must refer to the updated row
        let deleted = db.delete_webhook(id2).await.unwrap();
        assert!(deleted, "returned id must be usable for delete");
        assert!(db.list_webhooks("telegram").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn upsert_returns_correct_id_after_re_enable() {
        let db = setup_db().await;
        let id1 = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message",
                "",
                "",
                "",
            )
            .await
            .unwrap();
        db.disable_webhook(id1).await.unwrap();

        // Re-create same webhook => re-enable, must return original id
        let id2 = db
            .create_webhook(
                "telegram",
                "https://example.com/hook",
                "message,reaction",
                "",
                "",
                "",
            )
            .await
            .unwrap();

        assert_eq!(id1, id2, "re-enable upsert must return the original row ID");

        // Verify the webhook is enabled with updated events
        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].id, id2);
        assert_eq!(hooks[0].events, "message,reaction");
        assert!(hooks[0].enabled);
    }

    #[tokio::test]
    async fn upsert_id_stable_across_multiple_platforms() {
        let db = setup_db().await;
        // Create webhooks for two platforms so row IDs diverge from insert order
        let _other = db
            .create_webhook("discord", "https://example.com/dc", "message", "", "", "")
            .await
            .unwrap();
        let tg_id = db
            .create_webhook("telegram", "https://example.com/tg", "message", "", "", "")
            .await
            .unwrap();

        // Upsert the telegram webhook
        let tg_id2 = db
            .create_webhook("telegram", "https://example.com/tg", "reaction", "", "", "")
            .await
            .unwrap();

        assert_eq!(tg_id, tg_id2);

        // Deleting by the returned id must remove exactly the telegram webhook
        db.delete_webhook(tg_id2).await.unwrap();
        let all = db.list_all_webhooks().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].platform_id, "discord");
    }
}
