use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{debug, error, info, warn};

use matrix_bridge_core::config::PermissionsConfig;
use matrix_bridge_core::error::BridgeError;
use matrix_bridge_core::message::{BridgeMessage, ExternalRoom, ExternalUser, MessageContent};
use matrix_bridge_core::platform::{self, BridgePlatform};
use matrix_bridge_store::Database;

use crate::crypto_manager::CryptoManager;
use crate::matrix_client::MatrixClient;
use crate::puppet_manager::PuppetManager;

/// Routes events between Matrix and external platforms.
///
/// - Matrix -> Platform: Receives Matrix room events, looks up the room mapping,
///   and forwards to the appropriate platform plugin.
/// - Platform -> Matrix: Receives BridgeMessages from platform plugins,
///   ensures puppets exist, and sends messages to Matrix rooms.
pub struct Dispatcher {
    platforms: HashMap<String, Arc<dyn BridgePlatform>>,
    puppet_manager: Arc<PuppetManager>,
    matrix_client: MatrixClient,
    db: Database,
    _server_name: String,
    /// The bridge bot's full Matrix user ID (e.g. `@bridge_bot:example.com`).
    bot_user_id: String,
    /// Prefix for puppet user localparts (e.g. `"bot"` → `@bot_telegram_12345:domain`).
    puppet_prefix: String,
    /// Optional crypto manager for encrypting outbound messages.
    crypto: Option<Arc<CryptoManager>>,
    /// Whether to auto-enable encryption for rooms on link.
    encryption_default: bool,
    /// Permission settings (invite whitelist, etc.).
    permissions: PermissionsConfig,
}

impl Dispatcher {
    pub fn new(
        puppet_manager: Arc<PuppetManager>,
        matrix_client: MatrixClient,
        db: Database,
        server_name: &str,
        sender_localpart: &str,
        puppet_prefix: &str,
        permissions: PermissionsConfig,
    ) -> Self {
        Self {
            platforms: HashMap::new(),
            puppet_manager,
            matrix_client,
            db,
            _server_name: server_name.to_string(),
            bot_user_id: format!("@{sender_localpart}:{server_name}"),
            puppet_prefix: puppet_prefix.to_string(),
            crypto: None,
            encryption_default: false,
            permissions,
        }
    }

    /// Set the crypto manager for E2BE encryption.
    pub fn set_crypto(&mut self, crypto: Arc<CryptoManager>, encryption_default: bool) {
        self.crypto = Some(crypto);
        self.encryption_default = encryption_default;
    }

    /// Register a platform plugin.
    pub fn register_platform(&mut self, platform: Arc<dyn BridgePlatform>) {
        let id = platform.id().to_string();
        info!(platform = id, "registered platform");
        self.platforms.insert(id, platform);
    }

    /// Handle a batch of events from the homeserver transaction endpoint.
    pub async fn handle_transaction(&self, events: &[Value], crypto: Option<&CryptoManager>) {
        for event in events {
            if let Err(e) = self.handle_event(event, crypto).await {
                error!("failed to handle event: {e}");
            }
        }
    }

    /// Handle a single Matrix event.
    async fn handle_event(
        &self,
        event: &Value,
        crypto: Option<&CryptoManager>,
    ) -> anyhow::Result<()> {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let room_id = event.get("room_id").and_then(|v| v.as_str()).unwrap_or("");
        let sender = event.get("sender").and_then(|v| v.as_str()).unwrap_or("");

        // m.room.member events must be processed even when sent by the bot
        // itself (e.g. self-invite), so check membership before the bot skip.
        if event_type == "m.room.member" {
            return self.handle_membership(room_id, event, crypto).await;
        }

        // Skip events from the bridge bot itself (not puppet users — those need
        // cross-platform forwarding).
        if self.is_bridge_bot(sender) {
            return Ok(());
        }

        match event_type {
            "m.room.message" => self.handle_room_message(room_id, sender, event).await,
            "m.room.encrypted" => {
                self.handle_encrypted_event(room_id, sender, event, crypto)
                    .await
            }
            "m.room.encryption" => {
                // Track that this room is now encrypted and query member device keys.
                if let Some(crypto) = crypto {
                    let ruma_room_id: &ruma::RoomId = room_id.try_into()?;
                    crypto.set_room_encrypted(ruma_room_id).await?;
                    if let Err(e) = self.update_tracked_users(room_id, crypto).await {
                        warn!(room_id, "failed to track users on encryption event: {e}");
                    }
                }
                Ok(())
            }
            "m.room.redaction" => self.handle_redaction(room_id, sender, event).await,
            _ => {
                debug!(event_type, room_id, "ignoring event type");
                Ok(())
            }
        }
    }

