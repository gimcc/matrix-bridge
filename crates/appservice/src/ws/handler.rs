use std::sync::Arc;

use axum::{
    extract::{Query, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::IntoResponse,
};
use secrecy::ExposeSecret;
use tracing::{debug, info, warn};

use crate::auth;
use crate::server::AppState;

use super::{AUTH_TIMEOUT, MAX_WS_CLIENTS, PING_INTERVAL, WsAuthMessage, WsConnectParams};
use super::{is_valid_platform_id, parse_forward_sources};

/// Result type for WebSocket parameter validation.
type WsValidationResult = Result<(String, Vec<String>, Vec<String>), (StatusCode, &'static str)>;

/// Validate WebSocket connection parameters (pre-upgrade, no auth check).
fn validate_ws_params(params: &WsConnectParams) -> WsValidationResult {
    if !is_valid_platform_id(&params.platform) {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid platform: must be 1-64 alphanumeric/dash/underscore/dot characters",
        ));
    }

    let platform = params.platform.clone();
    let forward_sources = parse_forward_sources(params.forward_sources.as_deref());
    let capabilities = parse_forward_sources(params.capabilities.as_deref());

    Ok((platform, forward_sources, capabilities))
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
    let (platform, forward_sources, capabilities) = match validate_ws_params(&params) {
        Ok(v) => v,
        Err((status, msg)) => return (status, msg).into_response(),
    };

    ws.on_upgrade(move |socket| {
        handle_ws_session(state, platform, forward_sources, capabilities, socket)
    })
}

/// Run a WebSocket session: authenticate via first message (if needed),
/// then forward messages from the registry to the client,
/// and handle pings/close from the client side.
async fn handle_ws_session(
    state: Arc<AppState>,
    platform: String,
    forward_sources: Vec<String>,
    capabilities: Vec<String>,
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
                            .filter(|m| {
                                auth::verify_token(&m.access_token, expected.expose_secret())
                            })
                            .is_some();
                    }
                    Message::Close(_) => return false,
                    _ => continue,
                }
            }
            false
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

    // --- Authenticated: register (with atomic limit check) ---
    let (client_id, mut rx) = match state.ws_registry.try_register(
        &platform,
        forward_sources,
        capabilities,
        MAX_WS_CLIENTS,
    ) {
        Ok(v) => v,
        Err(()) => {
            warn!(platform, "ws connection rejected: limit reached");
            let _ = sink
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 4002,
                    reason: "too many connections".into(),
                })))
                .await;
            return;
        }
    };

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
                        None => break,
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

    // Wait for either task to finish, then abort the other.
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
pub(super) mod validation_tests {
    use super::*;
    use crate::ws::WsRegistry;

    fn params(platform: &str, forward: Option<&str>) -> WsConnectParams {
        WsConnectParams {
            platform: platform.to_string(),
            forward_sources: forward.map(|s| s.to_string()),
            capabilities: None,
        }
    }

    #[test]
    fn rejects_invalid_platform() {
        let result = validate_ws_params(&params("has space", None));
        assert_eq!(result.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_empty_platform() {
        let result = validate_ws_params(&params("", None));
        assert_eq!(result.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_when_connection_limit_reached() {
        let registry = WsRegistry::new();
        let _rxs: Vec<_> = (0..MAX_WS_CLIENTS)
            .map(|i| {
                let (_id, rx) = registry
                    .try_register(&format!("p{i}"), vec![], vec![], MAX_WS_CLIENTS)
                    .unwrap();
                rx
            })
            .collect();

        // Next registration should fail atomically.
        assert!(
            registry
                .try_register("telegram", vec![], vec![], MAX_WS_CLIENTS)
                .is_err()
        );
        // Count must stay at MAX_WS_CLIENTS (no leak from failed attempt).
        assert_eq!(registry.total_clients(), MAX_WS_CLIENTS);
    }

    #[test]
    fn accepts_valid_params() {
        let result = validate_ws_params(&params("telegram", Some("*")));
        let (platform, fwd, _caps) = result.unwrap();
        assert_eq!(platform, "telegram");
        assert_eq!(fwd, vec!["*"]);
    }

    #[test]
    fn empty_forward_sources_by_default() {
        let result = validate_ws_params(&params("telegram", None));
        let (_, fwd, _) = result.unwrap();
        assert!(fwd.is_empty());
    }
}
