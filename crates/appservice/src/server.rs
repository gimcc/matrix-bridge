use std::collections::{BTreeMap, HashMap};
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
use ruma::OwnedUserId;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::auth::{ApiKey, HsToken, verify_api_key, verify_hs_token};
use crate::bridge_api;
use crate::crypto_pool::CryptoManagerPool;
use crate::dispatcher::Dispatcher;

/// Maximum number of transaction IDs to keep for deduplication.
const MAX_PROCESSED_TXNS: usize = 10_000;

/// Non-sensitive server configuration exposed via the info API.
#[derive(Debug, Clone, Serialize)]
pub struct BridgeInfo {
    pub homeserver_url: String,
    pub homeserver_domain: String,
    pub bot_user_id: String,
    pub puppet_prefix: String,
    pub encryption_enabled: bool,
    pub encryption_default: bool,
    pub webhook_ssrf_protection: bool,
    pub api_key_required: bool,
    pub configured_platforms: Vec<String>,
    pub invite_whitelist: Vec<String>,
}

/// Shared application state for the axum server.
pub struct AppState {
    pub dispatcher: Arc<Mutex<Dispatcher>>,
    /// Track processed transaction IDs to deduplicate (insertion-ordered, bounded).
    pub processed_txns: Mutex<IndexSet<String>>,
    /// Optional crypto manager pool for E2BE (end-to-bridge encryption).
    pub crypto_pool: Option<Arc<CryptoManagerPool>>,
    /// Whether to block webhook URLs targeting private/reserved IPs.
    pub webhook_ssrf_protection: bool,
    /// Matrix user IDs to auto-invite when the bridge creates a room.
    pub auto_invite: Vec<String>,
    /// Whether to allow the API `invite` field in room creation requests.
    pub allow_api_invite: bool,
    /// Whether to auto-enable encryption for newly created rooms.
    pub encryption_default: bool,
    /// Non-sensitive server info for the info API.
    pub bridge_info: BridgeInfo,
}