    /// Handle m.room.member events — auto-accept invites for the bridge bot
    /// and puppet users.
    ///
    /// When the bridge bot is invited to a room, it automatically joins.
    /// This is essential because:
    /// 1. The bot must be IN the room to receive events (messages, encryption state).
    /// 2. E2EE requires the bot to be a member for Megolm key sharing.
    /// 3. The `!bridge link` command can only be processed if the bot is in the room.
    ///
    /// For puppet users, auto-accept ensures they can send messages on behalf
    /// of external platform users.
    async fn handle_membership(
        &self,
        room_id: &str,
        event: &Value,
        crypto: Option<&CryptoManager>,
    ) -> anyhow::Result<()> {
        let membership = event
            .get("content")
            .and_then(|c| c.get("membership"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let state_key = event
            .get("state_key")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if membership != "invite" || state_key.is_empty() {
            return Ok(());
        }

        // Check if the invited user is the bridge bot or a puppet user.
        let is_bot = state_key == self.bot_user_id;
        let is_puppet = state_key.starts_with(&format!("@{}_", self.puppet_prefix));

        if !is_bot && !is_puppet {
            return Ok(());
        }

        // Check invite whitelist for both bot and puppet invites.
        // The bridge bot itself is always allowed (it invites puppets internally).
        let inviter = event.get("sender").and_then(|v| v.as_str()).unwrap_or("");

        let is_bridge_bot_inviting = inviter == self.bot_user_id;
        if !is_bridge_bot_inviting && !self.permissions.is_invite_allowed(inviter) {
            let target = if is_bot { "bot" } else { "puppet" };
            warn!(
                room_id,
                inviter, target, "invite rejected: sender not in invite_whitelist"
            );
            return Ok(());
        }

        // Auto-accept the invite.
        info!(
            room_id,
            invited_user = state_key,
            is_bot,
            "auto-accepting room invite"
        );

        if let Err(e) = self.matrix_client.join_room(room_id, state_key).await {
            warn!(
                room_id,
                invited_user = state_key,
                "failed to auto-accept invite: {e}"
            );
            return Ok(());
        }

        // If the bridge bot just joined an encrypted room, track device keys.
        if is_bot && let Some(crypto) = crypto {
            let ruma_room_id: Result<&ruma::RoomId, _> = room_id.try_into();
            if let Ok(ruma_room_id) = ruma_room_id
                && crypto.is_room_encrypted(ruma_room_id).await
                && let Err(e) = self.update_tracked_users(room_id, crypto).await
            {
                warn!(room_id, "failed to track users after bot join: {e}");
            }
        }

        Ok(())
    }

    /// Handle an m.room.encrypted event — decrypt and process the inner event.
    async fn handle_encrypted_event(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
        crypto: Option<&CryptoManager>,
    ) -> anyhow::Result<()> {
        let Some(crypto) = crypto else {
            debug!(room_id, "received encrypted event but E2EE is not enabled");
            return Ok(());
        };

        let ruma_room_id: &ruma::RoomId = room_id.try_into()?;

        // Ensure we have tracked all room members' devices before decrypting.
        if let Err(e) = self.update_tracked_users(room_id, crypto).await {
            warn!(
                room_id,
                "failed to update tracked users before decrypt: {e}"
            );
        }

        let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");
        let decrypted = match crypto.decrypt(ruma_room_id, event).await {
            Ok(d) => d,
            Err(e) => {
                error!(
                    room_id,
                    sender, event_id, "failed to decrypt event (message will be dropped): {e}"
                );
                return Ok(());
            }
        };

        match decrypted.event_type.as_str() {
            "m.room.message" => {
                // Reconstruct a pseudo-event with the decrypted content.
                let mut pseudo_event = event.clone();
                pseudo_event["type"] = "m.room.message".into();
                pseudo_event["content"] = decrypted.content;
                if !decrypted.sender.is_empty() {
                    pseudo_event["sender"] = decrypted.sender.into();
                }
                self.handle_room_message(room_id, sender, &pseudo_event)
                    .await
            }
            other => {
                debug!(event_type = other, room_id, "ignoring decrypted event type");
                Ok(())
            }
        }
    }

    /// Handle an m.room.message event from Matrix -> external platform.
    ///
    /// Queries the database for all room mappings of this Matrix room,
    /// then delivers to registered webhooks for each platform.
    /// Does not depend on `self.platforms` — works with dynamically-created mappings.
    async fn handle_room_message(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let content = event.get("content").cloned().unwrap_or_default();
        let msgtype = content
            .get("msgtype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body = content.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");

        // Check for management commands.
        if body.starts_with("!bridge") {
            return self.handle_command(room_id, sender, body).await;
        }

        // Find all platforms that have a mapping for this room.
        let mappings = self.db.find_all_mappings_by_matrix_id(room_id).await?;
        if mappings.is_empty() {
            return Ok(());
        }

        // Check if the sender is allowed to have messages forwarded externally.
        // Puppet users (bridge-internal) skip this check — they relay external messages.
        let is_puppet_sender = sender.starts_with(&format!("@{}_", self.puppet_prefix));
        if !is_puppet_sender && !self.permissions.is_invite_allowed(sender) {
            debug!(
                sender,
                room_id, "message forwarding blocked: sender not in invite_whitelist"
            );
            return Ok(());
        }

        let message_content = match msgtype {
            "m.text" => {
                let formatted = content
                    .get("formatted_body")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                MessageContent::Text {
                    body: body.to_string(),
                    formatted_body: formatted,
                }
            }
            "m.notice" => MessageContent::Notice {
                body: body.to_string(),
            },
            "m.emote" => MessageContent::Emote {
                body: body.to_string(),
            },
            "m.image" => {
                let url = content
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mimetype = content
                    .get("info")
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("image/png")
                    .to_string();
                MessageContent::Image {
                    url,
                    caption: Some(body.to_string()).filter(|s| !s.is_empty()),
                    mimetype,
                }
            }
            "m.file" => {
                let url = content
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mimetype = content
                    .get("info")
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                MessageContent::File {
                    url,
                    filename: body.to_string(),
                    mimetype,
                }
            }
            "m.video" => {
                let url = content
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mimetype = content
                    .get("info")
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("video/mp4")
                    .to_string();
                MessageContent::Video {
                    url,
                    caption: Some(body.to_string()).filter(|s| !s.is_empty()),
                    mimetype,
                }
            }
            "m.audio" => {
                let url = content
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mimetype = content
                    .get("info")
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("audio/ogg")
                    .to_string();
                MessageContent::Audio { url, mimetype }
            }
            _ => {
                debug!(msgtype, "unsupported message type, skipping outbound");
                return Ok(());
            }
        };

        let timestamp = event
            .get("origin_server_ts")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Look up puppet from DB first (authoritative platform_id), then
        // fall back to string-based extraction for unregistered puppets.
        let puppet_record = self
            .db
            .find_puppet_by_matrix_id(sender)
            .await
            .ok()
            .flatten();

        let source_platform = puppet_record
            .as_ref()
            .map(|p| p.platform_id.clone())
            .or_else(|| platform::puppet_source_platform(sender, &self.puppet_prefix));

        if let Some(ref p) = source_platform {
            debug!(
                sender,
                source_platform = p.as_str(),
                "puppet user detected, will skip source platform"
            );
        }

        let original_sender = puppet_record;

        for mapping in &mappings {
            // Skip forwarding back to the puppet's source platform (loop prevention).
            if let Some(ref src) = source_platform
                && mapping.platform_id == *src
            {
                debug!(
                    platform = mapping.platform_id,
                    sender, "skipping source platform to prevent loop"
                );
                continue;
            }

            let bridge_sender = if let Some(ref puppet) = original_sender {
                ExternalUser {
                    platform: puppet.platform_id.clone(),
                    external_id: puppet.external_user_id.clone(),
                    display_name: puppet.display_name.clone(),
                    avatar_url: puppet.avatar_mxc.clone(),
                }
            } else {
                ExternalUser {
                    platform: "matrix".to_string(),
                    external_id: sender.to_string(),
                    display_name: None,
                    avatar_url: None,
                }
            };

            let bridge_msg = BridgeMessage {
                id: event_id.to_string(),
                sender: bridge_sender,
                room: ExternalRoom {
                    platform: mapping.platform_id.clone(),
                    external_id: mapping.external_room_id.clone(),
                    name: None,
                },
                content: message_content.clone(),
                timestamp,
                reply_to: None,
            };

            // Deliver to all webhooks for this platform.
            match self
                .deliver_to_webhooks(
                    &mapping.platform_id,
                    &bridge_msg,
                    source_platform.as_deref(),
                )
                .await
            {
                Ok(()) => {
                    self.db
                        .create_message_mapping(
                            event_id,
                            &mapping.platform_id,
                            &bridge_msg.id,
                            mapping.id,
                        )
                        .await?;
                    debug!(
                        event_id,
                        platform = mapping.platform_id,
                        "message bridged to platform webhooks"
                    );
                }
                Err(e) => {
                    error!(
                        platform = mapping.platform_id,
                        "failed to deliver to webhooks: {e}"
                    );
                }
            }
        }
        Ok(())
    }

    /// Deliver a message to all registered webhooks for a platform.
    ///
    /// `source_platform` is the originating platform (if any) — used to check
    /// per-webhook `exclude_sources` filters.
    async fn deliver_to_webhooks(
        &self,
        platform_id: &str,
        message: &BridgeMessage,
        source_platform: Option<&str>,
    ) -> anyhow::Result<()> {
        let webhooks = self.db.list_webhooks(platform_id).await?;
        if webhooks.is_empty() {
            debug!(platform = platform_id, "no webhooks registered, skipping");
            return Ok(());
        }

        let mut payload = serde_json::json!({
            "event": "message",
            "platform": platform_id,
            "message": message,
        });
        // Include the originating platform so the receiver can tell where
        // the message came from (e.g. "telegram" when forwarding to Slack).
        if let Some(src) = source_platform {
            payload["source_platform"] = serde_json::Value::String(src.to_string());
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;

        for webhook in &webhooks {
            // Check per-webhook source exclusion filter.
            if let Some(src) = source_platform
                && webhook.is_source_excluded(src)
            {
                debug!(
                    platform = platform_id,
                    url = webhook.webhook_url,
                    source = src,
                    "webhook excluded this source platform"
                );
                continue;
            }
            match client
                .post(&webhook.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        debug!(
                            platform = platform_id,
                            url = webhook.webhook_url,
                            "webhook delivered"
                        );
                    } else {
                        warn!(
                            platform = platform_id,
                            url = webhook.webhook_url,
                            status = %resp.status(),
                            "webhook delivery got non-2xx response"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        platform = platform_id,
                        url = webhook.webhook_url,
                        "webhook delivery failed: {e}"
                    );
                }
            }
        }
        Ok(())
    }

    /// Handle an m.room.redaction event.
    async fn handle_redaction(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let redacts = event.get("redacts").and_then(|v| v.as_str()).unwrap_or("");
        if redacts.is_empty() {
            return Ok(());
        }

        let msg_mapping = self.db.find_message_by_matrix_id(redacts).await?;
        let Some(msg_mapping) = msg_mapping else {
            return Ok(());
        };

        let room_mapping = self
            .db
            .find_room_by_matrix_id(room_id, &msg_mapping.platform_id)
            .await?;
        let Some(room_mapping) = room_mapping else {
            return Ok(());
        };

        let bridge_msg = BridgeMessage {
            id: redacts.to_string(),
            sender: ExternalUser {
                platform: "matrix".to_string(),
                external_id: sender.to_string(),
                display_name: None,
                avatar_url: None,
            },
            room: ExternalRoom {
                platform: msg_mapping.platform_id.clone(),
                external_id: room_mapping.external_room_id,
                name: None,
            },
            content: MessageContent::Redaction {
                target_id: msg_mapping.external_message_id,
            },
            timestamp: 0,
            reply_to: None,
        };

        if let Err(e) = self
            .deliver_to_webhooks(
                &msg_mapping.platform_id,
                &bridge_msg,
                platform::puppet_source_platform(sender, &self.puppet_prefix).as_deref(),
            )
            .await
        {
            error!("failed to bridge redaction: {e}");
        }

        Ok(())
    }

    /// Ensure a puppet user can access a Matrix room.
    ///
    /// Flow:
    /// 1. Try puppet join directly (works if room is public or puppet was already invited).
    /// 2. If that fails, ensure the bridge bot is in the room first.
    /// 3. Bridge bot invites the puppet.
    /// 4. Puppet retries join after invite.
    async fn ensure_room_access(
        &self,
        room_id: &str,
        puppet_user_id: &str,
    ) -> Result<(), BridgeError> {
        // Try direct join first.
        if self
            .matrix_client
            .join_room(room_id, puppet_user_id)
            .await
            .is_ok()
        {
            return Ok(());
        }

        // Direct join failed — ensure bridge bot is in the room.
        debug!(
            room_id,
            puppet_user_id, "direct puppet join failed, ensuring bridge bot access"
        );
        self.matrix_client
            .join_room(room_id, &self.bot_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("bridge bot join failed: {e}")))?;

        // Bridge bot invites puppet.
        self.matrix_client
            .invite_user(room_id, puppet_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("invite puppet failed: {e}")))?;

        // Puppet retries join after invite.
        self.matrix_client
            .join_room(room_id, puppet_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("puppet join after invite failed: {e}")))?;

