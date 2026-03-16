use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;
use tracing::error;

use crate::server::AppState;

/// GET /api/v1/admin/spaces
///
/// List all platform space mappings.
pub(super) async fn handle_list_spaces(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dispatcher = state.dispatcher.read().await;
    match dispatcher.db().list_platform_spaces().await {
        Ok(spaces) => (StatusCode::OK, Json(json!({ "spaces": spaces }))),
        Err(e) => {
            error!(error = %e, "list platform spaces failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
        }
    }
}
