use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::BridgeError;
use crate::message::{BridgeMessage, ExternalRoom};

/// Callback for incoming messages from external platforms.
pub type IncomingHandler = Arc<
    dyn Fn(BridgeMessage) -> Pin<Box<dyn Future<Output = Result<(), BridgeError>> + Send>>
        + Send
        + Sync,
>;

/// The main trait that every bridge platform must implement.
///
/// Each platform (Telegram, Discord, WhatsApp, etc.) provides a concrete
/// implementation that handles connecting to the external service and
/// converting messages bidirectionally.
#[async_trait]
pub trait BridgePlatform: Send + Sync + 'static {
    /// Unique platform identifier (e.g., "telegram", "discord").
    fn id(&self) -> &str;

    /// User regex pattern for the appservice registration namespace.
    /// Example: `@telegram_.*:example.com`
    fn user_namespace_regex(&self) -> String;

    /// Alias regex pattern for the appservice registration namespace.
    /// Example: `#telegram_.*:example.com`
    fn alias_namespace_regex(&self) -> Option<String>;

    /// Initialize the platform connection (login, websocket, polling, etc.)
    async fn start(&self) -> Result<(), BridgeError>;

    /// Graceful shutdown.
    async fn stop(&self) -> Result<(), BridgeError>;

    /// Called when a message arrives from Matrix destined for this platform.
    /// Returns the external message ID on success.
    async fn send_to_platform(&self, message: &BridgeMessage) -> Result<String, BridgeError>;

    /// Send a typing indicator to the external platform.
    async fn send_typing(&self, room: &ExternalRoom, is_typing: bool) -> Result<(), BridgeError>;

    /// Send a read receipt to the external platform.
    async fn send_read_receipt(
        &self,
        room: &ExternalRoom,
        message_id: &str,
    ) -> Result<(), BridgeError>;

    /// Register the callback for incoming messages from this platform.
    /// The bridge core calls this during startup to wire up the message flow.
    async fn set_incoming_handler(&self, handler: IncomingHandler) -> Result<(), BridgeError>;
}

/// Build a puppet user localpart from prefix, platform, and external user ID.
///
/// Format: `{prefix}_{platform}_{user_id}` → e.g. `bot_telegram_12345`
pub fn puppet_localpart(prefix: &str, platform: &str, user_id: &str) -> String {
    format!("{prefix}_{platform}_{user_id}")
}

/// Extract the source platform from a puppet user's Matrix ID.
///
/// Given `@bot_telegram_12345:domain` with prefix `"bot"`, returns `Some("telegram")`.
/// Returns `None` for non-puppet users.
pub fn puppet_source_platform(sender: &str, prefix: &str) -> Option<String> {
    let localpart = sender
        .strip_prefix('@')
        .and_then(|s| s.split(':').next())
        .unwrap_or("");

    // Strip the prefix + underscore separator.
    let rest = localpart.strip_prefix(prefix)?.strip_prefix('_')?;

    // The remainder is `{platform}_{user_id}`.
    let pos = rest.find('_')?;
    let platform = &rest[..pos];
    let user_part = &rest[pos + 1..];

    if !platform.is_empty()
        && platform.bytes().all(|b| b.is_ascii_lowercase())
        && !user_part.is_empty()
    {
        Some(platform.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_puppet_localpart() {
        assert_eq!(
            puppet_localpart("bot", "telegram", "12345"),
            "bot_telegram_12345"
        );
        assert_eq!(puppet_localpart("bot", "slack", "U001"), "bot_slack_U001");
    }

    #[test]
    fn test_puppet_source_platform() {
        assert_eq!(
            puppet_source_platform("@bot_telegram_12345:example.com", "bot"),
            Some("telegram".to_string())
        );
        assert_eq!(
            puppet_source_platform("@bot_slack_U001:example.com", "bot"),
            Some("slack".to_string())
        );
        // Real Matrix user — no prefix match.
        assert_eq!(puppet_source_platform("@alice:example.com", "bot"), None);
        // Wrong prefix.
        assert_eq!(
            puppet_source_platform("@other_telegram_12345:example.com", "bot"),
            None
        );
        // Bridge bot itself (no platform part after prefix).
        assert_eq!(puppet_source_platform("@bot:example.com", "bot"), None);
    }
}
