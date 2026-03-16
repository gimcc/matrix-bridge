use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tracing::{debug, error};

use crate::server::AppState;

use super::CreateWebhookRequest;
use super::validation::validate_webhook_url;

/// POST /api/v1/webhooks
pub(super) async fn handle_create_webhook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWebhookRequest>,
) -> impl IntoResponse {
    if let Err(e) = validate_webhook_url(&req.url, state.webhook_ssrf_protection).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid webhook URL: {e}") })),
        );
    }
    let forward_sources = req.forward_sources.join(",");
    let dispatcher = state.dispatcher.read().await;
    match dispatcher
        .db()
        .create_webhook(&req.platform, &req.url, &req.events, &forward_sources)
        .await
    {
        Ok(id) => {
            debug!(
                platform = req.platform,
                url = req.url,
                forward_sources,
                "webhook registered via API"
            );
            (StatusCode::CREATED, Json(json!({ "id": id })))
        }
        Err(e) => {
            error!(error = %e, "create webhook failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// GET /api/v1/admin/webhooks?platform=xxx&after=0&limit=100
pub(super) async fn handle_list_webhooks(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    let platform = params.get("platform").map(|s| s.as_str());
    let after = params
        .get("after")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 1000);

    match dispatcher
        .db()
        .list_webhooks_paginated(platform, after, limit)
        .await
    {
        Ok(webhooks) => {
            let next_cursor = webhooks.last().map(|w| w.id);
            (
                StatusCode::OK,
                Json(json!({
                    "webhooks": webhooks,
                    "next_cursor": next_cursor,
                })),
            )
        }
        Err(e) => {
            error!(error = %e, "list webhooks failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}

/// DELETE /api/v1/webhooks/{id}
pub(super) async fn handle_delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    match dispatcher.db().delete_webhook(id).await {
        Ok(true) => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            error!(error = %e, "delete webhook failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}