        Ok(())
    }

    /// Handle an incoming message from an external platform -> Matrix.
    pub async fn handle_incoming(&self, message: BridgeMessage) -> Result<(), BridgeError> {
        let platform_id = &message.room.platform;
        let platform = self
            .platforms
            .get(platform_id)
            .ok_or_else(|| BridgeError::NotFound(format!("platform {platform_id}")))?;

        // Find the Matrix room for this external room.
        let room_mapping = self
            .db
            .find_room_by_external_id(platform_id, &message.room.external_id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        let Some(room_mapping) = room_mapping else {
            debug!(
                platform = platform_id,
                external_room = message.room.external_id,
                "no room mapping found, ignoring message"
            );
            return Ok(());
        };

        // Ensure puppet user exists.
        let puppet_user_id = self
            .puppet_manager
            .ensure_puppet(platform.as_ref(), &message.sender)
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

        // Ensure puppet can access the room (join, or invite+join).
        self.ensure_room_access(&room_mapping.matrix_room_id, &puppet_user_id)
            .await?;

        // Convert message content to Matrix format and send (with encryption if room is encrypted).
        let (content, txn_id) = self.to_matrix_content(&message);

        let event_id = self
            .send_to_matrix(
                &room_mapping.matrix_room_id,
                &content,
                &puppet_user_id,
                &txn_id,
            )
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

        // Record the message mapping.
        self.db
            .create_message_mapping(&event_id, platform_id, &message.id, room_mapping.id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        debug!(
            event_id,
            platform = platform_id,
            "message bridged to matrix"
        );
        Ok(())
    }

    /// Convert a BridgeMessage to Matrix message content JSON.
    fn to_matrix_content(&self, message: &BridgeMessage) -> (Value, String) {
        let txn_id = ulid::Ulid::new().to_string();

        let content = match &message.content {
            MessageContent::Text {
                body,
                formatted_body,
            } => {
                let mut c = serde_json::json!({
                    "msgtype": "m.text",
                    "body": body,
                });
                if let Some(html) = formatted_body {
                    c["format"] = "org.matrix.custom.html".into();
                    c["formatted_body"] = html.clone().into();
                }
                c
            }
            MessageContent::Notice { body } => serde_json::json!({
                "msgtype": "m.notice",
                "body": body,
            }),
            MessageContent::Emote { body } => serde_json::json!({
                "msgtype": "m.emote",
                "body": body,
            }),
            MessageContent::Image {
                url,
                caption,
                mimetype,
            } => serde_json::json!({
                "msgtype": "m.image",
                "body": caption.as_deref().unwrap_or("image"),
                "url": url,
                "info": { "mimetype": mimetype },
            }),
            MessageContent::File {
                url,
                filename,
                mimetype,
            } => serde_json::json!({
                "msgtype": "m.file",
                "body": filename,
                "url": url,
                "info": { "mimetype": mimetype },
            }),
            MessageContent::Video {
                url,
                caption,
                mimetype,
            } => serde_json::json!({
                "msgtype": "m.video",
                "body": caption.as_deref().unwrap_or("video"),
                "url": url,
                "info": { "mimetype": mimetype },
            }),
            MessageContent::Audio { url, mimetype } => serde_json::json!({
                "msgtype": "m.audio",
                "body": "audio",
                "url": url,
                "info": { "mimetype": mimetype },
            }),
            MessageContent::Location {
                latitude,
                longitude,
            } => serde_json::json!({
                "msgtype": "m.location",
                "body": format!("Location: {latitude}, {longitude}"),
                "geo_uri": format!("geo:{latitude},{longitude}"),
            }),
            _ => serde_json::json!({
                "msgtype": "m.text",
                "body": "[unsupported message type]",
            }),
        };

        (content, txn_id)
    }

    /// Enable encryption on a room and register it with the crypto manager.
    pub async fn enable_room_encryption(&self, room_id: &str) -> anyhow::Result<()> {
        self.matrix_client.enable_room_encryption(room_id).await?;
        if let Some(crypto) = &self.crypto {
            let ruma_room_id: &ruma::RoomId = room_id.try_into()?;
            crypto.set_room_encrypted(ruma_room_id).await?;
        }
        Ok(())
    }

    /// Query device keys for all members of a room so the crypto manager
    /// can track their devices and receive Megolm session keys.
    async fn update_tracked_users(
        &self,
        room_id: &str,
        crypto: &CryptoManager,
    ) -> anyhow::Result<()> {
        let members_str = self.matrix_client.get_room_members(room_id).await?;
        let members: Vec<ruma::OwnedUserId> =
            members_str.iter().filter_map(|m| m.parse().ok()).collect();
        if !members.is_empty() {
            crypto.update_tracked_users(&members).await?;
        }
        Ok(())
    }

    /// Send a message to a Matrix room, encrypting if the room has encryption enabled.
    async fn send_to_matrix(
        &self,
        room_id: &str,
        content: &Value,
        as_user: &str,
        txn_id: &str,
    ) -> anyhow::Result<String> {
        if let Some(crypto) = &self.crypto {
            let ruma_room_id: &ruma::RoomId = room_id.try_into()?;
            if crypto.is_room_encrypted(ruma_room_id).await {
                // Get room members for key sharing.
                let members_str = self.matrix_client.get_room_members(room_id).await?;
                let members: Vec<ruma::OwnedUserId> =
                    members_str.iter().filter_map(|m| m.parse().ok()).collect();

                let encrypted = crypto
                    .encrypt(ruma_room_id, "m.room.message", content, &members)
                    .await?;

                return self
                    .matrix_client
                    .send_encrypted_message(room_id, &encrypted, as_user, txn_id)
                    .await;
            }
        }
        self.matrix_client
            .send_message(room_id, content, as_user, txn_id)
            .await
    }

    /// Handle !bridge management commands.
    /// Requires the sender to have moderator power level (>= 50) for link/unlink.
    async fn handle_command(&self, room_id: &str, sender: &str, body: &str) -> anyhow::Result<()> {
        let parts: Vec<&str> = body.split_whitespace().collect();
        let subcommand = parts.get(1).copied();

        // link/unlink require moderator power level.
        if matches!(subcommand, Some("link") | Some("unlink")) {
            let power_level = self
                .matrix_client
                .get_user_power_level(room_id, sender)
                .await
                .unwrap_or(0);
            if power_level < 50 {
                info!(
                    sender,
                    room_id, power_level, "bridge command denied: insufficient power level"
                );
                return Ok(());
            }
        }

        match subcommand {
            Some("link") => {
                // !bridge link <platform> <external_room_id>
                let platform_id = parts.get(2).copied().unwrap_or("");
                let external_id = parts.get(3).copied().unwrap_or("");
                if platform_id.is_empty() || external_id.is_empty() {
                    info!("usage: !bridge link <platform> <external_room_id>");
                    return Ok(());
                }
                self.db
                    .create_room_mapping(room_id, platform_id, external_id)
                    .await?;

                // Ensure the bridge bot is in the room so it receives events.
                if let Err(e) = self
                    .matrix_client
                    .join_room(room_id, &self.bot_user_id)
                    .await
                {
                    warn!(room_id, "bridge bot failed to join linked room: {e}");
                }

                // Auto-enable encryption if configured.
                if self.encryption_default
                    && let Err(e) = self.enable_room_encryption(room_id).await
                {
                    warn!(room_id, "failed to auto-enable encryption: {e}");
                }

                // If E2EE is active, query device keys of room members so they
                // can share Megolm sessions with the bridge bot.
                if let Some(crypto) = &self.crypto
                    && let Err(e) = self.update_tracked_users(room_id, crypto).await
                {
                    warn!(room_id, "failed to update tracked users: {e}");
                }

                info!(room_id, platform_id, external_id, "room linked");
            }
            Some("unlink") => {
                let platform_id = parts.get(2).copied().unwrap_or("");
                if platform_id.is_empty() {
                    info!("usage: !bridge unlink <platform>");
                    return Ok(());
                }
                if let Some(mapping) = self.db.find_room_by_matrix_id(room_id, platform_id).await? {
                    self.db.delete_room_mapping(mapping.id).await?;
                    info!(room_id, platform_id, "room unlinked");
                }
            }
            Some("status") => {
                info!(
                    "bridge status: {} platforms registered",
                    self.platforms.len()
                );
                for id in self.platforms.keys() {
                    info!("  platform: {id}");
                }
            }
            _ => {
                info!("commands: !bridge link|unlink|status");
            }
        }
        Ok(())
    }

    /// Get a reference to the database (used by bridge_api).
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get a reference to the Matrix client (used by bridge_api for uploads).
    pub fn matrix_client(&self) -> &MatrixClient {
        &self.matrix_client
    }

    /// Handle an incoming message via the HTTP bridge API (external -> Matrix).
    ///
    /// Unlike `handle_incoming`, this does not require a registered BridgePlatform.
    /// It directly creates a puppet user and sends the message to the mapped Matrix room.
    ///
    /// If no room mapping exists, automatically creates a portal room (the bridge
    /// bot is the room creator and is therefore already a member).
    pub async fn handle_incoming_http(
        &self,
        message: BridgeMessage,
    ) -> Result<String, BridgeError> {
        let platform_id = &message.room.platform;

        // Find the Matrix room for this external room, or auto-create a portal room.
        let room_mapping = self
            .db
            .find_room_by_external_id(platform_id, &message.room.external_id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        let room_mapping = match room_mapping {
            Some(m) => m,
            None => {
                // Auto-create a portal room. The bridge bot is the creator,
                // so it is automatically a member — no invite/join needed.
                let room_name = message
                    .room
                    .name
                    .as_deref()
                    .unwrap_or(&message.room.external_id);

                info!(
                    platform = platform_id,
                    external_room = message.room.external_id,
                    room_name,
                    "auto-creating portal room"
                );

                let matrix_room_id = self
                    .matrix_client
                    .create_room(Some(room_name), &[], self.encryption_default)
                    .await
                    .map_err(|e| {
                        BridgeError::Matrix(format!("portal room creation failed: {e}"))
                    })?;

                // If encryption was enabled, track it in the crypto store.
                if self.encryption_default
                    && let Some(crypto) = &self.crypto
                    && let Ok(ruma_room_id) = <&ruma::RoomId>::try_from(matrix_room_id.as_str())
                {
                    let _ = crypto.set_room_encrypted(ruma_room_id).await;
                }

                // Register the room mapping.
                let id = self
                    .db
                    .create_room_mapping(&matrix_room_id, platform_id, &message.room.external_id)
                    .await
                    .map_err(|e| BridgeError::Store(e.to_string()))?;

                info!(
                    matrix_room_id,
                    platform = platform_id,
                    external_room = message.room.external_id,
                    "portal room created and mapped"
                );

                matrix_bridge_store::RoomMapping {
                    id,
                    matrix_room_id,
                    platform_id: platform_id.to_string(),
                    external_room_id: message.room.external_id.clone(),
                }
            }
        };

        // Build puppet localpart from prefix + platform + user ID.
        // Validate that the localpart only contains allowed Matrix characters.
        let localpart = platform::puppet_localpart(
            &self.puppet_prefix,
            platform_id,
            &message.sender.external_id,
        );
        if !is_valid_localpart(&localpart) {
            return Err(BridgeError::Validation(format!(
                "invalid localpart: {localpart}"
            )));
        }
        let puppet_user_id = self
            .puppet_manager
            .ensure_puppet_direct(
                &localpart,
                platform_id,
                &message.sender.external_id,
                message.sender.display_name.as_deref(),
                message.sender.avatar_url.as_deref(),
            )
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

        // Ensure puppet can access the room (join, or invite+join).
        self.ensure_room_access(&room_mapping.matrix_room_id, &puppet_user_id)
            .await?;

        // Convert and send to Matrix (with encryption if room is encrypted).
        let (content, txn_id) = self.to_matrix_content(&message);
        let event_id = self
            .send_to_matrix(
                &room_mapping.matrix_room_id,
                &content,
                &puppet_user_id,
                &txn_id,
            )
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

        // Record message mapping.
        self.db
            .create_message_mapping(&event_id, platform_id, &message.id, room_mapping.id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        debug!(
            event_id,
            platform = platform_id,
            "message bridged to matrix via HTTP API"
        );
        Ok(event_id)
    }

    /// Check if a sender is the bridge bot itself.
    /// The bridge bot's own messages should always be ignored.
    fn is_bridge_bot(&self, sender: &str) -> bool {
        sender == self.bot_user_id
    }
}

/// Validate that a localpart only contains allowed Matrix user ID characters.
/// Per the Matrix spec, localparts must match `[a-z0-9._\-=/]+`.
fn is_valid_localpart(localpart: &str) -> bool {
    !localpart.is_empty()
        && localpart
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-=/".contains(&b))
}
