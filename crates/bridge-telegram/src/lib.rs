use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};

use matrix_bridge_core::error::BridgeError;
use matrix_bridge_core::message::{BridgeMessage, ExternalRoom};
use matrix_bridge_core::platform::{BridgePlatform, IncomingHandler};

/// Telegram bridge platform implementation (stub).
///
/// This is a skeleton that demonstrates the BridgePlatform trait.
/// A real implementation would connect to the Telegram Bot API
/// or use grammers-client for user-mode bridging.
pub struct TelegramPlatform {
    incoming_handler: Mutex<Option<IncomingHandler>>,
}

impl TelegramPlatform {
    pub fn new() -> Self {
        Self {
            incoming_handler: Mutex::new(None),
        }
    }
}

#[async_trait]
impl BridgePlatform for TelegramPlatform {
    fn id(&self) -> &str {
        "telegram"
    }

    fn user_namespace_regex(&self) -> String {
        "@telegram_.*".to_string()
    }

    fn alias_namespace_regex(&self) -> Option<String> {
        Some("#telegram_.*".to_string())
    }

    async fn start(&self) -> Result<(), BridgeError> {
        info!("telegram platform started (stub)");
        Ok(())
    }

    async fn stop(&self) -> Result<(), BridgeError> {
        info!("telegram platform stopped");
        Ok(())
    }

    async fn send_to_platform(&self, message: &BridgeMessage) -> Result<String, BridgeError> {
        warn!("telegram send_to_platform not implemented");
        Ok(format!("tg_stub_{}", &message.id))
    }

    async fn send_typing(&self, _room: &ExternalRoom, _is_typing: bool) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn send_read_receipt(
        &self,
        _room: &ExternalRoom,
        _message_id: &str,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn set_incoming_handler(&self, handler: IncomingHandler) -> Result<(), BridgeError> {
        let mut h = self.incoming_handler.lock().await;
        *h = Some(handler);
        Ok(())
    }
}
