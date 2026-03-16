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
        self.events
            .split(',')
            .any(|e| {
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
    ) -> anyhow::Result<i64> {
        let conn = self.lock().await;
        conn.execute(
            "INSERT INTO webhooks (platform_id, webhook_url, events, forward_sources) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(platform_id, webhook_url) DO UPDATE SET
               events = excluded.events,
               forward_sources = excluded.forward_sources,
               enabled = 1",
            rusqlite::params![platform_id, webhook_url, events, forward_sources],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// List all enabled webhooks for a platform.
    pub async fn list_webhooks(&self, platform_id: &str) -> anyhow::Result<Vec<Webhook>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, platform_id, webhook_url, events, enabled, forward_sources FROM webhooks WHERE platform_id = ?1 AND enabled = 1",
        )?;
        let rows = stmt.query_map(rusqlite::params![platform_id], |row| {
            Ok(Webhook {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                webhook_url: row.get(2)?,
                events: row.get(3)?,
                enabled: row.get(4)?,
                forward_sources: row.get(5)?,
            })
        })?;
        let mut webhooks = Vec::new();
        for row in rows {
            webhooks.push(row?);
        }
        Ok(webhooks)
    }

    /// List all webhooks (all platforms).
    pub async fn list_all_webhooks(&self) -> anyhow::Result<Vec<Webhook>> {
        let conn = self.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, platform_id, webhook_url, events, enabled, forward_sources FROM webhooks",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Webhook {
                id: row.get(0)?,
                platform_id: row.get(1)?,
                webhook_url: row.get(2)?,
                events: row.get(3)?,
                enabled: row.get(4)?,
                forward_sources: row.get(5)?,
            })
        })?;
        let mut webhooks = Vec::new();
        for row in rows {
            webhooks.push(row?);
        }
        Ok(webhooks)
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

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(p) = platform_id {
            (
                "SELECT id, platform_id, webhook_url, events, enabled, forward_sources \
                 FROM webhooks WHERE id > ?1 AND platform_id = ?2 ORDER BY id LIMIT ?3".to_string(),
                vec![Box::new(after_id), Box::new(p.to_string()), Box::new(limit)],
            )
        } else {
            (
                "SELECT id, platform_id, webhook_url, events, enabled, forward_sources \
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
            })
        })?;
        let mut webhooks = Vec::new();
        for row in rows {
            webhooks.push(row?);
        }
        Ok(webhooks)
    }

    /// Delete a webhook by ID.
    pub async fn delete_webhook(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock().await;
        let changed = conn.execute("DELETE FROM webhooks WHERE id = ?1", rusqlite::params![id])?;
        Ok(changed > 0)
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
    use crate::db::Database;
    use super::Webhook;

    async fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().await.unwrap();
        db
    }

    #[tokio::test]
    async fn create_and_list_webhook() {
        let db = setup_db().await;
        let id = db
            .create_webhook("telegram", "https://example.com/hook", "message", "")
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
            .create_webhook("telegram", "https://example.com/hook", "message", "")
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
            .create_webhook("telegram", "https://example.com/hook", "message", "")
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
        db.create_webhook("telegram", "https://example.com/tg", "message", "")
            .await
            .unwrap();
        db.create_webhook("discord", "https://example.com/dc", "message", "")
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
            .create_webhook("telegram", "https://example.com/hook", "message", "")
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
            .create_webhook("telegram", "https://example.com/hook", "message", "")
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
        db.create_webhook("telegram", "https://example.com/hook", "message", "")
            .await
            .unwrap();

        // Same platform + url, different events => update
        db.create_webhook(
            "telegram",
            "https://example.com/hook",
            "message,reaction",
            "*",
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
            .create_webhook("telegram", "https://example.com/hook", "message", "")
            .await
            .unwrap();
        db.disable_webhook(id).await.unwrap();

        // Re-create same webhook => should re-enable
        db.create_webhook("telegram", "https://example.com/hook", "message", "")
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
        )
        .await
        .unwrap();

        let hooks = db.list_webhooks("telegram").await.unwrap();
        assert_eq!(hooks[0].forward_sources, "discord,slack");
    }

    #[tokio::test]
    async fn count_webhooks() {
        let db = setup_db().await;
        assert_eq!(db.count_webhooks().await.unwrap(), 0);

        db.create_webhook("telegram", "https://example.com/h1", "message", "")
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
        };
        assert!(wh.should_forward_source("telegram"));
        assert!(wh.should_forward_source("discord"));
        assert!(!wh.should_forward_source("slack"));
    }
}
