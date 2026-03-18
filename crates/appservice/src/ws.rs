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

/// Maximum number of entries in `forward_sources`.
const MAX_FORWARD_SOURCES: usize = 20;

/// Maximum length for each `forward_sources` entry.
const MAX_FORWARD_SOURCE_LEN: usize = 64;

/// Timeout for the client to send the auth message after connecting.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// A connected WebSocket client.
struct WsClient {
    id: String,
    sender: mpsc::Sender<String>,
    forward_sources: Vec<String>,
}

impl WsClient {
    /// Check if messages from the given source platform should be forwarded.
    fn should_forward_source(&self, source_platform: &str) -> bool {
        if self.forward_sources.is_empty() {
            return false;
        }
        if self.forward_sources.iter().any(|s| s == "*") {
            return true;
        }
        self.forward_sources.iter().any(|s| s == source_platform)
    }
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

impl Default for WsRegistry {
    fn default() -> Self {
        Self {
            clients: DashMap::new(),
            count: AtomicUsize::new(0),
        }
    }
}

impl WsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new client for the given platform.
    /// Returns `(client_id, receiver)` — the receiver yields JSON payloads.
    fn register(
        &self,
        platform_id: &str,
        forward_sources: Vec<String>,
    ) -> (String, mpsc::Receiver<String>) {
        let id = ulid::Ulid::new().to_string();
        let (tx, rx) = mpsc::channel(CLIENT_CHANNEL_CAPACITY);

        let client = WsClient {
            id: id.clone(),
            sender: tx,
            forward_sources,
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
    /// Only delivers to clients whose `forward_sources` allowlist includes
    /// `source_platform`. Uses `try_send` to avoid blocking on slow consumers.
    ///
    /// Uses a read lock for iteration (sending) and only acquires a write lock
    /// when dead clients need to be cleaned up, reducing contention with
    /// concurrent register/unregister operations on the same shard.
    pub fn broadcast(&self, platform_id: &str, payload: &str, source_platform: Option<&str>) {
        let effective_source = source_platform.unwrap_or("matrix");

        // Phase 1: read lock — iterate and send, collect dead client IDs.
        let closed_ids = {
            let Some(entry) = self.clients.get(platform_id) else {
                return;
            };

            let mut closed = Vec::new();

            for client in entry.iter() {
                if !client.should_forward_source(effective_source) {
                    continue;
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
                        closed.push(client.id.clone());
                    }
                }
            }

            closed
        };
        // Read lock dropped here.

        // Phase 2: write lock — remove dead clients (only if needed).
        if !closed_ids.is_empty()
            && let Some(mut entry) = self.clients.get_mut(platform_id)
        {
            let before = entry.len();
            entry.retain(|c| !closed_ids.contains(&c.id));
            let removed = before - entry.len();
            if removed > 0 {
                self.count.fetch_sub(removed, Ordering::Relaxed);
            }
        }
    }

    /// Total number of connected WebSocket clients across all platforms.
    pub fn total_clients(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// Query parameters for the WebSocket upgrade request.
///
/// Authentication is NOT done via query params to avoid leaking tokens in
/// server/proxy logs. Instead, when `api_key` is configured the client must
/// send `{"access_token":"<key>"}` as the first WebSocket message after
/// connecting (within [`AUTH_TIMEOUT`]).
#[derive(Debug, Deserialize)]
pub struct WsConnectParams {
    /// Platform ID to subscribe to (required).
    pub platform: String,
    /// Comma-separated allowlist of source platform IDs to forward.
    /// Empty = deny all (default), "*" = forward all.
    pub forward_sources: Option<String>,
}

/// First message the client must send when `api_key` is configured.
#[derive(Debug, Deserialize)]
struct WsAuthMessage {
    access_token: String,
}

/// Validate that a platform ID contains only allowed characters and is bounded.
fn is_valid_platform_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_PLATFORM_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// Parse `forward_sources` from a comma-separated string.
fn parse_forward_sources(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= MAX_FORWARD_SOURCE_LEN)
        .take(MAX_FORWARD_SOURCES)
        .collect()
}

/// Validate WebSocket connection parameters (pre-upgrade, no auth check).
///
/// Returns `Ok((platform, forward_sources))` on success, or `Err((status, message))`
/// on validation failure. Auth is handled post-upgrade via the first message.
fn validate_ws_params(
    params: &WsConnectParams,
    state: &AppState,
) -> Result<(String, Vec<String>), (StatusCode, &'static str)> {
    if !is_valid_platform_id(&params.platform) {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid platform: must be 1-64 alphanumeric/dash/underscore/dot characters",
        ));
    }

    if state.ws_registry.total_clients() >= MAX_WS_CLIENTS {
        return Err((StatusCode::TOO_MANY_REQUESTS, "too many ws connections"));
    }

    let platform = params.platform.clone();
    let forward_sources = parse_forward_sources(params.forward_sources.as_deref());

    Ok((platform, forward_sources))
}

