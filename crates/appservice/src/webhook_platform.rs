use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use matrix_bridge_core::error::BridgeError;
use matrix_bridge_core::message::{BridgeMessage, ExternalRoom};
use matrix_bridge_core::platform::{BridgePlatform, IncomingHandler};
use matrix_bridge_store::Database;

/// A generic webhook-based bridge platform.
///
/// Instead of connecting to a specific service, this platform delivers
/// outbound messages (Matrix → external) by POSTing to registered webhook URLs.
/// Inbound messages (external → Matrix) come via the HTTP bridge API.
///
/// This allows any external service to bridge with Matrix by:
/// 1. Registering a webhook URL via POST /api/v1/webhooks
/// 2. Sending messages via POST /api/v1/message
/// 3. Receiving Matrix messages at their webhook URL
pub struct WebhookPlatform {
    platform_id: String,
    db: Database,
    client: Client,
    incoming_handler: Mutex<Option<IncomingHandler>>,
}

impl WebhookPlatform {
    pub fn new(platform_id: &str, db: Database) -> Self {
        Self {
            platform_id: platform_id.to_string(),
            db,
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("failed to build HTTP client"),
            incoming_handler: Mutex::new(None),
        }
    }

    /// Deliver a message to all registered webhooks for this platform.
    async fn deliver_to_webhooks(&self, message: &BridgeMessage) -> Result<(), BridgeError> {
        let webhooks = self
            .db
            .list_webhooks(&self.platform_id)
            .await
            .map_err(|e| BridgeError::Store(e.to_string()))?;

        if webhooks.is_empty() {
            debug!(
                platform = self.platform_id,
                "no webhooks registered, skipping delivery"
            );
            return Ok(());
        }

        let payload = json!({
            "event": "message",
            "platform": self.platform_id,
            "message": message,
        });

        for webhook in &webhooks {
            match self
                .client
                .post(&webhook.webhook_url)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        debug!(
                            platform = self.platform_id,
                            url = webhook.webhook_url,
                            "webhook delivered"
                        );
                    } else {
                        warn!(
                            platform = self.platform_id,
                            url = webhook.webhook_url,
                            status = %resp.status(),
                            "webhook delivery got non-2xx response"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        platform = self.platform_id,
                        url = webhook.webhook_url,
                        "webhook delivery failed: {e}"
                    );
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl BridgePlatform for WebhookPlatform {
    fn id(&self) -> &str {
        &self.platform_id
    }

    fn user_namespace_regex(&self) -> String {
        format!("@{}_.*", self.platform_id)
    }

    fn alias_namespace_regex(&self) -> Option<String> {
        Some(format!("#{}_.*", self.platform_id))
    }

    async fn start(&self) -> Result<(), BridgeError> {
        info!(platform = self.platform_id, "webhook platform started");
        Ok(())
    }

    async fn stop(&self) -> Result<(), BridgeError> {
        info!(platform = self.platform_id, "webhook platform stopped");
        Ok(())
    }

    async fn send_to_platform(&self, message: &BridgeMessage) -> Result<String, BridgeError> {
        self.deliver_to_webhooks(message).await?;
        // Return a synthetic external message ID.
        Ok(format!("wh_{}_{}", self.platform_id, &message.id))
    }

    async fn send_typing(&self, _room: &ExternalRoom, _is_typing: bool) -> Result<(), BridgeError> {
        // Optional: could deliver typing events via webhooks too.
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
