use serde::{Deserialize, Serialize};

/// Platform-agnostic message content that can flow in either direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text {
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        formatted_body: Option<String>,
    },
    Image {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        mimetype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    File {
        url: String,
        filename: String,
        mimetype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    Video {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
        mimetype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<u64>,
    },
    Audio {
        url: String,
        mimetype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
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
        target_id: String,
        emoji: String,
    },
    Redaction {
        target_id: String,
    },
    Edit {
        target_id: String,
        new_content: Box<MessageContent>,
    },
}

/// Represents a user on an external platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUser {
    pub platform: String,
    pub external_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// Represents a channel/chat/room on an external platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRoom {
    pub platform: String,
    pub external_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A normalized bridge message that flows between Matrix and external platforms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeMessage {
    pub id: String,
    pub sender: ExternalUser,
    pub room: ExternalRoom,
    pub content: MessageContent,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}
