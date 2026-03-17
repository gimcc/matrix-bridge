use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, put},
};
use indexmap::IndexSet;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::auth::{ApiKey, HsToken, verify_api_key, verify_hs_token};
use crate::bridge_api;
use crate::crypto_manager::CryptoManager;
use crate::dispatcher::Dispatcher;

/// Maximum number of transaction IDs to keep for deduplication.
const MAX_PROCESSED_TXNS: usize = 10_000;

/// Shared application state for the axum server.
pub struct AppState {
    pub dispatcher: Arc<Mutex<Dispatcher>>,
    /// Track processed transaction IDs to deduplicate (insertion-ordered, bounded).
    pub processed_txns: Mutex<IndexSet<String>>,
    /// Optional crypto manager for E2BE (end-to-bridge encryption).
    pub crypto_manager: Option<Arc<CryptoManager>>,
    /// Whether to block webhook URLs targeting private/reserved IPs.
    pub webhook_ssrf_protection: bool,
    /// Matrix user IDs to auto-invite when the bridge creates a room.
    pub auto_invite: Vec<String>,
    /// Whether to allow the API `invite` field in room creation requests.
    pub allow_api_invite: bool,
    /// Whether to auto-enable encryption for newly created rooms.
    pub encryption_default: bool,
}

/// Build the axum Router for the appservice HTTP endpoints.
///
/// - `hs_token`: Matrix protocol shared secret (Synapse ↔ appservice). Always
///   required on `/_matrix/app/v1/*` routes.
/// - `api_key`: optional API key for the Bridge HTTP API (`/api/v1/*`). When
///   `Some`, every Bridge API request must carry this key. When `None`, the
///   Bridge API is unauthenticated — suitable for internal/trusted-network
///   deployments where access control is handled externally.
pub fn build_router(state: Arc<AppState>, hs_token: String, api_key: Option<String>) -> Router {
    // Matrix appservice endpoints (always require hs_token auth).
    let matrix_routes = Router::new()
        .route(
            "/_matrix/app/v1/transactions/{txnId}",
            put(handle_transaction),
        )
        .route("/_matrix/app/v1/users/{userId}", get(handle_user_query))
        .route("/_matrix/app/v1/rooms/{roomAlias}", get(handle_room_query))
        .layer(middleware::from_fn(verify_hs_token))
        .layer(Extension(HsToken(hs_token)));

    // Bridge HTTP API endpoints (for external platform services).
    // Authentication is separate from hs_token — uses an independent api_key.
    // Normalize empty string to None so `api_key = ""` behaves as unauthenticated.
    let api_key = api_key.filter(|k| !k.is_empty());
    let bridge_routes = if let Some(key) = api_key {
        bridge_api::build_bridge_api_router()
            .layer(middleware::from_fn(verify_api_key))
            .layer(Extension(ApiKey(key)))
    } else {
        bridge_api::build_bridge_api_router()
    };

    // Merge all routes under one state.
    matrix_routes
        .merge(bridge_routes)
        .route("/health", get(handle_health))
        .with_state(state)
}

