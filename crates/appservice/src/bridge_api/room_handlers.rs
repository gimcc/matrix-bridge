use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::{Value, json};
use tracing::{debug, error, info};

use crate::dispatcher::Dispatcher;
use crate::server::AppState;

use super::CreateRoomMappingRequest;

/// Maximum number of Matrix user IDs in an invite list.
const MAX_INVITE_LIST_SIZE: usize = 50;
/// Maximum length for room names and user IDs.
const MAX_USER_ID_LEN: usize = 255;
const MAX_ROOM_NAME_LEN: usize = 255;

/// Create a new Matrix room with the given name, invite list, and encryption
/// settings.  Returns the new room's Matrix ID on success.
async fn auto_create_room(
    state: &AppState,
    dispatcher: &Dispatcher,
    room_name: &str,
    invite: &[String],
) -> Result<String, (StatusCode, Json<Value>)> {
    let invite_refs: Vec<&str> = invite.iter().map(|s| s.as_str()).collect();

    let id = dispatcher
        .matrix_client()
        .create_room(Some(room_name), &invite_refs, state.encryption_default)
        .await
        .map_err(|e| {
            error!(error = %e, "auto-create room failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to create room" })),
            )
        })?;

    // Register encryption state and track member devices so
    // other clients share Megolm session keys with the bridge.
    if state.encryption_default
        && let Some(pool) = &state.crypto_pool
        && let Ok(ruma_room_id) = <&ruma::RoomId>::try_from(id.as_str())
    {
        if let Err(e) = pool.bot().set_room_encrypted(ruma_room_id).await {
            error!(room_id = %id, error = %e, "failed to mark room as encrypted");
        }
        // Query device keys for invited members.
        let members: Vec<ruma::OwnedUserId> =
            invite.iter().filter_map(|u| u.parse().ok()).collect();
        if !members.is_empty()
            && let Err(e) = pool.bot().update_tracked_users(&members).await
        {
            error!(room_id = %id, error = %e, "failed to track user devices");
        }
    }

    info!(
        room_id = %id,
        invited = ?invite,
        "auto-created Matrix room for mapping"
    );
    Ok(id)
}

/// POST /api/v1/rooms
///
/// Idempotent: if a mapping for `(platform, external_room_id)` already exists,
/// returns the existing mapping (200). Otherwise creates a new one (201).
/// When `matrix_room_id` is omitted and no existing mapping is found, the
/// bridge auto-creates a new Matrix room.
pub(super) async fn handle_create_room_mapping(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRoomMappingRequest>,
) -> impl IntoResponse {
    // Validate room_name length.
    if let Some(ref name) = req.room_name
        && name.len() > MAX_ROOM_NAME_LEN
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("room_name exceeds {} characters", MAX_ROOM_NAME_LEN) })),
        );
    }

    // Validate invite entries when allow_api_invite is enabled.
    if state.allow_api_invite {
        if req.invite.len() > MAX_INVITE_LIST_SIZE {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({ "error": format!("invite list exceeds maximum of {} entries", MAX_INVITE_LIST_SIZE) }),
                ),
            );
        }
        for user_id in &req.invite {
            if user_id.len() > MAX_USER_ID_LEN {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("invite user ID too long: {}", user_id) })),
                );
            }
            if !matrix_bridge_core::platform::is_valid_matrix_user_id(user_id) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("invalid Matrix user ID: {}", user_id) })),
                );
            }
        }
    }

    let dispatcher = state.dispatcher.read().await;

    // Check for an existing mapping first.
    match dispatcher
        .db()
        .find_room_by_external_id(&req.platform, &req.external_room_id)
        .await
    {
        Ok(Some(existing)) => {
            // If caller provided a specific matrix_room_id that differs, update it.
            if let Some(ref wanted) = req.matrix_room_id
                && !wanted.is_empty()
                && wanted != &existing.matrix_room_id
            {
                match dispatcher
                    .db()
                    .create_room_mapping(wanted, &req.platform, &req.external_room_id)
                    .await
                {
                    Ok(id) => {
                        debug!(
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
                        error!(error = %e, "update room mapping failed");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": "internal error" })),
                        );
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
            error!(error = %e, "find room mapping failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            );
        }
    }

    // Resolve the Matrix room ID: use the provided one or auto-create.
    // Track whether we created the room so cleanup only applies to our rooms.
    let (matrix_room_id, auto_created) = match req.matrix_room_id {
        Some(id) if !id.is_empty() => (id, false),
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

            let room_name = req.room_name.as_deref().unwrap_or(&req.external_room_id);
            match auto_create_room(&state, &dispatcher, room_name, &invite_users).await {
                Ok(id) => (id, true),
                Err(resp) => return resp,
            }
        }
    };

    match dispatcher
        .db()
        .create_room_mapping(&matrix_room_id, &req.platform, &req.external_room_id)
        .await
    {
        Ok(id) => {
            debug!(
                platform = req.platform,
                external = req.external_room_id,
                matrix = matrix_room_id,
                "room mapping created via API"
            );

            // Add the new room to the platform's Space (best-effort).
            dispatcher
                .ensure_platform_space(&req.platform, &matrix_room_id)
                .await;

            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id,
                    "matrix_room_id": matrix_room_id,
                })),
            )
        }
        Err(e) => {
            error!(
                orphaned_room_id = %matrix_room_id,
                error = %e,
                "create room mapping failed — attempting cleanup"
            );
            // Only clean up rooms we auto-created; caller-supplied rooms are not ours to leave.
            if auto_created {
                if let Err(cleanup_err) = dispatcher
                    .matrix_client()
                    .leave_room_as_bot(&matrix_room_id)
                    .await
                {
                    error!(
                        orphaned_room_id = %matrix_room_id,
                        error = %cleanup_err,
                        "failed to clean up orphaned room"
                    );
                }
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/admin/rooms?platform=xxx&after=0&limit=100
///
/// List room mappings with cursor-based pagination.
/// When `platform` is provided, returns mappings for that platform only.
pub(super) async fn handle_list_room_mappings(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    let pg = super::PaginationParams::from_query(&params);

    match dispatcher
        .db()
        .list_room_mappings_paginated(pg.platform, pg.after, pg.limit)
        .await
    {
        Ok(mappings) => {
            let next_cursor = mappings.last().map(|m| m.id);
            (
                StatusCode::OK,
                Json(json!({
                    "rooms": mappings,
                    "next_cursor": next_cursor,
                })),
            )
        }
        Err(e) => {
            error!(error = %e, "list room mappings failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// DELETE /api/v1/rooms/{id}
pub(super) async fn handle_delete_room_mapping(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    match dispatcher.db().delete_room_mapping(id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            error!(error = %e, "delete room mapping failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}
