use std::sync::Arc;

use axum::{
    Json,
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tracing::{debug, error};

use matrix_bridge_core::message::{BridgeMessage, ExternalRoom, ExternalUser};

use crate::server::AppState;

use super::{SendMessageRequest, convert_content};

/// Maximum upload file size (200 MB).
const MAX_UPLOAD_SIZE: usize = 200 * 1024 * 1024;

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
pub(super) async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // Input length validation
    if req.platform.len() > 64 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "platform exceeds 64 characters" })));
    }
    if req.room_id.len() > 255 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "room_id exceeds 255 characters" })));
    }
    if req.sender.id.len() > 255 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "sender.id exceeds 255 characters" })));
    }
    if let Some(ref name) = req.sender.display_name {
        if name.len() > 255 {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "display_name exceeds 255 characters" })));
        }
    }
    if let Some(ref mid) = req.external_message_id {
        if mid.len() > 255 {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "external_message_id exceeds 255 characters" })));
        }
    }
    if let Some(ref rt) = req.reply_to {
        if rt.len() > 255 {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "reply_to exceeds 255 characters" })));
        }
    }
    // Check body length for text-based content
    match &req.content {
        super::ContentPayload::Text { body, .. } | super::ContentPayload::Notice { body } | super::ContentPayload::Emote { body } => {
            if body.len() > 65536 {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": "body exceeds 64KB" })));
            }
        }
        _ => {}
    }
    if let super::ContentPayload::Reaction { ref emoji, .. } = req.content {
        if emoji.chars().count() > 64 {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "emoji exceeds 64 characters" })));
        }
    }
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

    let dispatcher = state.dispatcher.read().await;
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
            error!(platform = req.platform, %status, error = %e, "bridge api send failed");
            let error_msg = match &e {
                matrix_bridge_core::error::BridgeError::Validation(msg) => msg.clone(),
                matrix_bridge_core::error::BridgeError::NotFound(msg) => msg.clone(),
                _ => "internal error".to_string(),
            };
            (status, Json(json!({ "error": error_msg })))
        }
    }
}

/// POST /api/v1/upload
///
/// Upload a file to the Matrix media repository.
/// Accepts multipart/form-data with a single `file` field.
/// Returns the `mxc://` content URI that can be used in message content.
pub(super) async fn handle_upload(
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
        .map(|s| {
            let basename = s.rsplit(['/', '\\']).next().unwrap_or(s);
            if basename.len() > 255 { basename[..255].to_string() } else { basename.to_string() }
        })
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
    if size > MAX_UPLOAD_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(
                json!({ "error": format!("file too large: {size} bytes (max {MAX_UPLOAD_SIZE})") }),
            ),
        );
    }

    let dispatcher = state.dispatcher.read().await;
    match dispatcher
        .matrix_client()
        .upload_media(data, &content_type, &filename)
        .await
    {
        Ok(mxc_uri) => {
            debug!(
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
            error!(error = %e, "media upload failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}
