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

use matrix_bridge_core::config::MAX_MEDIA_SIZE;

/// Sanitize a user-supplied content type to prevent stored XSS.
///
/// Only well-known safe MIME prefixes are allowed through; everything else
/// is replaced with `application/octet-stream`.
fn sanitize_content_type(ct: &str) -> String {
    const SAFE_PREFIXES: &[&str] = &[
        "image/",
        "video/",
        "audio/",
        "text/plain",
        "application/pdf",
        "application/octet-stream",
    ];
    if SAFE_PREFIXES.iter().any(|prefix| ct.starts_with(prefix)) {
        ct.to_string()
    } else {
        "application/octet-stream".to_string()
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
pub(super) async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // Input validation
    if !crate::ws::is_valid_platform_id(&req.platform) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "invalid platform ID: must be 1-64 alphanumeric, '_', '-', or '.' characters" }),
            ),
        );
    }
    if req.room_id.len() > 255 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "room_id exceeds 255 characters" })),
        );
    }
    if req.sender.id.len() > 255 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "sender.id exceeds 255 characters" })),
        );
    }
    if let Some(ref name) = req.sender.display_name {
        if name.len() > 255 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "display_name exceeds 255 characters" })),
            );
        }
    }
    if let Some(ref mid) = req.external_message_id {
        if mid.len() > 255 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "external_message_id exceeds 255 characters" })),
            );
        }
    }
    if let Some(ref rt) = req.reply_to {
        if rt.len() > 255 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "reply_to exceeds 255 characters" })),
            );
        }
    }
    // Check body length for text-based content
    match &req.content {
        super::ContentPayload::Text { body, .. }
        | super::ContentPayload::Notice { body }
        | super::ContentPayload::Emote { body } => {
            if body.len() > 65536 {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "body exceeds 64KB" })),
                );
            }
        }
        _ => {}
    }
    if let super::ContentPayload::Reaction { ref emoji, .. } = req.content {
        if emoji.chars().count() > 64 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "emoji exceeds 64 characters" })),
            );
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
            if basename.len() > 255 {
                basename.chars().take(255).collect::<String>()
            } else {
                basename.to_string()
            }
        })
        .unwrap_or_else(|| "upload".to_string());

    let content_type = field
        .content_type()
        .map(sanitize_content_type)
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
    if size > MAX_MEDIA_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(
                json!({ "error": format!("file too large: {size} bytes (max {MAX_MEDIA_SIZE})") }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_content_type_allows_safe_types() {
        assert_eq!(sanitize_content_type("image/png"), "image/png");
        assert_eq!(sanitize_content_type("image/jpeg"), "image/jpeg");
        assert_eq!(sanitize_content_type("video/mp4"), "video/mp4");
        assert_eq!(sanitize_content_type("audio/ogg"), "audio/ogg");
        assert_eq!(sanitize_content_type("text/plain"), "text/plain");
        assert_eq!(
            sanitize_content_type("text/plain; charset=utf-8"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(sanitize_content_type("application/pdf"), "application/pdf");
        assert_eq!(
            sanitize_content_type("application/octet-stream"),
            "application/octet-stream"
        );
    }

    #[test]
    fn sanitize_content_type_blocks_dangerous_types() {
        assert_eq!(
            sanitize_content_type("text/html"),
            "application/octet-stream"
        );
        assert_eq!(
            sanitize_content_type("text/xml"),
            "application/octet-stream"
        );
        assert_eq!(
            sanitize_content_type("application/javascript"),
            "application/octet-stream"
        );
        assert_eq!(
            sanitize_content_type("application/xhtml+xml"),
            "application/octet-stream"
        );
        assert_eq!(
            sanitize_content_type("text/html; charset=utf-8"),
            "application/octet-stream"
        );
        assert_eq!(
            sanitize_content_type("multipart/form-data"),
            "application/octet-stream"
        );
    }
}
