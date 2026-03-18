use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: i64,
    pub platform_id: String,
    pub webhook_url: String,
    pub events: String,
    pub enabled: bool,
    /// Comma-separated allowlist of source platform IDs whose messages MAY be
    /// forwarded to this webhook.
    /// - Empty (`""`) = deny all (default — nothing is forwarded).
    /// - `"*"` = allow all sources.
    /// - `"telegram,discord"` = allow only those platforms.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forward_sources: String,
}

impl Webhook {
    /// Check if messages from the given source platform should be forwarded.
    pub fn should_forward_source(&self, source_platform: &str) -> bool {
        if self.forward_sources.is_empty() {
            return false;
        }
        if self.forward_sources.trim() == "*" {
            return true;
        }
        self.forward_sources
            .split(',')
            .any(|p| p.trim() == source_platform)
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
