use matrix_bridge_core::message::MessageContent;
use serde::{Deserialize, Serialize};

/// Request body for sending a message from an external platform to Matrix.
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// Platform identifier (e.g., "telegram", "slack", "my_app").
    pub platform: String,
    /// External room/channel ID on the platform.
    pub room_id: String,
    /// Sender information.
    pub sender: SenderInfo,
    /// Message content.
    pub content: ContentPayload,
    /// Optional: external message ID for deduplication.
    #[serde(default)]
    pub external_message_id: Option<String>,
    /// Optional: reply to an external message ID.
    #[serde(default)]
    pub reply_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SenderInfo {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPayload {
    Text {
        body: String,
        #[serde(default)]
        html: Option<String>,
    },
    Image {
        /// mxc:// URI or external URL.
        url: String,
        #[serde(default)]
        caption: Option<String>,
        #[serde(default = "default_image_mime")]
        mimetype: String,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        size: Option<u64>,
    },
    File {
        /// mxc:// URI or external URL.
        url: String,
        filename: String,
        #[serde(default = "default_file_mime")]
        mimetype: String,
        #[serde(default)]
        size: Option<u64>,
    },
    Video {
        /// mxc:// URI or external URL.
        url: String,
        #[serde(default)]
        caption: Option<String>,
        #[serde(default = "default_video_mime")]
        mimetype: String,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        size: Option<u64>,
        #[serde(default)]
        duration: Option<u64>,
    },
    Audio {
        /// mxc:// URI or external URL.
        url: String,
        #[serde(default = "default_audio_mime")]
        mimetype: String,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        size: Option<u64>,
        #[serde(default)]
        duration: Option<u64>,
    },
    Location {
        latitude: f64,
        longitude: f64,
    },
    Notice {
        body: String,
    },
    Emote {
        body: String,
    },
    Reaction {
        /// External message ID of the message being reacted to.
        target_id: String,
        emoji: String,
    },
    Redaction {
        /// External message ID of the message being redacted.
        target_id: String,
    },
    Edit {
        /// External message ID of the message being edited.
        target_id: String,
        /// New content after editing.
        new_content: Box<ContentPayload>,
    },
}

fn default_image_mime() -> String {
    "image/png".to_string()
}
fn default_file_mime() -> String {
    "application/octet-stream".to_string()
}
fn default_video_mime() -> String {
    "video/mp4".to_string()
}
fn default_audio_mime() -> String {
    "audio/ogg".to_string()
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub event_id: String,
    pub message_id: String,
}

/// Request body for creating a room mapping.
///
/// When `matrix_room_id` is omitted, the bridge automatically creates a new
/// Matrix room and uses its ID for the mapping.
#[derive(Debug, Deserialize)]
pub struct CreateRoomMappingRequest {
    pub platform: String,
    pub external_room_id: String,
    /// If `None`, the bridge auto-creates a Matrix room.
    pub matrix_room_id: Option<String>,
    /// Optional room name used when auto-creating (ignored if `matrix_room_id`
    /// is provided).
    pub room_name: Option<String>,
    /// Extra Matrix user IDs to invite when auto-creating a room.
    /// Only effective when `allow_api_invite = true` in server config.
    #[serde(default)]
    pub invite: Vec<String>,
}

/// Request body for registering a webhook.
#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub platform: String,
    pub url: String,
    #[serde(default = "default_events")]
    pub events: String,
    /// Allowlist of *non-matrix* source platforms whose messages are forwarded.
    /// Messages from Matrix users are always forwarded (bridge core functionality).
    /// - Empty / omitted = only forward Matrix user messages (default).
    /// - `["*"]` or `"*"` = forward all sources (Matrix + other platforms).
    /// - `["telegram","discord"]` = forward Matrix + those platforms only.
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub forward_sources: Vec<String>,
    /// Capabilities this integration supports.
    /// e.g. `["message","image","reaction","edit","redaction","command"]`
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub capabilities: Vec<String>,
    /// Matrix user ID of the integration operator. Auto-invited into portal
    /// rooms created for this platform. e.g. `"@admin:example.com"`
    #[serde(default)]
    pub owner: String,
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<String>, E> {
            if v.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(v.split(',').map(|s| s.trim().to_string()).collect())
            }
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
            let mut v = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                v.push(item);
            }
            Ok(v)
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

fn default_events() -> String {
    "message".to_string()
}

