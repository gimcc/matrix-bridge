mod homeserver;
mod transaction;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::Extension,
    middleware,
    routing::{get, put},
};
use indexmap::IndexSet;
use serde::Serialize;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tracing::info;

use axum::{Json, http::StatusCode, response::IntoResponse};
use secrecy::{ExposeSecret, SecretString};

use crate::auth::{ApiKey, HsToken, verify_api_key, verify_hs_token};
use crate::bridge_api;
use crate::crypto_pool::CryptoManagerPool;
use crate::dispatcher::Dispatcher;
use crate::ws::{self, WsRegistry};

pub(crate) use homeserver::*;
pub(crate) use transaction::*;

/// Maximum number of transaction IDs to keep for deduplication.
pub(crate) const MAX_PROCESSED_TXNS: usize = 10_000;

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
    pub admin_users: Vec<String>,
    pub relay_users: Vec<String>,
}

/// Shared application state for the axum server.
pub struct AppState {
    pub dispatcher: Arc<RwLock<Dispatcher>>,
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
    /// Registry of active WebSocket connections.
    pub ws_registry: Arc<WsRegistry>,
    /// Optional API key for the Bridge HTTP API (cached for WS auth).
    pub api_key: Option<SecretString>,
}

/// Build the axum Router for the appservice HTTP endpoints.
///
/// - `hs_token`: Matrix protocol shared secret (Synapse <-> appservice). Always
///   required on `/_matrix/app/v1/*` routes.
/// - `api_key`: optional API key for the Bridge HTTP API (`/api/v1/admin/*`). When
///   `Some`, every Bridge API request must carry this key. When `None`, the
///   Bridge API is unauthenticated — suitable for internal/trusted-network
///   deployments where access control is handled externally.
pub fn build_router(
    state: Arc<AppState>,
    hs_token: SecretString,
    api_key: Option<SecretString>,
) -> Router {
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

    // Rate limit per IP for bridge API.
    let governor_config = Arc::new({
        // Safety: per_second and burst_size are valid positive constants.
        #[allow(clippy::unwrap_used)]
        GovernorConfigBuilder::default()
            .per_second(120)
            .burst_size(300)
            .finish()
            .expect("valid governor rate limit config")
    });

    // Bridge HTTP API endpoints (for external platform services).
    // Authentication is separate from hs_token — uses an independent api_key.
    // Normalize empty string to None so `api_key = ""` behaves as unauthenticated.
    let api_key = api_key.filter(|k| !k.expose_secret().is_empty());
    let governor_layer = GovernorLayer::new(governor_config);
    let bridge_routes = if let Some(key) = api_key {
        bridge_api::build_bridge_api_router()
            .layer(governor_layer)
            .layer(middleware::from_fn(verify_api_key))
            .layer(Extension(ApiKey(key)))
    } else {
        bridge_api::build_bridge_api_router().layer(governor_layer)
    };

    // WebSocket endpoint — authenticates via query param, not middleware.
    let ws_route = Router::new().route("/api/v1/ws", get(ws::handle_ws_upgrade));

    // Merge all routes under one state.
    matrix_routes
        .merge(bridge_routes)
        .merge(ws_route)
        .route("/health", get(handle_health))
        .with_state(state)
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
    hs_token: SecretString,
    api_key: Option<SecretString>,
) -> anyhow::Result<()> {
    let app = build_router(state, hs_token, api_key);
    let bind_addr = format!("{address}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!(bind_addr, "appservice HTTP server listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    info!("server shut down gracefully");
    Ok(())
}

/// Wait for SIGTERM or SIGINT (Ctrl-C) to trigger graceful shutdown.
async fn shutdown_signal() {
    use anyhow::Context;

    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .context("failed to install Ctrl+C handler")
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to install SIGTERM handler")?
            .recv()
            .await;
        Ok::<(), anyhow::Error>(())
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<anyhow::Result<()>>();

    tokio::select! {
        result = ctrl_c => {
            if let Err(err) = result {
                tracing::error!(?err, "Ctrl+C handler failed");
            }
            info!("received SIGINT, shutting down");
        }
        result = terminate => {
            if let Err(err) = result {
                tracing::error!(?err, "SIGTERM handler failed");
            }
            info!("received SIGTERM, shutting down");
        }
    }
}
