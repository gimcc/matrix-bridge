use matrix_bridge_core::message::MessageContent;
use serde_json::Value;
use tracing::{debug, error, warn};

use super::Dispatcher;
use super::attachment_crypto::decrypt_attachment;

impl Dispatcher {
    /// Handle an m.room.message event from Matrix -> external platform.
    pub(super) async fn handle_room_message(
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

        // Route `!bridge` commands (room management, works in any room).
        if body.starts_with("!bridge") {
            return self.handle_command(room_id, sender, body).await;
        }

        let mappings = self.db.find_all_mappings_by_matrix_id(room_id).await?;

        // In rooms with no bridge mappings, route `!` commands to the DM bot
        // command handler (help, rooms, platforms, custom passthrough, etc.).
        if mappings.is_empty() {
            if body.starts_with('!') {
                return self.handle_bot_command(room_id, sender, body).await;
            }
            return Ok(());
        }

        // Room-level trust: an admin invited the bot → the room is bridged.
        // Optionally enforce a minimum power level for message forwarding.
        let min_pl = self.permissions.relay_min_power_level;
        if min_pl > 0 && !sender.starts_with(&self.puppet_user_prefix) {
            let sender_pl = self
                .matrix_client
                .get_user_power_level(room_id, sender)
                .await
                .unwrap_or(0);
            if sender_pl < min_pl {
                debug!(
                    sender,
                    room_id, sender_pl, min_pl, "message forwarding blocked: power level too low"
                );
                return Ok(());
            }
        }

        // Detect edits: m.room.message with m.relates_to.rel_type = "m.replace".
        let is_edit = content
            .get("m.relates_to")
            .and_then(|r| r.get("rel_type"))
            .and_then(|v| v.as_str())
            == Some("m.replace");

        if is_edit {
            return self.handle_edit(room_id, sender, event, &content).await;
        }

        let parsed = match Self::parse_message_content(msgtype, body, &content) {
            Some(c) => c,
            None => {
                debug!(msgtype, "unsupported message type, skipping outbound");
                return Ok(());
            }
        };

        // Resolve media URLs for outbound delivery:
        // 1. Encrypted attachments: download ciphertext → decrypt → re-upload
        //    as plaintext so external platforms can access the media.
        // 2. Convert all mxc:// URIs to HTTP download URLs.
        let message_content = self
            .resolve_outbound_media(
                parsed.content,
                parsed.encrypted_media,
                parsed.file_encryption,
            )
            .await;

        let timestamp = event
            .get("origin_server_ts")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let puppet_record = self
            .db
            .find_puppet_by_matrix_id(sender)
            .await
            .ok()
            .flatten();

        let source_platform = puppet_record
            .as_ref()
            .map(|p| p.platform_id.clone())
            .or_else(|| {
                matrix_bridge_core::platform::puppet_source_platform(sender, &self.puppet_prefix)
            });

        if let Some(ref p) = source_platform {
            debug!(
                sender,
                source_platform = p.as_str(),
                "puppet user detected, will skip source platform"
            );
        }

        for mapping in &mappings {
            if let Some(ref src) = source_platform
                && mapping.platform_id == *src
            {
                debug!(
                    platform = mapping.platform_id,
                    sender, "skipping source platform to prevent loop"
                );
                continue;
            }

            let bridge_sender = if let Some(ref puppet) = puppet_record {
                matrix_bridge_core::message::ExternalUser {
                    platform: puppet.platform_id.clone(),
                    external_id: puppet.external_user_id.clone(),
                    display_name: puppet.display_name.clone(),
                    avatar_url: puppet.avatar_mxc.clone(),
                }
            } else {
                matrix_bridge_core::message::ExternalUser {
                    platform: "matrix".to_string(),
                    external_id: sender.to_string(),
                    display_name: None,
                    avatar_url: None,
                }
            };

            let bridge_msg = matrix_bridge_core::message::BridgeMessage {
                id: event_id.to_string(),
                sender: bridge_sender,
                room: matrix_bridge_core::message::ExternalRoom {
                    platform: mapping.platform_id.clone(),
                    external_id: mapping.external_room_id.clone(),
                    name: None,
                },
                content: message_content.clone(),
                timestamp,
                reply_to: None,
            };

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
                        error = %e,
                        "failed to deliver to webhooks"
                    );
                }
            }
        }
        Ok(())
    }

    /// Resolve media URLs for outbound delivery to external platforms.
    ///
    /// For encrypted attachments: downloads the ciphertext from the homeserver,
    /// decrypts it using the key material from the Matrix `file` object, and
    /// re-uploads as plaintext to get a clean mxc:// URI.
    ///
    /// For all media: converts mxc:// URIs to HTTP download URLs that external
    /// platforms can access directly.
    async fn resolve_outbound_media(
        &self,
        content: MessageContent,
        encrypted_media: bool,
        file_encryption: Option<super::matrix_content::FileEncryption>,
    ) -> MessageContent {
        if !encrypted_media {
            // No encryption — just convert mxc:// to HTTP URL.
            return self.convert_mxc_urls(content);
        }

        // Encrypted media: download → decrypt → re-upload as plaintext.
        let Some(enc) = file_encryption else {
            warn!("encrypted media flag set but no encryption metadata found");
            return self.convert_mxc_urls(content);
        };

        let mxc_uri = match &content {
            MessageContent::Image { url, .. }
            | MessageContent::File { url, .. }
            | MessageContent::Video { url, .. }
            | MessageContent::Audio { url, .. } => url.clone(),
            _ => return content,
        };

        if mxc_uri.is_empty() || !mxc_uri.starts_with("mxc://") {
            warn!(url = mxc_uri, "encrypted media has invalid mxc URI");
            return self.convert_mxc_urls(content);
        }

        // Download ciphertext from homeserver.
        let (ciphertext, _content_type) = match self.matrix_client.download_media(&mxc_uri).await {
            Ok(r) => r,
            Err(e) => {
                warn!(mxc_uri, error = %e, "failed to download encrypted media for decryption");
                return self.convert_mxc_urls(content);
            }
        };

        // Decrypt the attachment.
        let plaintext = match decrypt_attachment(
            &ciphertext,
            &enc.key_b64url,
            &enc.iv_b64,
            Some(&enc.sha256_b64),
        ) {
            Ok(p) => p,
            Err(e) => {
                warn!(mxc_uri, error = %e, "failed to decrypt media attachment");
                return self.convert_mxc_urls(content);
            }
        };

        // Determine content type and filename for re-upload.
        let (content_type, upload_name) = match &content {
            MessageContent::Image {
                mimetype, filename, ..
            } => (mimetype.as_str(), filename.as_deref().unwrap_or("image")),
            MessageContent::File {
                mimetype, filename, ..
            } => (mimetype.as_str(), filename.as_str()),
            MessageContent::Video {
                mimetype, filename, ..
            } => (mimetype.as_str(), filename.as_deref().unwrap_or("video")),
            MessageContent::Audio {
                mimetype, filename, ..
            } => (mimetype.as_str(), filename.as_deref().unwrap_or("audio")),
            _ => ("application/octet-stream", "file"),
        };

        // Re-upload as unencrypted plaintext.
        let new_mxc = match self
            .matrix_client
            .upload_media(plaintext, content_type, upload_name)
            .await
        {
            Ok(uri) => uri,
            Err(e) => {
                warn!(error = %e, "failed to re-upload decrypted media");
                return self.convert_mxc_urls(content);
            }
        };

        debug!(
            old_mxc = mxc_uri,
            new_mxc, "decrypted and re-uploaded encrypted media for outbound"
        );

        // Replace the URL in the content and convert to HTTP.
        let updated = Self::replace_media_url(content, &new_mxc);
        self.convert_mxc_urls(updated)
    }

    /// Convert all mxc:// URLs in a `MessageContent` to HTTP download URLs.
    fn convert_mxc_urls(&self, content: MessageContent) -> MessageContent {
        match content {
            MessageContent::Image {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
            } => MessageContent::Image {
                url: self.mxc_to_http(&url),
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
            },
            MessageContent::File {
                url,
                filename,
                mimetype,
                size,
            } => MessageContent::File {
                url: self.mxc_to_http(&url),
                filename,
                mimetype,
                size,
            },
            MessageContent::Video {
                url,
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
            } => MessageContent::Video {
                url: self.mxc_to_http(&url),
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
            },
            MessageContent::Audio {
                url,
                mimetype,
                filename,
                size,
                duration,
            } => MessageContent::Audio {
                url: self.mxc_to_http(&url),
                mimetype,
                filename,
                size,
                duration,
            },
            other => other,
        }
    }

    /// Convert a single mxc:// URI to an HTTP download URL, or return as-is
    /// if it's already an HTTP URL or not a valid mxc URI.
    fn mxc_to_http(&self, url: &str) -> String {
        if !url.starts_with("mxc://") {
            return url.to_string();
        }
        self.matrix_client
            .mxc_to_download_url(url)
            .unwrap_or_else(|| url.to_string())
    }

    /// Replace the media URL in a `MessageContent`.
    fn replace_media_url(content: MessageContent, new_url: &str) -> MessageContent {
        match content {
            MessageContent::Image {
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                ..
            } => MessageContent::Image {
                url: new_url.to_string(),
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
            },
            MessageContent::File {
                filename,
                mimetype,
                size,
                ..
            } => MessageContent::File {
                url: new_url.to_string(),
                filename,
                mimetype,
                size,
            },
            MessageContent::Video {
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
                ..
            } => MessageContent::Video {
                url: new_url.to_string(),
                caption,
                mimetype,
                filename,
                width,
                height,
                size,
                duration,
            },
            MessageContent::Audio {
                mimetype,
                filename,
                size,
                duration,
                ..
            } => MessageContent::Audio {
                url: new_url.to_string(),
                mimetype,
                filename,
                size,
                duration,
            },
            other => other,
        }
    }

    /// Handle an edit (m.room.message with `m.relates_to.rel_type = "m.replace"`).
    ///
    /// Extracts the new content from `m.new_content`, parses it into a
    /// `MessageContent`, wraps it in `MessageContent::Edit`, and fans out
    /// to all platforms that have a mapping for the original event.
    async fn handle_edit(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
        content: &Value,
    ) -> anyhow::Result<()> {
        let relates_to = content.get("m.relates_to");
        let target_event_id = relates_to
            .and_then(|r| r.get("event_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if target_event_id.is_empty() {
            debug!("edit missing target event_id, skipping");
            return Ok(());
        }

        // Parse the replacement content from m.new_content (preferred) or
        // fall back to the top-level content (some clients only set top-level).
        let new_content_json = content.get("m.new_content").unwrap_or(content);
        let new_msgtype = new_content_json
            .get("msgtype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_body = new_content_json
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let new_parsed = match Self::parse_message_content(new_msgtype, new_body, new_content_json)
        {
            Some(c) => c,
            None => {
                debug!(
                    new_msgtype,
                    "unsupported edit content type, skipping outbound"
                );
                return Ok(());
            }
        };

        let new_content = self
            .resolve_outbound_media(
                new_parsed.content,
                new_parsed.encrypted_media,
                new_parsed.file_encryption,
            )
            .await;

        let msg_mappings = self
            .db
            .find_all_messages_by_matrix_id(target_event_id)
            .await?;
        if msg_mappings.is_empty() {
            debug!(
                target_event_id,
                "no message mapping for edit target, skipping"
            );
            return Ok(());
        }

        let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");

        let source_platform =
            matrix_bridge_core::platform::puppet_source_platform(sender, &self.puppet_prefix);

        let timestamp = event
            .get("origin_server_ts")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        for msg_mapping in &msg_mappings {
            let room_mapping = self
                .db
                .find_room_by_matrix_id(room_id, &msg_mapping.platform_id)
                .await?;
            let Some(room_mapping) = room_mapping else {
                continue;
            };

            let bridge_msg = matrix_bridge_core::message::BridgeMessage {
                id: event_id.to_string(),
                sender: matrix_bridge_core::message::ExternalUser {
                    platform: "matrix".to_string(),
                    external_id: sender.to_string(),
                    display_name: None,
                    avatar_url: None,
                },
                room: matrix_bridge_core::message::ExternalRoom {
                    platform: msg_mapping.platform_id.clone(),
                    external_id: room_mapping.external_room_id,
                    name: None,
                },
                content: MessageContent::Edit {
                    target_id: msg_mapping.external_message_id.clone(),
                    new_content: Box::new(new_content.clone()),
                },
                timestamp,
                reply_to: None,
            };

            if let Err(e) = self
                .deliver_to_webhooks(
                    &msg_mapping.platform_id,
                    &bridge_msg,
                    source_platform.as_deref(),
                )
                .await
            {
                error!(
                    platform = msg_mapping.platform_id,
                    error = %e,
                    "failed to bridge edit"
                );
            }
        }

        Ok(())
    }

    /// Handle an m.reaction event.
    ///
    /// Extracts the target event and emoji from `m.relates_to`, maps the
    /// target event to its external ID, and fans out to all platforms.
    pub(super) async fn handle_reaction(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let content = event.get("content").cloned().unwrap_or_default();
        let relates_to = content.get("m.relates_to");

        let target_event_id = relates_to
            .and_then(|r| r.get("event_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let emoji = relates_to
            .and_then(|r| r.get("key"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if target_event_id.is_empty() || emoji.is_empty() {
            debug!("reaction missing event_id or key, skipping");
            return Ok(());
        }

        let msg_mappings = self
            .db
            .find_all_messages_by_matrix_id(target_event_id)
            .await?;
        if msg_mappings.is_empty() {
            debug!(
                target_event_id,
                "no message mapping for reaction target, skipping"
            );
            return Ok(());
        }

        let source_platform =
            matrix_bridge_core::platform::puppet_source_platform(sender, &self.puppet_prefix);

        let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");

        let timestamp = event
            .get("origin_server_ts")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Reactions are fire-and-forget — no message mapping is stored
        // because platforms do not echo reactions as bridged messages.
        for msg_mapping in &msg_mappings {
            let room_mapping = self
                .db
                .find_room_by_matrix_id(room_id, &msg_mapping.platform_id)
                .await?;
            let Some(room_mapping) = room_mapping else {
                continue;
            };

            let bridge_msg = matrix_bridge_core::message::BridgeMessage {
                id: event_id.to_string(),
                sender: matrix_bridge_core::message::ExternalUser {
                    platform: "matrix".to_string(),
                    external_id: sender.to_string(),
                    display_name: None,
                    avatar_url: None,
                },
                room: matrix_bridge_core::message::ExternalRoom {
                    platform: msg_mapping.platform_id.clone(),
                    external_id: room_mapping.external_room_id,
                    name: None,
                },
                content: MessageContent::Reaction {
                    target_id: msg_mapping.external_message_id.clone(),
                    emoji: emoji.to_string(),
                },
                timestamp,
                reply_to: None,
            };

            if let Err(e) = self
                .deliver_to_webhooks(
                    &msg_mapping.platform_id,
                    &bridge_msg,
                    source_platform.as_deref(),
                )
                .await
            {
                error!(
                    platform = msg_mapping.platform_id,
                    error = %e,
                    "failed to bridge reaction"
                );
            }
        }

        Ok(())
    }

    /// Handle an m.room.redaction event.
    ///
    /// Fans out the redaction to every platform that has a mapping for the
    /// redacted Matrix event, so multi-platform rooms get consistent deletions.
    pub(super) async fn handle_redaction(
        &self,
        room_id: &str,
        sender: &str,
        event: &Value,
    ) -> anyhow::Result<()> {
        let redacts = event.get("redacts").and_then(|v| v.as_str()).unwrap_or("");
        if redacts.is_empty() {
            return Ok(());
        }

        let msg_mappings = self.db.find_all_messages_by_matrix_id(redacts).await?;
        if msg_mappings.is_empty() {
            return Ok(());
        }

        let source_platform =
            matrix_bridge_core::platform::puppet_source_platform(sender, &self.puppet_prefix);

        for msg_mapping in &msg_mappings {
            let room_mapping = self
                .db
                .find_room_by_matrix_id(room_id, &msg_mapping.platform_id)
                .await?;
            let Some(room_mapping) = room_mapping else {
                continue;
            };

            let timestamp = event
                .get("origin_server_ts")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let bridge_msg = matrix_bridge_core::message::BridgeMessage {
                id: redacts.to_string(),
                sender: matrix_bridge_core::message::ExternalUser {
                    platform: "matrix".to_string(),
                    external_id: sender.to_string(),
                    display_name: None,
                    avatar_url: None,
                },
                room: matrix_bridge_core::message::ExternalRoom {
                    platform: msg_mapping.platform_id.clone(),
                    external_id: room_mapping.external_room_id,
                    name: None,
                },
                content: matrix_bridge_core::message::MessageContent::Redaction {
                    target_id: msg_mapping.external_message_id.clone(),
                },
                timestamp,
                reply_to: None,
            };

            if let Err(e) = self
                .deliver_to_webhooks(
                    &msg_mapping.platform_id,
                    &bridge_msg,
                    source_platform.as_deref(),
                )
                .await
            {
                error!(
                    platform = msg_mapping.platform_id,
                    error = %e,
                    "failed to bridge redaction"
                );
            }
        }

        Ok(())
    }
}