/// PUT /_matrix/app/v1/transactions/{txnId}
///
/// Receives a batch of events from the homeserver.
/// With MSC2409/MSC3202 support, also receives to-device events,
/// device list changes, and OTK counts for E2EE.
async fn handle_transaction(
    State(state): State<Arc<AppState>>,
    Path(txn_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // Deduplicate transaction IDs.
    {
        let mut txns = state.processed_txns.lock().await;
        if txns.contains(&txn_id) {
            debug!(txn_id, "duplicate transaction, skipping");
            return (StatusCode::OK, Json(json!({})));
        }
        txns.insert(txn_id.clone());

        // Evict oldest entries when the set exceeds the limit.
        while txns.len() > MAX_PROCESSED_TXNS {
            txns.shift_remove_index(0);
        }
    }

    // Process MSC2409 to-device events and MSC3202 device list/OTK data
    // for end-to-bridge encryption.
    //
    // IMPORTANT: always call receive_sync_changes on every transaction,
    // even when to_device_events is empty.  The OTK counts and device-list
    // changes arrive independently and must be processed to:
    //   1. Upload new one-time keys when the count drops
    //   2. Track device-list changes for room members
    if let Some(crypto) = &state.crypto_manager {
        // Support both unstable (de.sorunome.msc2409) and stable (org.matrix.msc2409) prefixes.
        let to_device_events = body
            .get("de.sorunome.msc2409.to_device")
            .or_else(|| body.get("org.matrix.msc2409.to_device"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let raw_events: Vec<_> = to_device_events
            .into_iter()
            .filter_map(|v| {
                serde_json::value::to_raw_value(&v)
                    .ok()
                    .map(ruma::serde::Raw::from_json)
            })
            .collect();

        // Support both unstable (de.sorunome.msc3202) and stable (org.matrix.msc3202) prefixes.
        // Newer Synapse versions use org.matrix.msc3202, older ones use de.sorunome.msc3202.
        let changed_devices: ruma::api::client::sync::sync_events::DeviceLists = body
            .get("org.matrix.msc3202.device_lists")
            .or_else(|| body.get("de.sorunome.msc3202.device_lists"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let otk_counts: BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt> = body
            .get("org.matrix.msc3202.device_one_time_keys_count")
            .or_else(|| body.get("de.sorunome.msc3202.device_one_time_keys_count"))
            // Also handle the typo'd field name some Synapse versions use.
            .or_else(|| body.get("org.matrix.msc3202.device_one_time_key_counts"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let fallback_keys: Option<Vec<ruma::OneTimeKeyAlgorithm>> = body
            .get("org.matrix.msc3202.device_unused_fallback_key_types")
            .or_else(|| body.get("de.sorunome.msc3202.device_unused_fallback_key_types"))
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        if !raw_events.is_empty() || !changed_devices.changed.is_empty() || !otk_counts.is_empty()
        {
            debug!(
                to_device = raw_events.len(),
                device_list_changed = changed_devices.changed.len(),
                otk_counts = otk_counts.len(),
                "processing MSC2409/3202 crypto data"
            );
        }

        if let Err(e) = crypto
            .receive_sync_changes(
                raw_events,
                &changed_devices,
                &otk_counts,
                fallback_keys.as_deref(),
            )
            .await
        {
            error!("failed to process crypto sync changes: {e}");
        }
    }

    let events = body
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    debug!(txn_id, event_count = events.len(), "processing transaction");

    let dispatcher = state.dispatcher.lock().await;
    dispatcher
        .handle_transaction(&events, state.crypto_manager.as_deref())
        .await;

    // Flush any outgoing crypto requests generated during event processing
    // (e.g., key queries triggered by new encrypted rooms, key claims, etc.).
    if let Some(crypto) = &state.crypto_manager
        && let Err(e) = crypto.process_outgoing_requests().await
    {
        error!("failed to process outgoing crypto requests: {e}");
    }

    (StatusCode::OK, Json(json!({})))
}

/// GET /_matrix/app/v1/users/{userId}
///
/// The homeserver queries whether a user is managed by this appservice.
async fn handle_user_query(Path(user_id): Path<String>) -> impl IntoResponse {
    debug!(user_id, "user query");
    (StatusCode::OK, Json(json!({})))
}

/// GET /_matrix/app/v1/rooms/{roomAlias}
///
/// The homeserver queries whether a room alias is managed by this appservice.
async fn handle_room_query(Path(room_alias): Path<String>) -> impl IntoResponse {
    debug!(room_alias, "room alias query");
    (StatusCode::OK, Json(json!({})))
}

/// GET /health
async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// Start the appservice HTTP server with graceful shutdown on SIGTERM/SIGINT.
pub async fn run_server(
    state: Arc<AppState>,
    address: &str,
    port: u16,
    hs_token: String,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    let app = build_router(state, hs_token, api_key);
    let bind_addr = format!("{address}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!(bind_addr, "appservice HTTP server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("server shut down gracefully");
    Ok(())
}

/// Wait for SIGTERM or SIGINT (Ctrl-C) to trigger graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("received SIGINT, shutting down"),
        _ = terminate => info!("received SIGTERM, shutting down"),
    }
}
