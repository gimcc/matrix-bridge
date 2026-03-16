use matrix_bridge_core::config::PermissionLevel;
use tracing::{debug, error, warn};

use super::Dispatcher;

impl Dispatcher {
    /// Handle commands sent via DM to the bridge bot's main account.
    ///
    /// Only **admin** users can use bot commands. Relay users can only
    /// have their messages forwarded; they cannot execute commands.
    /// Unauthorized users receive no response (silent deny).
    ///
    /// Supported commands (admin only):
    /// - `!help` — list available commands
    /// - `!platforms` — list all known platforms with summary
    /// - `!rooms [platform]` — list bridged room mappings
    /// - `!spaces` — list platform spaces
    /// - `!<platform>` — show platform details (capabilities, rooms, integrations)
    /// - `!<platform> <command...>` — forward custom command to external platform
    pub async fn handle_bot_command(
        &self,
        room_id: &str,
        sender: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        if self.permissions.permission_level(sender) < PermissionLevel::Admin {
            debug!(sender, "bot command denied: admin required");
            return Ok(());
        }

        let parts: Vec<&str> = body.split_whitespace().collect();
        let cmd = parts.first().copied().unwrap_or("");

        match cmd {
            "!help" => self.cmd_help(room_id).await,
            "!rooms" => {
                let platform_filter = parts.get(1).copied();
                self.cmd_rooms(room_id, platform_filter).await
            }
            "!platforms" => self.cmd_platforms(room_id).await,
            "!spaces" => self.cmd_spaces(room_id).await,
            _ => {
                let platform = cmd.strip_prefix('!').unwrap_or("");
                if platform.is_empty() {
                    return Ok(());
                }
                if parts.len() == 1 {
                    // `!<platform>` with no args → show platform info.
                    self.cmd_platform_info(room_id, platform).await
                } else {
                    // `!<platform> <command...>` → passthrough to external.
                    let command_body = parts[1..].join(" ");
                    self.cmd_platform_passthrough(room_id, sender, platform, &command_body)
                        .await
                }
            }
        }
    }

    /// Reply to the user in the DM room.
    async fn reply(&self, room_id: &str, text: &str) {
        if let Err(e) = self.matrix_client.send_text_as_bot(room_id, text).await {
            error!(room_id, error = %e, "failed to send bot reply");
        }
    }

    /// `!help` — show available commands.
    async fn cmd_help(&self, room_id: &str) -> anyhow::Result<()> {
        let text = "\
Commands (DM):
  !help                  — show this help
  !platforms             — list all platforms with summary
  !rooms [platform]      — list bridged room mappings
  !spaces                — list platform spaces
  !<platform>            — show platform details and capabilities
  !<platform> <command>  — forward a custom command to a platform

Room commands (in bridged rooms):
  !bridge link <platform> <external_id>  — link room to platform
  !bridge unlink <platform>              — unlink room from platform
  !bridge status                         — show room bridge status";

        self.reply(room_id, text).await;
        Ok(())
    }

    /// `!rooms [platform]` — list bridged room mappings.
    async fn cmd_rooms(&self, room_id: &str, platform_filter: Option<&str>) -> anyhow::Result<()> {
        let mappings = if let Some(platform) = platform_filter {
            self.db.list_room_mappings(platform).await?
        } else {
            self.db.list_all_room_mappings().await?
        };

        if mappings.is_empty() {
            let msg = match platform_filter {
                Some(p) => format!("No bridged rooms for platform '{p}'."),
                None => "No bridged rooms.".to_string(),
            };
            self.reply(room_id, &msg).await;
            return Ok(());
        }

        let mut lines = vec![format!("Bridged rooms ({}):", mappings.len())];
        for m in &mappings {
            lines.push(format!(
                "  [{platform}] {ext} ↔ {matrix}",
                platform = m.platform_id,
                ext = m.external_room_id,
                matrix = m.matrix_room_id,
            ));
        }
        self.reply(room_id, &lines.join("\n")).await;
        Ok(())
    }

    /// `!platforms` — list all known platforms (from room mappings, webhooks, and WS clients).
    async fn cmd_platforms(&self, room_id: &str) -> anyhow::Result<()> {
        let mut all_platforms = std::collections::BTreeSet::new();

        let db_platforms = self.db.list_all_platforms().await?;
        all_platforms.extend(db_platforms);

        let ws_platforms = self.ws_registry.list_platforms();
        all_platforms.extend(ws_platforms);

        if all_platforms.is_empty() {
            self.reply(room_id, "No platforms registered.").await;
            return Ok(());
        }

        let mut lines = vec![format!("Platforms ({}):", all_platforms.len())];
        for p in &all_platforms {
            let rooms = self
                .db
                .list_room_mappings(p)
                .await
                .map(|v| v.len())
                .unwrap_or(0);
            let webhooks = self.db.list_webhooks(p).await.map(|v| v.len()).unwrap_or(0);
            let has_ws = self.ws_registry.has_clients(p);

            let mut parts = Vec::new();
            if rooms > 0 {
                parts.push(format!("{rooms} room(s)"));
            }
            if webhooks > 0 {
                parts.push(format!("{webhooks} webhook(s)"));
            }
            if has_ws {
                parts.push("ws".to_string());
            }

            let summary = if parts.is_empty() {
                "no integrations".to_string()
            } else {
                parts.join(", ")
            };
            lines.push(format!("  {p} — {summary}"));
        }
        self.reply(room_id, &lines.join("\n")).await;
        Ok(())
    }

