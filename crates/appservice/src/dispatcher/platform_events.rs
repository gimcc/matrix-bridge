use serde_json::Value;
use tracing::{debug, info, warn};

use matrix_bridge_core::error::BridgeError;
use matrix_bridge_core::message::{BridgeMessage, MessageContent};
use matrix_bridge_core::platform;

use super::Dispatcher;
use super::attachment_crypto::EncryptedAttachment;

impl Dispatcher {
    /// Handle an incoming message via the HTTP bridge API (external -> Matrix).
    pub async fn handle_incoming_http(
        &self,
        message: BridgeMessage,
    ) -> Result<String, BridgeError> {
        let platform_id = &message.room.platform;
        let external_room_id = platform::sanitize_external_id(&message.room.external_id);
        let external_sender_id = platform::sanitize_external_id(&message.sender.external_id);

        let room_mapping = self
            .db
            .find_room_by_external_id(platform_id, &external_room_id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        let room_mapping = match room_mapping {
            Some(m) => m,
            None => {
                let room_name = message
                    .room
                    .name
                    .as_deref()
                    .unwrap_or(&external_room_id);

                info!(
                    platform = platform_id,
                    external_room = %external_room_id,
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

                if self.encryption_default
                    && let Some(pool) = &self.crypto_pool
                    && let Ok(ruma_room_id) =
                        <&ruma::RoomId>::try_from(matrix_room_id.as_str())
                    && let Err(e) = pool.bot().set_room_encrypted(ruma_room_id).await
                {
                    warn!(
                        %matrix_room_id,
                        error = %e,
                        "failed to mark portal room as encrypted in crypto store"
                    );
                }

                let id = match self
                    .db
                    .create_room_mapping(&matrix_room_id, platform_id, &external_room_id)
                    .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        // Best-effort cleanup of auto-created room on DB failure.
                        warn!(
                            %matrix_room_id,
                            error = %e,
                            "portal room DB mapping failed — attempting cleanup"
                        );
                        let _ = self
                            .matrix_client
                            .leave_room_as_bot(&matrix_room_id)
                            .await;
                        return Err(BridgeError::Store(e.to_string()));
                    }
                };

                info!(
                    matrix_room_id,
                    platform = platform_id,
                    external_room = %external_room_id,
                    "portal room created and mapped"
                );

                matrix_bridge_store::RoomMapping {
                    id,
                    matrix_room_id,
                    platform_id: platform_id.to_string(),
                    external_room_id: external_room_id.clone(),
                }
            }
        };

        let localpart = platform::puppet_localpart(
            &self.puppet_prefix,
            platform_id,
            &external_sender_id,
        );
        let puppet_user_id = self
            .puppet_manager
            .ensure_puppet_direct(
                &localpart,
                platform_id,
                &external_sender_id,
                message.sender.display_name.as_deref(),
                message.sender.avatar_url.as_deref(),
            )
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

        self.ensure_room_access(&room_mapping.matrix_room_id, &puppet_user_id)
            .await?;

        // Check if the target room uses encryption so we can encrypt attachments.
        let room_encrypted = self.is_room_encrypted(&room_mapping.matrix_room_id).await;

        // Auto-download external media (http/https) and reupload to Matrix media repo.
        // When the room is encrypted, files are AES-256-CTR encrypted before uploading.
        let reupload = self
            .reupload_external_media(message.content.clone(), room_encrypted)
            .await;
        let reuploaded_message = BridgeMessage {
            content: reupload.content,
            ..message.clone()
        };
        let (content, txn_id) =
            Self::to_matrix_content(&reuploaded_message, reupload.encrypted_file.as_ref());
        let event_id = self
            .send_to_matrix(
                &room_mapping.matrix_room_id,
                &content,
                &puppet_user_id,
                &txn_id,
                Some(room_encrypted),
            )
            .await
            .map_err(|e| BridgeError::Matrix(e.to_string()))?;

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

    /// Check if a room is encrypted via the crypto pool.
    async fn is_room_encrypted(&self, room_id: &str) -> bool {
        let Some(pool) = &self.crypto_pool else {
            return false;
        };
        let Ok(ruma_room_id) = <&ruma::RoomId>::try_from(room_id) else {
            return false;
        };
        pool.bot()
            .is_room_encrypted(ruma_room_id, &self.matrix_client)
            .await
    }

    /// Ensure a puppet user can access a Matrix room.
    pub(super) async fn ensure_room_access(
        &self,
        room_id: &str,
        puppet_user_id: &str,
    ) -> Result<(), BridgeError> {
        let cache_key = (room_id.to_string(), puppet_user_id.to_string());
        if self.room_membership.contains(&cache_key) {
            return Ok(());
        }

        self.matrix_client
            .join_room(room_id, &self.bot_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("bridge bot join failed: {e}")))?;

        self.matrix_client
            .invite_user(room_id, puppet_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("invite puppet failed: {e}")))?;

