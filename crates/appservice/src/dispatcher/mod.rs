mod attachment_crypto;
mod commands;
mod crypto_helpers;
mod matrix_content;
mod matrix_events;
mod media_proxy;
mod outbound;
mod platform_events;

use std::sync::Arc;

use dashmap::DashSet;
use matrix_bridge_core::config::PermissionsConfig;
use matrix_bridge_store::Database;

use crate::crypto_pool::CryptoManagerPool;
use crate::dns_resolver::SafeDnsResolver;
use crate::matrix_client::MatrixClient;
use crate::puppet_manager::PuppetManager;
use crate::ws::WsRegistry;

/// Routes events between Matrix and external platforms.
///
/// - Matrix -> Platform: Receives Matrix room events, looks up the room mapping,
///   and forwards to registered webhooks for each platform.
/// - Platform -> Matrix: Receives BridgeMessages from the HTTP bridge API,
///   ensures puppets exist, and sends messages to Matrix rooms.
pub struct Dispatcher {
    pub(super) puppet_manager: Arc<PuppetManager>,
    pub(super) matrix_client: MatrixClient,
    pub(super) db: Database,
    /// The bridge bot's full Matrix user ID (e.g. `@bridge_bot:example.com`).
    pub(super) bot_user_id: String,
    /// Prefix for puppet user localparts (e.g. `"bot"`).
    pub(super) puppet_prefix: String,
    /// Precomputed `"@{puppet_prefix}_"` for fast starts_with checks.
    pub(super) puppet_user_prefix: String,
    /// Shared HTTP client for webhook delivery (reuses connection pool).
    /// SSRF protection is configurable (operator-supplied URLs).
    pub(super) http_client: reqwest::Client,
    /// HTTP client for downloading external media from bridged messages.
    /// Always uses SafeDnsResolver because URLs originate from untrusted sources.
    pub(super) media_client: reqwest::Client,
    /// WebSocket client registry for real-time message delivery.
    pub(super) ws_registry: Arc<WsRegistry>,
    /// Optional crypto manager pool for encrypting outbound messages.
    pub(super) crypto_pool: Option<Arc<CryptoManagerPool>>,
    /// Whether to auto-enable encryption for rooms on link.
    pub(super) encryption_default: bool,
    /// Permission settings (invite whitelist, etc.).
    pub(super) permissions: PermissionsConfig,
    /// Whether cross-platform relay is enabled (platform A -> platform B).
    pub(super) allow_relay: bool,
    /// Cache of (room_id, puppet_user_id) pairs that have already joined.
    pub(super) room_membership: DashSet<(String, String)>,
}

impl Dispatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        puppet_manager: Arc<PuppetManager>,
        matrix_client: MatrixClient,
        db: Database,
        server_name: &str,
        sender_localpart: &str,
        puppet_prefix: &str,
        permissions: PermissionsConfig,
        ws_registry: Arc<WsRegistry>,
        ssrf_protection: bool,
        allow_relay: bool,
    ) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10));

        if ssrf_protection {
            builder = builder.dns_resolver(Arc::new(SafeDnsResolver::new()));
        }

        let http_client = builder.build().expect("failed to build HTTP client");

        // Media client always has SSRF protection — URLs come from untrusted
        // external platforms, unlike webhooks which are operator-configured.
        let media_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(10))
            .dns_resolver(Arc::new(SafeDnsResolver::new()))
            .build()
            .expect("failed to build media HTTP client");

        Self {
            puppet_manager,
            matrix_client,
            db,
            bot_user_id: format!("@{sender_localpart}:{server_name}"),
            puppet_prefix: puppet_prefix.to_string(),
            puppet_user_prefix: format!("@{puppet_prefix}_"),
            http_client,
            media_client,
            ws_registry,
            crypto_pool: None,
            encryption_default: false,
            permissions,
            allow_relay,
            room_membership: DashSet::new(),
        }
    }

    /// Set the crypto manager pool for E2BE encryption.
    pub fn set_crypto(&mut self, pool: Arc<CryptoManagerPool>, encryption_default: bool) {
        self.crypto_pool = Some(pool);
        self.encryption_default = encryption_default;
    }

    /// Get a reference to the database (used by bridge_api).
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get a reference to the Matrix client (used by bridge_api for uploads).
    pub fn matrix_client(&self) -> &MatrixClient {
        &self.matrix_client
    }

    /// Check if a sender is the bridge bot itself.
    fn is_bridge_bot(&self, sender: &str) -> bool {
        sender == self.bot_user_id
    }
}