/// Build the axum Router for the appservice HTTP endpoints.
///
/// - `hs_token`: Matrix protocol shared secret (Synapse ↔ appservice). Always
///   required on `/_matrix/app/v1/*` routes.
/// - `api_key`: optional API key for the Bridge HTTP API (`/api/v1/admin/*`). When
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
    if let Some(pool) = &state.crypto_pool {
        // Support both unstable (de.sorunome.msc2409) and stable (org.matrix.msc2409) prefixes.
        let to_device_events = body
            .get("de.sorunome.msc2409.to_device")
            .or_else(|| body.get("org.matrix.msc2409.to_device"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let raw_events: Vec<_> = to_device_events
            .iter()
            .filter_map(|v| {
                serde_json::value::to_raw_value(v)
                    .ok()
                    .map(ruma::serde::Raw::from_json)
            })
            .collect();

        // Support both unstable (de.sorunome.msc3202) and stable (org.matrix.msc3202) prefixes.
        let changed_devices: ruma::api::client::sync::sync_events::DeviceLists = body
            .get("org.matrix.msc3202.device_lists")
            .or_else(|| body.get("de.sorunome.msc3202.device_lists"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // Parse OTK counts. MSC3202 per-user format:
        //   { "@user:server": { "DEVICE_ID": { "signed_curve25519": 20 } } }
        // Legacy single-device format:
        //   { "signed_curve25519": 20 }
        let otk_raw = body
            .get("org.matrix.msc3202.device_one_time_keys_count")
            .or_else(|| body.get("de.sorunome.msc3202.device_one_time_keys_count"))
            .or_else(|| body.get("org.matrix.msc3202.device_one_time_key_counts"))
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        let (otk_counts, per_user_otk_counts) = parse_per_user_otk_counts(&otk_raw);

        // Parse fallback keys. Per-user format:
        //   { "@user:server": { "DEVICE_ID": ["signed_curve25519"] } }
        // Legacy format:
        //   ["signed_curve25519"]
        let fallback_raw = body
            .get("org.matrix.msc3202.device_unused_fallback_key_types")
            .or_else(|| body.get("de.sorunome.msc3202.device_unused_fallback_key_types"))
            .cloned();

        let (fallback_keys, per_user_fallback_keys) = parse_per_user_fallback_keys(&fallback_raw);

        // Parse per-user to-device events (route by recipient).
        let per_user_to_device = if pool.is_per_user() {
            parse_per_user_to_device(&to_device_events)
        } else {
            HashMap::new()
        };

        if !raw_events.is_empty() || !changed_devices.changed.is_empty() || !otk_counts.is_empty()
        {
            debug!(
                to_device = raw_events.len(),
                device_list_changed = changed_devices.changed.len(),
                otk_counts = otk_counts.len(),
                per_user_otk = per_user_otk_counts.len(),
                "processing MSC2409/3202 crypto data"
            );
        }

        if let Err(e) = pool
            .receive_sync_changes(
                raw_events,
                &changed_devices,
                &otk_counts,
                fallback_keys.as_deref(),
                &per_user_otk_counts,
                &per_user_fallback_keys,
                &per_user_to_device,
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
        .handle_transaction(&events, state.crypto_pool.as_deref())
        .await;

    // Flush any outgoing crypto requests generated during event processing
    // (e.g., key queries triggered by new encrypted rooms, key claims, etc.).
    if let Some(pool) = &state.crypto_pool
        && let Err(e) = pool.process_all_outgoing_requests().await
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

/// Parse OTK counts from MSC3202 transaction data.
///
/// Supports two formats:
/// - Per-user (MSC3202): `{ "@user:server": { "DEVICE": { "signed_curve25519": 20 } } }`
/// - Legacy flat: `{ "signed_curve25519": 20 }`
///
/// Returns (flat_counts, per_user_counts).
fn parse_per_user_otk_counts(
    raw: &Value,
) -> (
    BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt>,
    HashMap<OwnedUserId, BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt>>,
) {
    let mut flat: BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt> = BTreeMap::new();
    let mut per_user: HashMap<OwnedUserId, BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt>> =
        HashMap::new();

    let obj = match raw.as_object() {
        Some(o) => o,
        None => return (flat, per_user),
    };

    // Detect format: if any key starts with '@', it's per-user format.
    let is_per_user = obj.keys().any(|k| k.starts_with('@'));

    if is_per_user {
        // Per-user format: { "@user:server": { "DEVICE_ID": { "algo": count } } }
        for (user_id_str, devices_val) in obj {
            let user_id: OwnedUserId = match user_id_str.parse() {
                Ok(u) => u,
                Err(_) => continue,
            };
            let Some(devices) = devices_val.as_object() else {
                continue;
            };
            // Flatten per-device counts into per-user counts
            // (each puppet has only one device, so this is a simple merge).
            let mut user_counts = BTreeMap::new();
            for (_device_id, counts_val) in devices {
                if let Ok(counts) =
                    serde_json::from_value::<BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt>>(
                        counts_val.clone(),
                    )
                {
                    user_counts.extend(counts);
                }
            }
            per_user.insert(user_id, user_counts);
        }
    } else {
        // Legacy flat format: { "signed_curve25519": 20 }
        if let Ok(counts) = serde_json::from_value(raw.clone()) {
            flat = counts;
        }
    }

    (flat, per_user)
}

/// Parse fallback keys from MSC3202 transaction data.
///
/// Supports two formats:
/// - Per-user: `{ "@user:server": { "DEVICE": ["signed_curve25519"] } }`
/// - Legacy flat: `["signed_curve25519"]`
fn parse_per_user_fallback_keys(
    raw: &Option<Value>,
) -> (
    Option<Vec<ruma::OneTimeKeyAlgorithm>>,
    HashMap<OwnedUserId, Option<Vec<ruma::OneTimeKeyAlgorithm>>>,
) {
    let mut flat: Option<Vec<ruma::OneTimeKeyAlgorithm>> = None;
    let mut per_user: HashMap<OwnedUserId, Option<Vec<ruma::OneTimeKeyAlgorithm>>> =
        HashMap::new();

    let Some(raw) = raw else {
        return (flat, per_user);
    };

    if raw.is_array() {
        // Legacy flat format.
        flat = serde_json::from_value(raw.clone()).ok();
    } else if let Some(obj) = raw.as_object() {
        // Per-user format.
        for (user_id_str, devices_val) in obj {
            let user_id: OwnedUserId = match user_id_str.parse() {
                Ok(u) => u,
                Err(_) => continue,
            };
            let Some(devices) = devices_val.as_object() else {
                continue;
            };
            // Flatten per-device to per-user (take first device's types).
            for (_device_id, types_val) in devices {
                let types: Option<Vec<ruma::OneTimeKeyAlgorithm>> =
                    serde_json::from_value(types_val.clone()).ok();
                per_user.insert(user_id.clone(), types);
                break;
            }
        }
    }

    (flat, per_user)
}

/// Route to-device events by recipient user ID.
///
/// Each to-device event in the MSC2409 array may have a `to_user_id` field
/// (MSC3202 extension) indicating which appservice user should receive it.
/// Falls back to inspecting the encrypted content for `recipient` or
/// `recipient_keys` fields.
fn parse_per_user_to_device(
    events: &[Value],
) -> HashMap<OwnedUserId, Vec<ruma::serde::Raw<ruma::events::AnyToDeviceEvent>>> {
    let mut result: HashMap<OwnedUserId, Vec<ruma::serde::Raw<ruma::events::AnyToDeviceEvent>>> =
        HashMap::new();

    for event in events {
        // MSC3202 adds `to_user_id` to to-device events in appservice transactions.
        let to_user = event
            .get("to_user_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<OwnedUserId>().ok());

        if let Some(user_id) = to_user {
            if let Ok(raw_val) = serde_json::value::to_raw_value(event) {
                result
                    .entry(user_id)
                    .or_default()
                    .push(ruma::serde::Raw::from_json(raw_val));
            }
        }
        // Events without to_user_id are dropped in per-user mode
        // (they shouldn't exist per MSC3202).
    }

    result
}
