use tracing::{debug, error, info, warn};

use matrix_bridge_core::message::BridgeMessage;

use super::Dispatcher;

impl Dispatcher {
    /// Handle !bridge management commands.
    pub(super) async fn handle_command(
        &self,
        room_id: &str,
        sender: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        let parts: Vec<&str> = body.split_whitespace().collect();
        let subcommand = parts.get(1).copied();

        if matches!(subcommand, Some("link") | Some("unlink")) {
            let power_level = self
                .matrix_client
                .get_user_power_level(room_id, sender)
                .await
                .unwrap_or(0);
            if power_level < 50 {
                warn!(
                    sender,
                    room_id, power_level, "bridge command denied: insufficient power level"
                );
                return Ok(());
            }
        }

        match subcommand {
            Some("link") => {
                let platform_id = parts.get(2).copied().unwrap_or("");
                let external_id = parts.get(3).copied().unwrap_or("");
                if platform_id.is_empty() || external_id.is_empty() {
                    debug!("usage: !bridge link <platform> <external_room_id>");
                    return Ok(());
                }
                if platform_id.len() > 64
                    || !platform_id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
                {
                    debug!(
                        "invalid platform ID: must be alphanumeric/dash/underscore/dot, max 64 chars"
                    );
                    return Ok(());
                }
                if external_id.len() > 255 {
                    debug!("external_room_id too long: max 255 chars");
                    return Ok(());
                }
                self.db
                    .create_room_mapping(room_id, platform_id, external_id)
                    .await?;

                if let Err(e) = self
                    .matrix_client
                    .join_room(room_id, &self.bot_user_id)
                    .await
                {
                    warn!(room_id, error = %e, "bridge bot failed to join linked room");
                }

                if self.encryption_default
                    && let Err(e) = self.enable_room_encryption(room_id).await
                {
                    warn!(room_id, error = %e, "failed to auto-enable encryption");
                }

                self.track_all_room_members(room_id).await;

                info!(room_id, platform_id, external_id, "room linked");
            }
            Some("unlink") => {
                let platform_id = parts.get(2).copied().unwrap_or("");
                if platform_id.is_empty() {
                    debug!("usage: !bridge unlink <platform>");
                    return Ok(());
                }
                if let Some(mapping) = self.db.find_room_by_matrix_id(room_id, platform_id).await? {
                    self.db.delete_room_mapping(mapping.id).await?;
                    info!(room_id, platform_id, "room unlinked");
                }
            }
            Some("status") => {
                let mappings = self.db.find_all_mappings_by_matrix_id(room_id).await?;
                debug!(room_id, mapping_count = mappings.len(), "bridge status");
                for m in &mappings {
                    debug!(
                        platform = m.platform_id,
                        external_room = m.external_room_id,
                        "  mapping"
                    );
                }
            }
            _ => {
                debug!("commands: !bridge link|unlink|status");
            }
        }
        Ok(())
    }

    /// Deliver a message to all registered webhooks for a platform.
    pub(super) async fn deliver_to_webhooks(
        &self,
        platform_id: &str,
        message: &BridgeMessage,
        source_platform: Option<&str>,
    ) -> anyhow::Result<()> {
        // When relay is disabled, only forward messages from Matrix users
        // (source_platform is None = real Matrix user). Cross-platform relay
        // (e.g. telegram -> discord) is blocked.
        if !self.allow_relay && source_platform.is_some() {
            return Ok(());
        }
        let mut payload = serde_json::json!({
            "event": "message",
            "platform": platform_id,
            "message": message,
        });
        if let Some(src) = source_platform {
            payload["source_platform"] = serde_json::Value::String(src.to_string());
        }

        // Serialize once for both webhooks and WebSocket clients.
        let payload_str = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "failed to serialize webhook payload");
                return Ok(());
            }
        };

        // Deliver to WebSocket clients (non-blocking, lock-free).
        self.ws_registry
            .broadcast(platform_id, &payload_str, source_platform);

        // Deliver to HTTP webhooks.
        let webhooks = self.db.list_webhooks(platform_id).await?;
        if webhooks.is_empty() {
            debug!(platform = platform_id, "no webhooks registered, skipping");
            return Ok(());
        }

        let mut delivery_futures = Vec::new();
        for webhook in &webhooks {
            // forward_sources allowlist: empty = deny all, "*" = allow all,
            // "telegram,discord" = allow only those.
            let should_forward = match source_platform {
                Some(src) => webhook.should_forward_source(src),
                // No source_platform means the message originates from a real
                // Matrix user (not a puppet).  Treat "matrix" as the source.
                None => webhook.should_forward_source("matrix"),
            };
            if !should_forward {
                debug!(
                    platform = platform_id,
                    url = webhook.webhook_url,
                    source = source_platform.unwrap_or("matrix"),
                    forward_sources = webhook.forward_sources,
                    "webhook does not forward this source platform"
                );
                continue;
            }

            let client = self.http_client.clone();
            let url = webhook.webhook_url.clone();
            let body = payload_str.clone();
            let platform = platform_id.to_string();

            delivery_futures.push(tokio::spawn(async move {
                match client
                    .post(&url)
                    .body(body)
                    .header("Content-Type", "application/json")
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            debug!(platform, url, "webhook delivered");
                        } else {
                            warn!(platform, url, status = %resp.status(), "webhook got non-2xx");
                        }
                    }
                    Err(e) => {
                        error!(platform, url, error = %e, "webhook delivery failed");
                    }
                }
            }));
        }

        // Wait for all deliveries (but they run concurrently).
        for fut in delivery_futures {
            if let Err(e) = fut.await {
                error!(error = %e, "webhook delivery task panicked");
            }
        }

        Ok(())
    }
}