        self.matrix_client
            .join_room(room_id, puppet_user_id)
            .await
            .map_err(|e| BridgeError::Matrix(format!("puppet join failed: {e}")))?;

        self.room_membership.insert(cache_key);
        Ok(())
    }

    /// Convert a BridgeMessage to Matrix message content JSON.
    ///
    /// When `encrypted_file` is `Some`, media content uses a `"file"` object
    /// (containing encryption key, IV, and hash) instead of a plain `"url"` field.
    pub(super) fn to_matrix_content(
        message: &BridgeMessage,
        encrypted_file: Option<&EncryptedAttachment>,
    ) -> (Value, String) {
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
                    let safe_html = ammonia::Builder::default()
                        .link_rel(Some("noopener noreferrer"))
                        .clean(html)
                        .to_string();
                    c["format"] = "org.matrix.custom.html".into();
                    c["formatted_body"] = safe_html.into();
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
            } => {
                let mut c = serde_json::json!({
                    "msgtype": "m.image",
                    "body": caption.as_deref().unwrap_or("image"),
                    "info": { "mimetype": mimetype },
                });
                apply_media_url(&mut c, url, encrypted_file);
                c
            }
            MessageContent::File {
                url,
                filename,
                mimetype,
            } => {
                let mut c = serde_json::json!({
                    "msgtype": "m.file",
                    "body": filename,
                    "info": { "mimetype": mimetype },
                });
                apply_media_url(&mut c, url, encrypted_file);
                c
            }
            MessageContent::Video {
                url,
                caption,
                mimetype,
            } => {
                let mut c = serde_json::json!({
                    "msgtype": "m.video",
                    "body": caption.as_deref().unwrap_or("video"),
                    "info": { "mimetype": mimetype },
                });
                apply_media_url(&mut c, url, encrypted_file);
                c
            }
            MessageContent::Audio { url, mimetype } => {
                let mut c = serde_json::json!({
                    "msgtype": "m.audio",
                    "body": "audio",
                    "info": { "mimetype": mimetype },
                });
                apply_media_url(&mut c, url, encrypted_file);
                c
            }
            MessageContent::Location {
                latitude,
                longitude,
            } => serde_json::json!({
                "msgtype": "m.location",
                "body": format!("Location: {latitude}, {longitude}"),
                "geo_uri": format!("geo:{latitude},{longitude}"),
            }),
            MessageContent::Reaction { emoji, .. } => serde_json::json!({
                "msgtype": "m.text",
                "body": emoji,
            }),
            MessageContent::Redaction { .. } => serde_json::json!({
                "msgtype": "m.notice",
                "body": "[message deleted]",
            }),
            MessageContent::Edit { new_content, .. } => {
                // Send the edited content as a new message. Do NOT propagate
                // the outer encrypted_file — it belongs to the original media,
                // not to whatever the new_content may reference.
                return Self::to_matrix_content(
                    &BridgeMessage {
                        id: String::new(),
                        sender: message.sender.clone(),
                        room: message.room.clone(),
                        content: *new_content.clone(),
                        timestamp: message.timestamp,
                        reply_to: None,
                    },
                    None,
                );
            }
        };

        (content, txn_id)
    }
}

/// Set the `url` or `file` field on a media message JSON object.
///
/// If encryption metadata is available, uses the Matrix `file` object format
/// (AES-256-CTR key, IV, SHA-256 hash). Otherwise, uses a plain `url` field.
fn apply_media_url(
    content: &mut Value,
    url: &str,
    encrypted_file: Option<&EncryptedAttachment>,
) {
    if let Some(enc) = encrypted_file {
        content["file"] = enc.to_file_json();
    } else {
        content["url"] = Value::String(url.to_string());
    }
}
