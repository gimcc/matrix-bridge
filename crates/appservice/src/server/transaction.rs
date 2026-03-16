use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use ruma::OwnedUserId;
use serde_json::{Value, json};
use tracing::{debug, error};

use super::{AppState, MAX_PROCESSED_TXNS};

/// PUT /_matrix/app/v1/transactions/{txnId}
///
/// Receives a batch of events from the homeserver.
/// With MSC2409/MSC3202 support, also receives to-device events,
/// device list changes, and OTK counts for E2EE.
pub(crate) async fn handle_transaction(
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

        if !raw_events.is_empty() || !changed_devices.changed.is_empty() || !otk_counts.is_empty() {
            debug!(
                to_device = raw_events.len(),
                device_list_changed = changed_devices.changed.len(),
                otk_counts = otk_counts.len(),
                per_user_otk = per_user_otk_counts.len(),
                "processing MSC2409/3202 crypto data"
            );
        }

        if let Err(e) = pool
            .receive_sync_changes(crate::crypto_pool::SyncChanges {
                to_device_events: raw_events,
                changed_devices,
                otk_counts,
                fallback_keys,
                per_user_otk_counts,
                per_user_fallback_keys,
                per_user_to_device,
            })
            .await
        {
            error!(error = %e, "failed to process crypto sync changes");
        }
    }

    let events = body
        .get("events")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    debug!(txn_id, event_count = events.len(), "processing transaction");

    let dispatcher = state.dispatcher.read().await;
    dispatcher.handle_transaction(&events).await;

    // Flush any outgoing crypto requests generated during event processing
    // (e.g., key queries triggered by new encrypted rooms, key claims, etc.).
    if let Some(pool) = &state.crypto_pool
        && let Err(e) = pool.process_all_outgoing_requests().await
    {
        error!(error = %e, "failed to process outgoing crypto requests");
    }

    (StatusCode::OK, Json(json!({})))
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
                if let Ok(counts) = serde_json::from_value::<
                    BTreeMap<ruma::OneTimeKeyAlgorithm, ruma::UInt>,
                >(counts_val.clone())
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
#[allow(clippy::type_complexity)]
fn parse_per_user_fallback_keys(
    raw: &Option<Value>,
) -> (
    Option<Vec<ruma::OneTimeKeyAlgorithm>>,
    HashMap<OwnedUserId, Option<Vec<ruma::OneTimeKeyAlgorithm>>>,
) {
    let mut flat: Option<Vec<ruma::OneTimeKeyAlgorithm>> = None;
    let mut per_user: HashMap<OwnedUserId, Option<Vec<ruma::OneTimeKeyAlgorithm>>> = HashMap::new();

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
            if let Some((_device_id, types_val)) = devices.into_iter().next() {
                let types: Option<Vec<ruma::OneTimeKeyAlgorithm>> =
                    serde_json::from_value(types_val.clone()).ok();
                per_user.insert(user_id.clone(), types);
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

        if let Some(user_id) = to_user
            && let Ok(raw_val) = serde_json::value::to_raw_value(event)
        {
            result
                .entry(user_id)
                .or_default()
                .push(ruma::serde::Raw::from_json(raw_val));
        }
        // Events without to_user_id are dropped in per-user mode
        // (they shouldn't exist per MSC3202).
    }

    result
}