/// Convert a ContentPayload (API input) to MessageContent (internal).
pub(crate) fn convert_content(payload: ContentPayload) -> MessageContent {
    match payload {
        ContentPayload::Text { body, html } => MessageContent::Text {
            body,
            formatted_body: html,
        },
        ContentPayload::Image {
            url,
            caption,
            mimetype,
            filename,
            width,
            height,
            size,
        } => MessageContent::Image {
            url,
            caption,
            mimetype,
            filename,
            width,
            height,
            size,
        },
        ContentPayload::File {
            url,
            filename,
            mimetype,
            size,
        } => MessageContent::File {
            url,
            filename,
            mimetype,
            size,
        },
        ContentPayload::Video {
            url,
            caption,
            mimetype,
            filename,
            width,
            height,
            size,
            duration,
        } => MessageContent::Video {
            url,
            caption,
            mimetype,
            filename,
            width,
            height,
            size,
            duration,
        },
        ContentPayload::Audio {
            url,
            mimetype,
            filename,
            size,
            duration,
        } => MessageContent::Audio {
            url,
            mimetype,
            filename,
            size,
            duration,
        },
        ContentPayload::Location {
            latitude,
            longitude,
        } => MessageContent::Location {
            latitude,
            longitude,
        },
        ContentPayload::Notice { body } => MessageContent::Notice { body },
        ContentPayload::Emote { body } => MessageContent::Emote { body },
        ContentPayload::Reaction { target_id, emoji } => {
            MessageContent::Reaction { target_id, emoji }
        }
        ContentPayload::Redaction { target_id } => MessageContent::Redaction { target_id },
        ContentPayload::Edit {
            target_id,
            new_content,
        } => MessageContent::Edit {
            target_id,
            new_content: Box::new(convert_content(*new_content)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Serde deserialization tests --

    #[test]
    fn deserialize_send_message_text() {
        let json = serde_json::json!({
            "platform": "telegram",
            "room_id": "chat_123",
            "sender": { "id": "user_1", "display_name": "Alice" },
            "content": { "type": "text", "body": "Hello!" }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.platform, "telegram");
        assert_eq!(req.sender.display_name.as_deref(), Some("Alice"));
        assert!(
            matches!(req.content, ContentPayload::Text { body, html } if body == "Hello!" && html.is_none())
        );
    }

    #[test]
    fn deserialize_send_message_text_with_html() {
        let json = serde_json::json!({
            "platform": "slack", "room_id": "C1234",
            "sender": { "id": "U1" },
            "content": { "type": "text", "body": "Hello", "html": "<b>Hello</b>" },
            "external_message_id": "msg_42", "reply_to": "msg_41"
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.external_message_id.as_deref(), Some("msg_42"));
        assert_eq!(req.reply_to.as_deref(), Some("msg_41"));
    }

    #[test]
    fn deserialize_send_message_image() {
        let json = serde_json::json!({
            "platform": "discord", "room_id": "ch1", "sender": { "id": "u1" },
            "content": { "type": "image", "url": "mxc://example.com/abc" }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert!(
            matches!(req.content, ContentPayload::Image { url, mimetype, .. } if url == "mxc://example.com/abc" && mimetype == "image/png")
        );
    }

    #[test]
    fn deserialize_send_message_file() {
        let json = serde_json::json!({
            "platform": "telegram", "room_id": "ch1", "sender": { "id": "u1" },
            "content": { "type": "file", "url": "https://f.example.com/doc.pdf", "filename": "doc.pdf" }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert!(
            matches!(req.content, ContentPayload::File { filename, mimetype, .. } if filename == "doc.pdf" && mimetype == "application/octet-stream")
        );
    }

    #[test]
    fn deserialize_send_message_location() {
        let json = serde_json::json!({
            "platform": "telegram", "room_id": "ch1", "sender": { "id": "u1" },
            "content": { "type": "location", "latitude": 48.8566, "longitude": 2.3522 }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert!(
            matches!(req.content, ContentPayload::Location { latitude, longitude } if (latitude - 48.8566).abs() < 0.001 && (longitude - 2.3522).abs() < 0.001)
        );
    }

    #[test]
    fn deserialize_send_message_reaction() {
        let json = serde_json::json!({
            "platform": "slack", "room_id": "ch1", "sender": { "id": "u1" },
            "content": { "type": "reaction", "target_id": "msg_100", "emoji": "thumbsup" }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert!(
            matches!(req.content, ContentPayload::Reaction { target_id, emoji } if target_id == "msg_100" && emoji == "thumbsup")
        );
    }

    #[test]
    fn deserialize_send_message_edit() {
        let json = serde_json::json!({
            "platform": "telegram", "room_id": "ch1", "sender": { "id": "u1" },
            "content": { "type": "edit", "target_id": "msg_50", "new_content": { "type": "text", "body": "corrected" } }
        });
        let req: SendMessageRequest = serde_json::from_value(json).unwrap();
        assert!(
            matches!(req.content, ContentPayload::Edit { target_id, .. } if target_id == "msg_50")
        );
    }

    #[test]
    fn deserialize_create_room_mapping_minimal() {
        let json = serde_json::json!({ "platform": "telegram", "external_room_id": "chat_999" });
        let req: CreateRoomMappingRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.platform, "telegram");
        assert!(req.matrix_room_id.is_none());
        assert!(req.invite.is_empty());
    }

    #[test]
    fn deserialize_create_room_mapping_full() {
        let json = serde_json::json!({
            "platform": "discord", "external_room_id": "ch_42",
            "matrix_room_id": "!abc:example.com", "room_name": "Test Room",
            "invite": ["@admin:example.com"]
        });
        let req: CreateRoomMappingRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.matrix_room_id.as_deref(), Some("!abc:example.com"));
        assert_eq!(req.invite, vec!["@admin:example.com"]);
    }

    #[test]
    fn deserialize_create_webhook_defaults() {
        let json = serde_json::json!({ "platform": "telegram", "url": "https://example.com/hook" });
        let req: CreateWebhookRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.events, "message");
        assert!(req.forward_sources.is_empty());
    }

    #[test]
    fn deserialize_create_webhook_forward_sources_as_string() {
        let json = serde_json::json!({ "platform": "tg", "url": "https://e.com/h", "forward_sources": "matrix,discord" });
        let req: CreateWebhookRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.forward_sources, vec!["matrix", "discord"]);
    }

    #[test]
    fn deserialize_create_webhook_forward_sources_as_array() {
        let json = serde_json::json!({ "platform": "tg", "url": "https://e.com/h", "forward_sources": ["matrix", "discord"] });
        let req: CreateWebhookRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.forward_sources, vec!["matrix", "discord"]);
    }

    // -- convert_content tests --

    #[test]
    fn convert_content_text() {
        let result = convert_content(ContentPayload::Text {
            body: "hello".into(),
            html: Some("<b>hello</b>".into()),
        });
        assert!(
            matches!(result, MessageContent::Text { body, formatted_body } if body == "hello" && formatted_body.as_deref() == Some("<b>hello</b>"))
        );
    }

    #[test]
    fn convert_content_image() {
        let result = convert_content(ContentPayload::Image {
            url: "mxc://e/i".into(),
            caption: Some("photo".into()),
            mimetype: "image/jpeg".into(),
            filename: None,
            width: None,
            height: None,
            size: None,
        });
        assert!(
            matches!(result, MessageContent::Image { url, caption, mimetype, .. } if url == "mxc://e/i" && caption.as_deref() == Some("photo") && mimetype == "image/jpeg")
        );
    }

    #[test]
    fn convert_content_notice() {
        let result = convert_content(ContentPayload::Notice { body: "sys".into() });
        assert!(matches!(result, MessageContent::Notice { body } if body == "sys"));
    }

    #[test]
    fn convert_content_emote() {
        let result = convert_content(ContentPayload::Emote {
            body: "dances".into(),
        });
        assert!(matches!(result, MessageContent::Emote { body } if body == "dances"));
    }

    #[test]
    fn convert_content_reaction() {
        let result = convert_content(ContentPayload::Reaction {
            target_id: "m1".into(),
            emoji: "heart".into(),
        });
        assert!(
            matches!(result, MessageContent::Reaction { target_id, emoji } if target_id == "m1" && emoji == "heart")
        );
    }

    #[test]
    fn convert_content_redaction() {
        let result = convert_content(ContentPayload::Redaction {
            target_id: "m2".into(),
        });
        assert!(matches!(result, MessageContent::Redaction { target_id } if target_id == "m2"));
    }

    #[test]
    fn convert_content_edit_recursive() {
        let result = convert_content(ContentPayload::Edit {
            target_id: "m3".into(),
            new_content: Box::new(ContentPayload::Text {
                body: "fixed".into(),
                html: None,
            }),
        });
        assert!(matches!(result, MessageContent::Edit { target_id, .. } if target_id == "m3"));
    }

    #[test]
    fn convert_content_location() {
        let result = convert_content(ContentPayload::Location {
            latitude: 40.7,
            longitude: -74.0,
        });
        assert!(
            matches!(result, MessageContent::Location { latitude, longitude } if (latitude - 40.7).abs() < 0.01 && (longitude - (-74.0)).abs() < 0.01)
        );
    }

    #[test]
    fn convert_content_video() {
        let result = convert_content(ContentPayload::Video {
            url: "mxc://e/v".into(),
            caption: None,
            mimetype: "video/mp4".into(),
            filename: None,
            width: None,
            height: None,
            size: None,
            duration: None,
        });
        assert!(
            matches!(result, MessageContent::Video { url, mimetype, .. } if url == "mxc://e/v" && mimetype == "video/mp4")
        );
    }

    #[test]
    fn convert_content_audio() {
        let result = convert_content(ContentPayload::Audio {
            url: "mxc://e/a".into(),
            mimetype: "audio/ogg".into(),
            filename: None,
            size: None,
            duration: None,
        });
        assert!(
            matches!(result, MessageContent::Audio { url, mimetype, .. } if url == "mxc://e/a" && mimetype == "audio/ogg")
        );
    }

    #[test]
    fn convert_content_file() {
        let result = convert_content(ContentPayload::File {
            url: "mxc://e/f".into(),
            filename: "data.csv".into(),
            mimetype: "text/csv".into(),
            size: None,
        });
        assert!(
            matches!(result, MessageContent::File { url, filename, mimetype, .. } if url == "mxc://e/f" && filename == "data.csv" && mimetype == "text/csv")
        );
    }
}
