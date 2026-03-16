mod admin_handlers;
mod message_handlers;
mod room_handlers;
#[cfg(test)]
mod tests;
mod types;
pub(crate) mod validation;
mod webhook_handlers;

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};

use crate::server::AppState;

// Re-export public types for external consumers.
pub use types::{
    ContentPayload, CreateRoomMappingRequest, CreateWebhookRequest, SendMessageRequest,
    SendMessageResponse, SenderInfo,
};
pub(crate) use types::convert_content;

/// Build the bridge API router.
///
/// Routes are split into two groups:
/// - `/api/v1/...` — operational endpoints used by platform integrations
/// - `/api/v1/admin/...` — read-only status and monitoring endpoints
pub fn build_bridge_api_router() -> Router<Arc<AppState>> {
    Router::new()
        // Operational: messaging, room/webhook creation and deletion
        .route("/api/v1/message", post(message_handlers::handle_send_message))
        .route("/api/v1/upload", post(message_handlers::handle_upload).layer(DefaultBodyLimit::max(200 * 1024 * 1024)))
        .route("/api/v1/rooms", post(room_handlers::handle_create_room_mapping))
        .route("/api/v1/rooms/{id}", delete(room_handlers::handle_delete_room_mapping))
        .route("/api/v1/webhooks", post(webhook_handlers::handle_create_webhook))
        .route("/api/v1/webhooks/{id}", delete(webhook_handlers::handle_delete_webhook))
        // Admin: read-only status, monitoring, and listing
        .route("/api/v1/admin/info", get(admin_handlers::handle_server_info))
        .route("/api/v1/admin/puppets", get(admin_handlers::handle_list_puppets))
        .route("/api/v1/admin/messages", get(admin_handlers::handle_list_messages))
        .route("/api/v1/admin/crypto", get(admin_handlers::handle_crypto_status))
        .route("/api/v1/admin/rooms", get(room_handlers::handle_list_room_mappings))
        .route("/api/v1/admin/webhooks", get(webhook_handlers::handle_list_webhooks))
}
