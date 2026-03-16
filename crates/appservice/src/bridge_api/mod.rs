mod admin_handlers;
mod message_handlers;
mod room_handlers;
mod space_handlers;
#[cfg(test)]
mod tests;
mod types;
pub(crate) mod validation;
mod webhook_handlers;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};

use matrix_bridge_core::config::MAX_MEDIA_SIZE;

use crate::server::AppState;

// Re-export public types for external consumers.
pub(crate) use types::convert_content;
pub use types::{
    ContentPayload, CreateRoomMappingRequest, CreateWebhookRequest, SendMessageRequest,
    SendMessageResponse, SenderInfo,
};

/// Parsed cursor-based pagination parameters.
pub(super) struct PaginationParams<'a> {
    pub platform: Option<&'a str>,
    pub after: i64,
    pub limit: i64,
}

impl<'a> PaginationParams<'a> {
    pub fn from_query(params: &'a HashMap<String, String>) -> Self {
        Self {
            platform: params.get("platform").map(|s| s.as_str()),
            after: params
                .get("after")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0),
            limit: params
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(100)
                .clamp(1, 1000),
        }
    }
}

/// Build the bridge API router.
///
/// Routes are split into two groups:
/// - `/api/v1/...` — operational endpoints used by platform integrations
/// - `/api/v1/admin/...` — read-only status and monitoring endpoints
pub fn build_bridge_api_router() -> Router<Arc<AppState>> {
    Router::new()
        // Operational: messaging, room/webhook creation and deletion
        .route(
            "/api/v1/message",
            post(message_handlers::handle_send_message),
        )
        .route(
            "/api/v1/upload",
            post(message_handlers::handle_upload).layer(DefaultBodyLimit::max(MAX_MEDIA_SIZE)),
        )
        .route(
            "/api/v1/rooms",
            post(room_handlers::handle_create_room_mapping),
        )
        .route(
            "/api/v1/rooms/{id}",
            delete(room_handlers::handle_delete_room_mapping),
        )
        .route(
            "/api/v1/webhooks",
            post(webhook_handlers::handle_create_webhook),
        )
        .route(
            "/api/v1/webhooks/{id}",
            delete(webhook_handlers::handle_delete_webhook),
        )
        // Admin: read-only status, monitoring, and listing
        .route(
            "/api/v1/admin/info",
            get(admin_handlers::handle_server_info),
        )
        .route(
            "/api/v1/admin/puppets",
            get(admin_handlers::handle_list_puppets),
        )
        .route(
            "/api/v1/admin/messages",
            get(admin_handlers::handle_list_messages),
        )
        .route(
            "/api/v1/admin/crypto",
            get(admin_handlers::handle_crypto_status),
        )
        .route(
            "/api/v1/admin/rooms",
            get(room_handlers::handle_list_room_mappings),
        )
        .route(
            "/api/v1/admin/webhooks",
            get(webhook_handlers::handle_list_webhooks),
        )
        .route(
            "/api/v1/admin/spaces",
            get(space_handlers::handle_list_spaces),
        )
        .route(
            "/api/v1/admin/capabilities",
            get(admin_handlers::handle_platform_capabilities),
        )
}