/// GET /api/v1/ws?platform=xxx&forward_sources=*
///
/// Upgrades the HTTP connection to a WebSocket for real-time message delivery.
///
/// When `api_key` is configured, the client must send a JSON auth message
/// as the first frame after connecting: `{"access_token":"<key>"}`.
/// The server will close the connection if the token is missing, invalid,
/// or not received within [`AUTH_TIMEOUT`].
pub async fn handle_ws_upgrade(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WsConnectParams>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let (platform, forward_sources) = match validate_ws_params(&params, &state) {
        Ok(v) => v,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    ws.on_upgrade(move |socket| handle_ws_session(state, platform, forward_sources, socket))
}

/// Run a WebSocket session: authenticate via first message (if needed),
/// then forward messages from the registry to the client,
/// and handle pings/close from the client side.
async fn handle_ws_session(
    state: Arc<AppState>,
    platform: String,
    forward_sources: Vec<String>,
    socket: axum::extract::ws::WebSocket,
) {
    use futures_util::SinkExt;
    use futures_util::StreamExt;

    let (mut sink, mut stream) = socket.split();

    // --- Authentication via first message ---
    if let Some(ref expected) = state.api_key {
        let auth_result = tokio::time::timeout(AUTH_TIMEOUT, async {
            while let Some(Ok(msg)) = stream.next().await {
                match msg {
                    Message::Text(text) => {
                        return serde_json::from_str::<WsAuthMessage>(&text)
                            .ok()
                            .filter(|m| auth::verify_token(&m.access_token, expected))
                            .is_some();
                    }
                    Message::Close(_) => return false,
                    // Skip ping/pong/binary frames while waiting for auth.
                    _ => continue,
                }
            }
            false // stream ended without auth
        })
        .await;

        let authenticated = matches!(auth_result, Ok(true));
        if !authenticated {
            let reason = if auth_result.is_err() {
                "auth timeout"
            } else {
                "invalid or missing access_token"
            };
            warn!(platform, reason, "ws auth failed, closing connection");
            let _ = sink
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4001,
                    reason: reason.into(),
                })))
                .await;
            return;
        }
    }

    // --- Authenticated: register and start message relay ---
    let (client_id, mut rx) = state.ws_registry.register(&platform, forward_sources);

    info!(
        client_id,
        platform,
        total = state.ws_registry.total_clients(),
        "ws client connected"
    );

    // Outbound: registry -> client (messages + periodic ping).
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

    // Inbound: client -> server (only handle Close/Pong, ignore others).
    let mut inbound = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Close(_) = msg {
                break;
            }
        }
    });

    // Wait for either task to finish, then abort the other to prevent leaks.
    tokio::select! {
        result = &mut outbound => {
            if let Err(e) = result {
                warn!(client_id = client_id.as_str(), platform = platform.as_str(), "ws outbound task panicked: {e}");
            }
            inbound.abort();
        }
        result = &mut inbound => {
            if let Err(e) = result {
                warn!(client_id = client_id.as_str(), platform = platform.as_str(), "ws inbound task panicked: {e}");
            }
            outbound.abort();
        }
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
        let (_id1, mut rx1) = registry.register("telegram", vec!["*".to_string()]);
        let (_id2, mut rx2) = registry.register("telegram", vec!["*".to_string()]);
        let (_id3, mut rx3) = registry.register("slack", vec!["*".to_string()]);

        registry.broadcast("telegram", r#"{"event":"message"}"#, None);

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
        assert!(rx3.try_recv().is_err()); // slack client should not receive
    }

    #[test]
    fn test_broadcast_forward_sources() {
        let registry = WsRegistry::new();
        // Client 1: only forwards "slack" -> should NOT receive "matrix"
        let (_id1, mut rx1) = registry.register("telegram", vec!["slack".to_string()]);
        // Client 2: forwards all -> should receive "matrix"
        let (_id2, mut rx2) = registry.register("telegram", vec!["*".to_string()]);
        // Client 3: empty forward_sources -> should NOT receive anything
        let (_id3, mut rx3) = registry.register("telegram", vec![]);

        registry.broadcast("telegram", r#"{"event":"message"}"#, Some("matrix"));

        assert!(rx1.try_recv().is_err()); // matrix not in allowlist
        assert!(rx2.try_recv().is_ok()); // wildcard allows all
        assert!(rx3.try_recv().is_err()); // empty = deny all
    }

    #[test]
    fn test_broadcast_no_source_defaults_to_matrix() {
        let registry = WsRegistry::new();
        let (_id1, mut rx1) = registry.register("telegram", vec!["matrix".to_string()]);
        let (_id2, mut rx2) = registry.register("telegram", vec!["slack".to_string()]);

        // No source_platform -> treated as "matrix"
        registry.broadcast("telegram", r#"{"event":"message"}"#, None);

        assert!(rx1.try_recv().is_ok()); // "matrix" in allowlist
        assert!(rx2.try_recv().is_err()); // "matrix" not in allowlist
    }

    #[test]
    fn test_slow_consumer_does_not_block() {
        let registry = WsRegistry::new();
        let (_id, _rx) = registry.register("test", vec!["*".to_string()]);

        // Fill the channel beyond capacity — should not panic or block.
        for i in 0..CLIENT_CHANNEL_CAPACITY + 10 {
            registry.broadcast("test", &format!(r#"{{"n":{i}}}"#), None);
        }

        assert_eq!(registry.total_clients(), 1);
    }

    #[test]
    fn test_closed_client_is_cleaned_up() {
        let registry = WsRegistry::new();
        let (id, rx) = registry.register("test", vec!["*".to_string()]);
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

    #[test]
    fn test_default_trait() {
        let registry = WsRegistry::default();
        assert_eq!(registry.total_clients(), 0);
    }

    #[test]
    fn test_parse_forward_sources() {
        assert_eq!(parse_forward_sources(None), Vec::<String>::new());
        assert_eq!(parse_forward_sources(Some("")), Vec::<String>::new());
        assert_eq!(
            parse_forward_sources(Some("matrix, slack")),
            vec!["matrix", "slack"]
        );
        assert_eq!(parse_forward_sources(Some("*")), vec!["*"]);

        // Oversized entries are filtered out.
        let long = "x".repeat(MAX_FORWARD_SOURCE_LEN + 1);
        let input = format!("ok,,{long},valid");
        assert_eq!(parse_forward_sources(Some(&input)), vec!["ok", "valid"]);
    }

    // --- Validation tests ---
    //
    // Test the `validate_ws_params` function directly, which covers the
    // 400/429 rejection logic without needing a real TCP/WebSocket
    // connection (axum's `WebSocketUpgrade` extractor requires one).
    // Auth is now handled post-upgrade via the first message.

    mod validation_tests {
        use super::*;
        use crate::dispatcher::Dispatcher;
        use crate::matrix_client::MatrixClient;
        use crate::puppet_manager::PuppetManager;

        /// Build a minimal AppState with a real (but dummy) Dispatcher.
        fn test_state(api_key: Option<String>, ws_registry: Arc<WsRegistry>) -> AppState {
            let client = MatrixClient::new("http://localhost:0", "test_token", "localhost");
            let db = matrix_bridge_store::Database::open_in_memory().expect("in-memory db");
            let puppet_mgr = Arc::new(PuppetManager::new(client.clone(), db.clone(), None));
            let dispatcher = Dispatcher::new(
                puppet_mgr,
                client,
                db,
                "localhost",
                "bridge",
                "bot",
                Default::default(),
                ws_registry.clone(),
            );

            AppState {
                dispatcher: Arc::new(tokio::sync::Mutex::new(dispatcher)),
                processed_txns: tokio::sync::Mutex::new(indexmap::IndexSet::new()),
                crypto_pool: None,
                webhook_ssrf_protection: false,
                auto_invite: vec![],
                allow_api_invite: false,
                encryption_default: false,
                bridge_info: crate::server::BridgeInfo {
                    homeserver_url: String::new(),
                    homeserver_domain: String::new(),
                    bot_user_id: String::new(),
                    puppet_prefix: String::new(),
                    encryption_enabled: false,
                    encryption_default: false,
                    webhook_ssrf_protection: false,
                    api_key_required: api_key.is_some(),
                    configured_platforms: vec![],
                    invite_whitelist: vec![],
                },
                ws_registry,
                api_key,
            }
        }

        fn params(platform: &str, forward: Option<&str>) -> WsConnectParams {
            WsConnectParams {
                platform: platform.to_string(),
                forward_sources: forward.map(|s| s.to_string()),
            }
        }

        #[test]
        fn rejects_invalid_platform() {
            let state = test_state(None, Arc::new(WsRegistry::new()));
            let result = validate_ws_params(&params("has space", None), &state);
            assert_eq!(result.unwrap_err().0, StatusCode::BAD_REQUEST);
        }

        #[test]
        fn rejects_empty_platform() {
            let state = test_state(None, Arc::new(WsRegistry::new()));
            let result = validate_ws_params(&params("", None), &state);
            assert_eq!(result.unwrap_err().0, StatusCode::BAD_REQUEST);
        }

        #[test]
        fn rejects_when_connection_limit_reached() {
            let registry = Arc::new(WsRegistry::new());
            let _rxs: Vec<_> = (0..MAX_WS_CLIENTS)
                .map(|i| {
                    let (_id, rx) = registry.register(&format!("p{i}"), vec![]);
                    rx
                })
                .collect();

            let state = test_state(None, registry);
            let result = validate_ws_params(&params("telegram", None), &state);
            assert_eq!(result.unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        }

        #[test]
        fn accepts_valid_params() {
            let state = test_state(None, Arc::new(WsRegistry::new()));
            let result = validate_ws_params(&params("telegram", Some("*")), &state);
            let (platform, fwd) = result.unwrap();
            assert_eq!(platform, "telegram");
            assert_eq!(fwd, vec!["*"]);
        }

        #[test]
        fn empty_forward_sources_by_default() {
            let state = test_state(None, Arc::new(WsRegistry::new()));
            let result = validate_ws_params(&params("telegram", None), &state);
            let (_, fwd) = result.unwrap();
            assert!(fwd.is_empty());
        }
    }
}
