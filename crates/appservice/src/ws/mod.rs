mod handler;
mod registry;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::time::Duration;

use serde::Deserialize;

pub use self::handler::handle_ws_upgrade;
pub use self::registry::WsRegistry;

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
