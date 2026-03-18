use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{
    extract::{Query, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::IntoResponse,
};
use dashmap::DashMap;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::auth;
use crate::server::AppState;

/// Bounded channel capacity per WebSocket client.
const CLIENT_CHANNEL_CAPACITY: usize = 64;

/// Interval between server-sent ping frames.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum total WebSocket connections across all platforms.
const MAX_WS_CLIENTS: usize = 1000;

/// Maximum length for a platform ID.
const MAX_PLATFORM_ID_LEN: usize = 64;

/// Maximum number of entries in `exclude_sources`.
const MAX_EXCLUDE_SOURCES: usize = 20;

/// Maximum length for each `exclude_sources` entry.
const MAX_EXCLUDE_SOURCE_LEN: usize = 64;

/// A connected WebSocket client.
struct WsClient {
    id: String,
    sender: mpsc::Sender<String>,
    exclude_sources: Vec<String>,
}

/// Registry of active WebSocket connections, keyed by platform ID.
///
/// Uses `DashMap` with per-shard locking for concurrent access — safe to
/// call from the Dispatcher while holding its own lock.
pub struct WsRegistry {
    clients: DashMap<String, Vec<WsClient>>,
    /// Atomic counter for fast total_clients() without iterating the map.
    count: AtomicUsize,
}

impl WsRegistry {
    pub fn new() -> Self {
        Self {
            clients: DashMap::new(),
            count: AtomicUsize::new(0),
        }
    }

    /// Register a new client for the given platform.
    /// Returns `(client_id, receiver)` — the receiver yields JSON payloads.
    fn register(
        &self,
        platform_id: &str,
        exclude_sources: Vec<String>,
    ) -> (String, mpsc::Receiver<String>) {
        let id = ulid::Ulid::new().to_string();
        let (tx, rx) = mpsc::channel(CLIENT_CHANNEL_CAPACITY);

        let client = WsClient {
            id: id.clone(),
            sender: tx,
            exclude_sources,
        };

        self.clients
            .entry(platform_id.to_string())
            .or_default()
            .push(client);
        self.count.fetch_add(1, Ordering::Relaxed);

        (id, rx)
    }

    /// Remove a client from the registry.
    fn unregister(&self, platform_id: &str, client_id: &str) {
        if let Some(mut entry) = self.clients.get_mut(platform_id) {
            let before = entry.len();
            entry.retain(|c| c.id != client_id);
            let removed = before - entry.len();
            if removed > 0 {
                self.count.fetch_sub(removed, Ordering::Relaxed);
            }
            if entry.is_empty() {
                drop(entry);
                self.clients.remove(platform_id);
            }
        }
    }

    /// Broadcast a JSON payload to all clients subscribed to the given platform.
    ///
    /// Skips clients whose `exclude_sources` contains `source_platform`.
    /// Uses `try_send` to avoid blocking on slow consumers.
    pub fn broadcast(&self, platform_id: &str, payload: &str, source_platform: Option<&str>) {
        let Some(mut entry) = self.clients.get_mut(platform_id) else {
            return;
        };

        let mut closed_ids = Vec::new();

        for client in entry.iter() {
            if let Some(src) = source_platform {
                if client.exclude_sources.iter().any(|s| s == src) {
                    continue;
                }
            }
            match client.sender.try_send(payload.to_string()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        client_id = client.id,
                        platform = platform_id,
                        "ws client channel full, dropping message"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    closed_ids.push(client.id.clone());
                }
            }
        }

        if !closed_ids.is_empty() {
            let removed = closed_ids.len();
            entry.retain(|c| !closed_ids.contains(&c.id));
            self.count.fetch_sub(removed, Ordering::Relaxed);
        }
    }

    /// Total number of connected WebSocket clients across all platforms.
    pub fn total_clients(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// Query parameters for the WebSocket upgrade request.
#[derive(Debug, Deserialize)]
pub struct WsConnectParams {
    /// Platform ID to subscribe to (required).
    pub platform: String,
    /// API key for authentication (required if `api_key` is configured).
    pub access_token: Option<String>,
    /// Comma-separated platform IDs whose messages should be excluded.
    pub exclude_sources: Option<String>,
}

/// Validate that a platform ID contains only allowed characters and is bounded.
fn is_valid_platform_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_PLATFORM_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// GET /api/v1/ws?platform=xxx&access_token=yyy&exclude_sources=a,b
///
/// Upgrades the HTTP connection to a WebSocket for real-time message delivery.
pub async fn handle_ws_upgrade(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WsConnectParams>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate platform parameter.
    if !is_valid_platform_id(&params.platform) {
        return (
            StatusCode::BAD_REQUEST,
            "invalid platform: must be 1-64 alphanumeric/dash/underscore/dot characters",
        )
            .into_response();
    }

    // Enforce global connection limit.
    if state.ws_registry.total_clients() >= MAX_WS_CLIENTS {
        return (StatusCode::TOO_MANY_REQUESTS, "too many ws connections").into_response();
    }

    // Authenticate if api_key is configured.
    if let Some(ref expected) = state.api_key {
        match &params.access_token {
            Some(token) if auth::verify_token(token, expected) => {}
            Some(_) => return (StatusCode::FORBIDDEN, "invalid access_token").into_response(),
            None => return (StatusCode::UNAUTHORIZED, "missing access_token").into_response(),
        }
    }

    let platform = params.platform.clone();
    let exclude_sources: Vec<String> = params
        .exclude_sources
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= MAX_EXCLUDE_SOURCE_LEN)
        .take(MAX_EXCLUDE_SOURCES)
        .collect();

    ws.on_upgrade(move |socket| handle_ws_session(state, platform, exclude_sources, socket))
}

/// Run a WebSocket session: forward messages from the registry to the client,
/// and handle pings/close from the client side.
async fn handle_ws_session(
    state: Arc<AppState>,
    platform: String,
    exclude_sources: Vec<String>,
    socket: axum::extract::ws::WebSocket,
) {
    let (client_id, mut rx) = state
        .ws_registry
        .register(&platform, exclude_sources);

    info!(
        client_id,
        platform,
        total = state.ws_registry.total_clients(),
        "ws client connected"
    );

    let (mut sink, mut stream) = socket.split();

    use futures_util::SinkExt;
    use futures_util::StreamExt;

    // Outbound: registry → client (messages + periodic ping).
    let platform_clone = platform.clone();
    let client_id_clone = client_id.clone();
    let mut outbound = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(payload) => {
                            if sink.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break, // channel closed
                    }
                }
                _ = ping_interval.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }

        debug!(
            client_id = client_id_clone,
            platform = platform_clone,
            "ws outbound task ended"
        );
    });

    // Inbound: client → server (only handle Close/Pong, ignore others).
    let mut inbound = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Close(_) => break,
                _ => {} // Pong is auto-handled by axum; ignore text/binary from client.
            }
        }
    });

    // Wait for either task to finish, then abort the other to prevent leaks.
    tokio::select! {
        _ = &mut outbound => { inbound.abort(); }
        _ = &mut inbound => { outbound.abort(); }
    }

    state.ws_registry.unregister(&platform, &client_id);
    info!(
        client_id,
        platform,
        total = state.ws_registry.total_clients(),
        "ws client disconnected"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_unregister() {
        let registry = WsRegistry::new();
        let (id, _rx) = registry.register("telegram", vec![]);
        assert_eq!(registry.total_clients(), 1);

        registry.unregister("telegram", &id);
        assert_eq!(registry.total_clients(), 0);
    }

    #[test]
    fn test_broadcast_delivers_to_matching_platform() {
        let registry = WsRegistry::new();
        let (_id1, mut rx1) = registry.register("telegram", vec![]);
        let (_id2, mut rx2) = registry.register("telegram", vec![]);
        let (_id3, mut rx3) = registry.register("slack", vec![]);

        registry.broadcast("telegram", r#"{"event":"message"}"#, None);

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
        assert!(rx3.try_recv().is_err()); // slack client should not receive
    }

    #[test]
    fn test_broadcast_exclude_sources() {
        let registry = WsRegistry::new();
        let (_id1, mut rx1) = registry.register("telegram", vec!["matrix".to_string()]);
        let (_id2, mut rx2) = registry.register("telegram", vec![]);

        registry.broadcast("telegram", r#"{"event":"message"}"#, Some("matrix"));

        assert!(rx1.try_recv().is_err()); // excluded
        assert!(rx2.try_recv().is_ok()); // not excluded
    }

    #[test]
    fn test_slow_consumer_does_not_block() {
        let registry = WsRegistry::new();
        let (_id, _rx) = registry.register("test", vec![]);

        // Fill the channel beyond capacity — should not panic or block.
        for i in 0..CLIENT_CHANNEL_CAPACITY + 10 {
            registry.broadcast("test", &format!(r#"{{"n":{i}}}"#), None);
        }

        assert_eq!(registry.total_clients(), 1);
    }

    #[test]
    fn test_closed_client_is_cleaned_up() {
        let registry = WsRegistry::new();
        let (id, rx) = registry.register("test", vec![]);
        assert_eq!(registry.total_clients(), 1);

        // Drop the receiver to simulate a disconnected client.
        drop(rx);

        // Broadcast should detect the closed channel and remove the client.
        registry.broadcast("test", r#"{"event":"cleanup"}"#, None);
        assert_eq!(registry.total_clients(), 0);
        // Idempotent unregister should not panic.
        registry.unregister("test", &id);
    }

    #[test]
    fn test_valid_platform_id() {
        assert!(is_valid_platform_id("telegram"));
        assert!(is_valid_platform_id("my-app_v2"));
        assert!(is_valid_platform_id("a.b.c"));
        assert!(!is_valid_platform_id(""));
        assert!(!is_valid_platform_id("has space"));
        assert!(!is_valid_platform_id("has/slash"));
        assert!(!is_valid_platform_id(&"x".repeat(65)));
    }

    #[test]
    fn test_total_clients_atomic_counter() {
        let registry = WsRegistry::new();
        let (_id1, _rx1) = registry.register("a", vec![]);
        let (_id2, _rx2) = registry.register("b", vec![]);
        let (id3, _rx3) = registry.register("a", vec![]);
        assert_eq!(registry.total_clients(), 3);

        registry.unregister("a", &id3);
        assert_eq!(registry.total_clients(), 2);
    }
}
