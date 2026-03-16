use axum::{
    Json,
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use tracing::debug;

/// GET /_matrix/app/v1/users/{userId}
///
/// The homeserver queries whether a user is managed by this appservice.
pub(crate) async fn handle_user_query(Path(user_id): Path<String>) -> impl IntoResponse {
    debug!(user_id, "user query");
    (StatusCode::OK, Json(json!({})))
}

/// GET /_matrix/app/v1/rooms/{roomAlias}
///
/// The homeserver queries whether a room alias is managed by this appservice.
pub(crate) async fn handle_room_query(Path(room_alias): Path<String>) -> impl IntoResponse {
    debug!(room_alias, "room alias query");
    (StatusCode::OK, Json(json!({})))
}
