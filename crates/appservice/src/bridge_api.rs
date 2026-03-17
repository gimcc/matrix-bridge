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

/// Validate that a webhook URL uses an allowed scheme.
/// When `ssrf_protection` is enabled, also blocks localhost, cloud metadata
/// endpoints, and private/reserved IP ranges (RFC1918, link-local, CGNAT, etc.).
/// DNS names are resolved to catch rebinding attacks (e.g., `127.0.0.1.nip.io`).
fn validate_webhook_url(url: &str, ssrf_protection: bool) -> Result<(), String> {
    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("unsupported scheme: {other}")),
    }

    if !ssrf_protection {
        return Ok(());
    }

    let host = parsed.host_str().ok_or("missing host")?;

    // Block well-known dangerous hostnames.
    let blocked_hosts = ["localhost", "metadata.google.internal"];
    if blocked_hosts.contains(&host) {
        return Err(format!("blocked host: {host}"));
    }

    // Parse as IP and block private/reserved ranges.
    if let Ok(ip) = host.parse::<std::net::IpAddr>()
        && is_private_ip(ip)
    {
        return Err(format!("blocked private/reserved IP: {ip}"));
    }
    // Also try stripping brackets for IPv6 (e.g., "[::1]").
    let stripped = host.trim_start_matches('[').trim_end_matches(']');
    if stripped != host
        && let Ok(ip) = stripped.parse::<std::net::IpAddr>()
        && is_private_ip(ip)
    {
        return Err(format!("blocked private/reserved IP: {ip}"));
    }

    // Resolve DNS names to catch rebinding attacks (e.g., 127.0.0.1.nip.io).
    // Only check if the host is not already a raw IP address.
    if host.parse::<std::net::IpAddr>().is_err() && stripped.parse::<std::net::IpAddr>().is_err() {
        let port = parsed
            .port()
            .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
        let authority = format!("{host}:{port}");
        if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&authority) {
            for addr in addrs {
                if is_private_ip(addr.ip()) {
                    return Err(format!(
                        "host {host} resolves to blocked private/reserved IP: {}",
                        addr.ip()
                    ));
                }
            }
        }
        // If DNS resolution fails, the webhook will fail at delivery time anyway.
    }

    Ok(())
}

/// Check if an IP address belongs to a private, loopback, link-local,
/// or otherwise reserved range that should not be reachable via webhooks.
fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.0.0.0/8
            || v4.is_private()        // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()     // 169.254.0.0/16
            || v4.is_unspecified()    // 0.0.0.0
            || v4.is_broadcast()      // 255.255.255.255
            || v4.is_documentation()  // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
            || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGNAT)
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()          // ::1
            || v6.is_unspecified()    // ::
            || (seg[0] & 0xfe00) == 0xfc00  // fc00::/7 (unique local address)
            || (seg[0] & 0xffc0) == 0xfe80  // fe80::/10 (link-local)
            // Check for IPv4-mapped IPv6 (::ffff:x.x.x.x).
            || match v6.to_ipv4_mapped() {
                Some(v4) => is_private_ip(std::net::IpAddr::V4(v4)),
                None => false,
            }
        }
    }
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
///
/// Idempotent: if a mapping for `(platform, external_room_id)` already exists,
/// returns the existing mapping (200). Otherwise creates a new one (201).
/// When `matrix_room_id` is omitted and no existing mapping is found, the
/// bridge auto-creates a new Matrix room.
async fn handle_create_room_mapping(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRoomMappingRequest>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.lock().await;

    // Check for an existing mapping first.
    match dispatcher
        .db()
        .find_room_by_external_id(&req.platform, &req.external_room_id)
        .await
    {
        Ok(Some(existing)) => {
            // If caller provided a specific matrix_room_id that differs, update it.
            if let Some(ref wanted) = req.matrix_room_id {
                if !wanted.is_empty() && wanted != &existing.matrix_room_id {
                    match dispatcher
                        .db()
                        .create_room_mapping(wanted, &req.platform, &req.external_room_id)
                        .await
                    {
                        Ok(id) => {
                            info!(
                                platform = req.platform,
                                external = req.external_room_id,
                                matrix = %wanted,
                                "room mapping updated via API"
                            );
                            return (
                                StatusCode::OK,
                                Json(json!({
                                    "id": id,
                                    "matrix_room_id": wanted,
                                })),
                            );
                        }
                        Err(e) => {
                            error!("update room mapping failed: {e}");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(json!({ "error": "internal error" })),
                            );
                        }
                    }
                }
            }
            // Existing mapping matches — return as-is.
            return (
                StatusCode::OK,
                Json(json!({
                    "id": existing.id,
                    "matrix_room_id": existing.matrix_room_id,
                })),
            );
        }
        Ok(None) => {} // No existing mapping — proceed to create.
        Err(e) => {
            error!("find room mapping failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            );
        }
    }

    // Resolve the Matrix room ID: use the provided one or auto-create.
    let matrix_room_id = match req.matrix_room_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            // Global auto_invite always applied; per-request invite only if config allows.
            let mut invite_users: Vec<String> = state.auto_invite.clone();
            if state.allow_api_invite {
                for u in &req.invite {
                    if !invite_users.contains(u) {
                        invite_users.push(u.clone());
                    }
                }
            }
            let invite_refs: Vec<&str> = invite_users.iter().map(|s| s.as_str()).collect();

            let room_name = req.room_name.as_deref().unwrap_or(&req.external_room_id);
            match dispatcher
                .matrix_client()
                .create_room(Some(room_name), &invite_refs, state.encryption_default)
                .await
            {
                Ok(id) => {
                    // Register encryption state and track member devices so
                    // other clients share Megolm session keys with the bridge.
                    if state.encryption_default {
                        if let Some(crypto) = &state.crypto_manager {
                            if let Ok(ruma_room_id) =
                                <&ruma::RoomId>::try_from(id.as_str())
                            {
                                if let Err(e) = crypto.set_room_encrypted(ruma_room_id).await {
                                    error!(room_id = %id, "failed to mark room as encrypted: {e}");
                                }
                                // Query device keys for invited members.
                                let members: Vec<ruma::OwnedUserId> = invite_users
                                    .iter()
                                    .filter_map(|u| u.parse().ok())
                                    .collect();
                                if !members.is_empty() {
                                    if let Err(e) = crypto.update_tracked_users(&members).await {
                                        error!(room_id = %id, "failed to track user devices: {e}");
                                    }
                                }
                            }
                        }
                    }
                    info!(
                        room_id = %id,
                        invited = ?invite_users,
                        "auto-created Matrix room for mapping"
                    );
                    id
                }
                Err(e) => {
                    error!("auto-create room failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "failed to create room" })),
                    );
                }
            }
        }
    };

    match dispatcher
        .db()
        .create_room_mapping(&matrix_room_id, &req.platform, &req.external_room_id)
        .await
    {
        Ok(id) => {
            info!(
                platform = req.platform,
                external = req.external_room_id,
                matrix = matrix_room_id,
                "room mapping created via API"
            );
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id,
                    "matrix_room_id": matrix_room_id,
                })),
            )
        }
        Err(e) => {
            error!("create room mapping failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "internal error",
                    "matrix_room_id": matrix_room_id,
                })),
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
    if let Err(e) = validate_webhook_url(&req.url, state.webhook_ssrf_protection) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid webhook URL: {e}") })),
        );
    }
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

    const MAX_UPLOAD_SIZE: usize = 200 * 1024 * 1024; // 200 MB

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
