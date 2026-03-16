use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info};

use matrix_bridge_core::message::{BridgeMessage, ExternalRoom, ExternalUser, MessageContent};

use crate::server::AppState;

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
    },
    File {
        /// mxc:// URI or external URL.
        url: String,
        filename: String,
        #[serde(default = "default_file_mime")]
        mimetype: String,
    },
    Video {
        /// mxc:// URI or external URL.
        url: String,
        #[serde(default)]
        caption: Option<String>,
        #[serde(default = "default_video_mime")]
        mimetype: String,
    },
    Audio {
        /// mxc:// URI or external URL.
        url: String,
        #[serde(default = "default_audio_mime")]
        mimetype: String,
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
#[derive(Debug, Deserialize)]
pub struct CreateRoomMappingRequest {
    pub platform: String,
    pub external_room_id: String,
    pub matrix_room_id: String,
}

/// Request body for registering a webhook.
#[derive(Debug, Deserialize)]
pub struct CreateWebhookRequest {
    pub platform: String,
    pub url: String,
    #[serde(default = "default_events")]
    pub events: String,
    /// Platform IDs whose messages should NOT be forwarded to this webhook.
    /// Accepts either a JSON array `["telegram","discord"]` or a
    /// comma-separated string `"telegram,discord"`.
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub exclude_sources: Vec<String>,
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

/// Build the bridge API router.
/// These routes are for external platform services to interact with the bridge.
pub fn build_bridge_api_router() -> Router<Arc<AppState>> {
    Router::new()
        // Message API
        .route("/api/v1/message", post(handle_send_message))
        // Media upload API
        .route("/api/v1/upload", post(handle_upload))
        // Room mapping API
        .route("/api/v1/rooms", post(handle_create_room_mapping))
        .route("/api/v1/rooms", get(handle_list_room_mappings))
        .route("/api/v1/rooms/{id}", delete(handle_delete_room_mapping))
        // Webhook API
        .route("/api/v1/webhooks", post(handle_create_webhook))
        .route("/api/v1/webhooks", get(handle_list_webhooks))
        .route("/api/v1/webhooks/{id}", delete(handle_delete_webhook))
}

/// Convert a ContentPayload (API input) to MessageContent (internal).
fn convert_content(payload: ContentPayload) -> MessageContent {
    match payload {
        ContentPayload::Text { body, html } => MessageContent::Text {
            body,
            formatted_body: html,
        },
        ContentPayload::Image {
            url,
            caption,
            mimetype,
        } => MessageContent::Image {
            url,
            caption,
            mimetype,
        },
        ContentPayload::File {
            url,
            filename,
            mimetype,
        } => MessageContent::File {
            url,
            filename,
            mimetype,
        },
        ContentPayload::Video {
            url,
            caption,
            mimetype,
        } => MessageContent::Video {
            url,
            caption,
            mimetype,
        },
        ContentPayload::Audio { url, mimetype } => MessageContent::Audio { url, mimetype },
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

/// POST /api/v1/message
///
/// External platform sends a message to be bridged into Matrix.
///
/// Example:
/// ```json
/// {
///   "platform": "telegram",
///   "room_id": "chat_12345",
///   "sender": {
///     "id": "user_789",
///     "display_name": "Alice",
///     "avatar_url": "https://example.com/alice.jpg"
///   },
///   "content": {
///     "type": "text",
///     "body": "Hello from Telegram!"
///   }
/// }
/// ```
async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    let msg_id = req
        .external_message_id
        .unwrap_or_else(|| ulid::Ulid::new().to_string());

    let content = convert_content(req.content);

    let bridge_msg = BridgeMessage {
        id: msg_id.clone(),
        sender: ExternalUser {
            platform: req.platform.clone(),
            external_id: req.sender.id,
            display_name: req.sender.display_name,
            avatar_url: req.sender.avatar_url,
        },
        room: ExternalRoom {
            platform: req.platform.clone(),
            external_id: req.room_id,
            name: None,
        },
        content,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        reply_to: req.reply_to,
    };

    let dispatcher = state.dispatcher.lock().await;
    match dispatcher.handle_incoming_http(bridge_msg).await {
        Ok(event_id) => (
            StatusCode::OK,
            Json(json!({
                "event_id": event_id,
                "message_id": msg_id,
            })),
        ),
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error!(platform = req.platform, %status, "bridge api send failed: {e}");
            (status, Json(json!({ "error": e.to_string() })))
        }
    }
}

/// POST /api/v1/rooms
async fn handle_create_room_mapping(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRoomMappingRequest>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.lock().await;
    match dispatcher
        .db()
        .create_room_mapping(&req.matrix_room_id, &req.platform, &req.external_room_id)
        .await
    {
        Ok(id) => {
            info!(
                platform = req.platform,
                external = req.external_room_id,
                matrix = req.matrix_room_id,
                "room mapping created via API"
            );
            (StatusCode::CREATED, Json(json!({ "id": id })))
        }
        Err(e) => {
            error!("create room mapping failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/rooms?platform=xxx
async fn handle_list_room_mappings(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let platform = params.get("platform").map(|s| s.as_str()).unwrap_or("");
    let dispatcher = state.dispatcher.lock().await;

    if platform.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "platform query parameter required" })),
        );
    }

    match dispatcher.db().list_room_mappings(platform).await {
        Ok(mappings) => (StatusCode::OK, Json(json!({ "rooms": mappings }))),
        Err(e) => {
            error!("list room mappings failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// DELETE /api/v1/rooms/{id}
async fn handle_delete_room_mapping(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.lock().await;
    match dispatcher.db().delete_room_mapping(id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            error!("delete room mapping failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// POST /api/v1/webhooks
async fn handle_create_webhook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWebhookRequest>,
) -> impl IntoResponse {
    let exclude_sources = req.exclude_sources.join(",");
    let dispatcher = state.dispatcher.lock().await;
    match dispatcher
        .db()
        .create_webhook(&req.platform, &req.url, &req.events, &exclude_sources)
        .await
    {
        Ok(id) => {
            info!(
                platform = req.platform,
                url = req.url,
                exclude_sources,
                "webhook registered via API"
            );
            (StatusCode::CREATED, Json(json!({ "id": id })))
        }
        Err(e) => {
            error!("create webhook failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/webhooks?platform=xxx
async fn handle_list_webhooks(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.lock().await;
    let platform = params.get("platform").map(|s| s.as_str());

    let result = if let Some(p) = platform {
        dispatcher.db().list_webhooks(p).await
    } else {
        dispatcher.db().list_all_webhooks().await
    };

    match result {
        Ok(webhooks) => (StatusCode::OK, Json(json!({ "webhooks": webhooks }))),
        Err(e) => {
            error!("list webhooks failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// DELETE /api/v1/webhooks/{id}
async fn handle_delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.lock().await;
    match dispatcher.db().delete_webhook(id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            error!("delete webhook failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// POST /api/v1/upload
///
/// Upload a file to the Matrix media repository.
/// Accepts multipart/form-data with a single `file` field.
/// Returns the `mxc://` content URI that can be used in message content.
///
/// Example (curl):
/// ```bash
/// curl -X POST http://bridge:29320/api/v1/upload \
///   -F "file=@photo.jpg;type=image/jpeg"
/// ```
///
/// Response:
/// ```json
/// { "content_uri": "mxc://example.com/abc123", "filename": "photo.jpg", "size": 12345 }
/// ```
async fn handle_upload(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Extract the first file field from the multipart form.
    let field = match multipart.next_field().await {
        Ok(Some(f)) => f,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "no file field in multipart body" })),
            );
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("multipart parse error: {e}") })),
            );
        }
    };

    let filename = field
        .file_name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "upload".to_string());

    let content_type = field
        .content_type()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let data = match field.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("failed to read file: {e}") })),
            );
        }
    };

    let size = data.len();

    let dispatcher = state.dispatcher.lock().await;
    match dispatcher
        .matrix_client()
        .upload_media(data, &content_type, &filename)
        .await
    {
        Ok(mxc_uri) => {
            info!(
                filename,
                size,
                content_uri = mxc_uri,
                "file uploaded via API"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "content_uri": mxc_uri,
                    "filename": filename,
                    "size": size,
                })),
            )
        }
        Err(e) => {
            error!("media upload failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}