    /// `!spaces` — list platform spaces.
    async fn cmd_spaces(&self, room_id: &str) -> anyhow::Result<()> {
        let spaces = self.db.list_platform_spaces().await?;
        if spaces.is_empty() {
            self.reply(room_id, "No platform spaces.").await;
            return Ok(());
        }

        let mut lines = vec![format!("Platform spaces ({}):", spaces.len())];
        for space in &spaces {
            lines.push(format!(
                "  {} → {}",
                space.platform_id, space.matrix_space_id
            ));
        }
        self.reply(room_id, &lines.join("\n")).await;
        Ok(())
    }

    /// `!<platform>` (no args) — show platform details including capabilities.
    async fn cmd_platform_info(&self, room_id: &str, platform: &str) -> anyhow::Result<()> {
        let webhooks = self.db.list_webhooks(platform).await?;
        let has_ws = self.ws_registry.has_clients(platform);
        let rooms = self.db.list_room_mappings(platform).await?;
        let space = self.db.get_platform_space(platform).await?;

        if webhooks.is_empty() && !has_ws && rooms.is_empty() {
            self.reply(
                room_id,
                &format!(
                    "Unknown platform '{platform}'. Type !platforms to see available platforms."
                ),
            )
            .await;
            return Ok(());
        }

        let mut lines = vec![format!("Platform: {platform}")];

        // Integrations.
        lines.push(format!(
            "  Integrations: {} webhook(s){}",
            webhooks.len(),
            if has_ws { ", ws connected" } else { "" }
        ));

        // Rooms.
        lines.push(format!("  Rooms: {}", rooms.len()));

        // Space.
        if let Some(ref sid) = space {
            lines.push(format!("  Space: {sid}"));
        }

        // Capabilities (aggregated from webhooks + WS).
        let mut caps = std::collections::BTreeSet::new();
        for wh in &webhooks {
            for cap in wh.capabilities.split(',') {
                let cap = cap.trim();
                if !cap.is_empty() {
                    caps.insert(cap.to_string());
                }
            }
        }
        let ws_caps = self.ws_registry.get_capabilities(platform);
        caps.extend(ws_caps);

        if caps.is_empty() {
            lines.push("  Capabilities: (none declared)".to_string());
        } else {
            let caps_list: Vec<&str> = caps.iter().map(|s| s.as_str()).collect();
            lines.push(format!("  Capabilities: {}", caps_list.join(", ")));
        }

        self.reply(room_id, &lines.join("\n")).await;
        Ok(())
    }

    /// `!<platform> <command>` — forward a custom command to external platform webhooks/WS.
    async fn cmd_platform_passthrough(
        &self,
        room_id: &str,
        sender: &str,
        platform: &str,
        command_body: &str,
    ) -> anyhow::Result<()> {
        let webhooks = self.db.list_webhooks(platform).await?;
        let has_ws = self.ws_registry.has_clients(platform);

        if webhooks.is_empty() && !has_ws {
            self.reply(
                room_id,
                &format!(
                    "No integrations for platform '{platform}'. Type !platforms to see available."
                ),
            )
            .await;
            return Ok(());
        }

        let payload = serde_json::json!({
            "event": "command",
            "platform": platform,
            "sender": sender,
            "command": command_body,
            "room_id": room_id,
        });
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();

        self.ws_registry.broadcast(platform, &payload_str, None);

        for webhook in &webhooks {
            if !webhook.should_forward_source("matrix") {
                continue;
            }
            let client = self.http_client.clone();
            let url = webhook.webhook_url.clone();
            let body = payload_str.clone();
            let plat = platform.to_string();

            tokio::spawn(async move {
                match client
                    .post(&url)
                    .body(body)
                    .header("Content-Type", "application/json")
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        debug!(platform = plat, url, "command delivered to webhook");
                    }
                    Ok(resp) => {
                        warn!(platform = plat, url, status = %resp.status(), "command webhook got non-2xx");
                    }
                    Err(e) => {
                        error!(platform = plat, url, error = %e, "command webhook delivery failed");
                    }
                }
            });
        }

        debug!(
            platform,
            command = command_body,
            sender,
            "command forwarded to platform"
        );
        self.reply(room_id, &format!("Command sent to {platform}."))
            .await;
        Ok(())
    }
}
