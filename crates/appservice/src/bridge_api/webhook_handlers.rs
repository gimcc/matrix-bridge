use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tracing::{debug, error};
use url::Url;

use crate::server::AppState;

use super::CreateWebhookRequest;
use super::validation::validate_webhook_url;

/// Redact a URL for logging: keep only scheme and host, replace path with `***`.
fn redact_url(url: &str) -> String {
    Url::parse(url)
        .map(|u| format!("{}://{}/***", u.scheme(), u.host_str().unwrap_or("unknown")))
        .unwrap_or_else(|_| "<invalid-url>".to_string())
}

/// POST /api/v1/webhooks
pub(super) async fn handle_create_webhook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWebhookRequest>,
) -> impl IntoResponse {
    if !crate::ws::is_valid_platform_id(&req.platform) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "invalid platform ID: must be 1-64 alphanumeric, '_', '-', or '.' characters" }),
            ),
        );
    }
    if let Err(e) = validate_webhook_url(&req.url, state.webhook_ssrf_protection).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("invalid webhook URL: {e}") })),
        );
    }
    let forward_sources = req.forward_sources.join(",");
    let capabilities = req.capabilities.join(",");
    let dispatcher = state.dispatcher.read().await;
    match dispatcher
        .db()
        .create_webhook(
            &req.platform,
            &req.url,
            &req.events,
            &forward_sources,
            &capabilities,
            &req.owner,
        )
        .await
    {
        Ok(id) => {
            debug!(
                platform = req.platform,
                url = redact_url(&req.url),
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
    let pg = super::PaginationParams::from_query(&params);

    match dispatcher
        .db()
        .list_webhooks_paginated(pg.platform, pg.after, pg.limit)
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
