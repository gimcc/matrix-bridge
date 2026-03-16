use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;
use tracing::error;

use crate::server::AppState;

/// GET /api/v1/admin/info
///
/// Returns server configuration and runtime statistics.
pub(super) async fn handle_server_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let info = &state.bridge_info;

    let dispatcher = state.dispatcher.read().await;
    let db = dispatcher.db();

    let room_mappings = db.count_room_mappings().await.unwrap_or(-1);
    let webhooks = db.count_webhooks().await.unwrap_or(-1);
    let message_mappings = db.count_message_mappings().await.unwrap_or(-1);
    let puppets = db.count_puppets().await.unwrap_or(-1);
    let active_platforms = db.list_active_platforms().await.unwrap_or_default();

    (
        StatusCode::OK,
        Json(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "homeserver": {
                "url": info.homeserver_url,
                "domain": info.homeserver_domain,
            },
            "bot": {
                "user_id": info.bot_user_id,
                "puppet_prefix": info.puppet_prefix,
            },
            "features": {
                "encryption_enabled": info.encryption_enabled,
                "encryption_default": info.encryption_default,
                "webhook_ssrf_protection": info.webhook_ssrf_protection,
                "api_key_required": info.api_key_required,
                "websocket_enabled": true,
            },
            "permissions": {
                "admin_count": info.admin_users.len(),
                "relay_count": info.relay_users.len(),
            },
            "platforms": {
                "configured": info.configured_platforms,
                "active": active_platforms,
            },
            "stats": {
                "room_mappings": room_mappings,
                "webhooks": webhooks,
                "message_mappings": message_mappings,
                "puppets": puppets,
                "ws_clients": state.ws_registry.total_clients(),
            },
        })),
    )
}

/// GET /api/v1/admin/puppets?platform=xxx&after=0&limit=100
///
/// List puppet (external) users with cursor-based pagination.
/// When `platform` is provided, returns puppets for that platform only.
pub(super) async fn handle_list_puppets(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    let pg = super::PaginationParams::from_query(&params);

    match dispatcher
        .db()
        .list_puppets_paginated(pg.platform, pg.after, pg.limit)
        .await
    {
        Ok(puppets) => {
            let next_cursor = puppets.last().map(|p| p.id);
            (
                StatusCode::OK,
                Json(json!({
                    "puppets": puppets,
                    "next_cursor": next_cursor,
                })),
            )
        }
        Err(e) => {
            error!(error = %e, "list puppets failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/admin/messages?platform=xxx&room_mapping_id=1&after=0&limit=100
///
/// List message mappings with cursor-based pagination.
pub(super) async fn handle_list_messages(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    let pg = super::PaginationParams::from_query(&params);
    let room_mapping_id = params
        .get("room_mapping_id")
        .and_then(|s| s.parse::<i64>().ok());

    match dispatcher
        .db()
        .list_message_mappings(pg.platform, room_mapping_id, pg.after, pg.limit)
        .await
    {
        Ok(messages) => {
            let next_cursor = messages.last().map(|m| m.id);
            (
                StatusCode::OK,
                Json(json!({
                    "messages": messages,
                    "next_cursor": next_cursor,
                })),
            )
        }
        Err(e) => {
            error!(error = %e, "list message mappings failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/admin/capabilities?platform=xxx
///
/// Returns the aggregated capabilities for a platform (union of all webhooks + WS clients).
pub(super) async fn handle_platform_capabilities(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(platform) = params.get("platform") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing required query parameter: platform" })),
        );
    };

    let dispatcher = state.dispatcher.read().await;

    // Aggregate from webhooks (DB).
    let mut caps = std::collections::BTreeSet::new();
    match dispatcher.db().get_platform_capabilities(platform).await {
        Ok(db_caps) => caps.extend(db_caps),
        Err(e) => {
            error!(error = %e, "failed to query webhook capabilities");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            );
        }
    }

    // Aggregate from WS clients (in-memory).
    let ws_caps = state.ws_registry.get_capabilities(platform);
    caps.extend(ws_caps);

    let caps_vec: Vec<&str> = caps.iter().map(|s| s.as_str()).collect();
    (
        StatusCode::OK,
        Json(json!({
            "platform": platform,
            "capabilities": caps_vec,
        })),
    )
}

/// GET /api/v1/admin/crypto
///
/// Returns encryption key status for the bot and all initialized puppets.
pub(super) async fn handle_crypto_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(pool) = &state.crypto_pool else {
        return (
            StatusCode::OK,
            Json(json!({
                "enabled": false,
                "per_user_crypto": false,
                "bot": null,
                "puppets": [],
            })),
        );
    };

    let bot_status = match pool.bot().crypto_status().await {
        Ok(s) => serde_json::to_value(s).unwrap_or_default(),
        Err(e) => {
            error!(error = %e, "failed to query bot crypto status");
            json!({ "error": format!("{e}") })
        }
    };

    let all = pool.get_all().await;
    let mut puppet_statuses = Vec::new();
    for cm in &all {
        // Skip the bot (already handled above).
        if cm.user_id() == pool.bot().user_id() {
            continue;
        }
        match cm.crypto_status().await {
            Ok(s) => puppet_statuses.push(serde_json::to_value(s).unwrap_or_default()),
            Err(e) => {
                error!(user_id = %cm.user_id(), error = %e, "failed to query puppet crypto status");
                puppet_statuses.push(json!({
                    "user_id": cm.user_id().to_string(),
                    "device_id": cm.device_id().to_string(),
                    "error": format!("{e}"),
                }));
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "enabled": true,
            "per_user_crypto": pool.is_per_user(),
            "bot": bot_status,
            "puppets": puppet_statuses,
        })),
    )
}
