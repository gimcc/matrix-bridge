mod attachment_crypto;
mod bot_commands;
mod commands;
mod crypto_helpers;
mod matrix_content;
mod matrix_events;
mod media_proxy;
mod outbound;
mod platform_events;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// Matrix user IDs to auto-invite when creating portal rooms.
    pub(super) auto_invite: Vec<String>,
    /// Whether cross-platform relay is enabled (platform A -> platform B).
    pub(super) allow_relay: bool,
    /// Cache of (room_id, puppet_user_id) pairs that have already joined.
    pub(super) room_membership: DashSet<(String, String)>,
    /// Set to true when the homeserver rejects Space creation, disabling
    /// further attempts for the lifetime of this Dispatcher.
    spaces_unsupported: AtomicBool,
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
        auto_invite: Vec<String>,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;

        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10));

        if ssrf_protection {
            builder = builder.dns_resolver(Arc::new(SafeDnsResolver::new()));
        }

        let http_client = builder.build().context("failed to build HTTP client")?;

        // Media client always has SSRF protection — URLs come from untrusted
        // external platforms, unlike webhooks which are operator-configured.
        let media_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(10))
            .dns_resolver(Arc::new(SafeDnsResolver::new()))
            .build()
            .context("failed to build media HTTP client")?;

        Ok(Self {
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
            auto_invite,
            allow_relay,
            room_membership: DashSet::new(),
            spaces_unsupported: AtomicBool::new(false),
        })
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

    /// Ensure a platform Space exists and add the room as a child.
    ///
    /// Creates the Space on first use for a given platform, then caches
    /// the mapping in the database. Best-effort: errors are logged but
    /// do not prevent the room from functioning.
    ///
    /// If the homeserver rejects Space creation (e.g. unsupported room type),
    /// the `spaces_unsupported` flag is set and all future calls are no-ops.
    pub async fn ensure_platform_space(&self, platform_id: &str, room_id: &str) {
        // Fast path: homeserver doesn't support Spaces.
        if self.spaces_unsupported.load(Ordering::Relaxed) {
            return;
        }

        let space_id = match self.db.get_platform_space(platform_id).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                // Create a new Space for this platform.
                let name = format!("{platform_id} (bridge)");
                let topic = format!("Bridged rooms from {platform_id}");
                match self.matrix_client.create_space(&name, Some(&topic)).await {
                    Ok(id) => {
                        if let Err(e) = self.db.set_platform_space(platform_id, &id).await {
                            tracing::error!(
                                platform = platform_id,
                                space_id = %id,
                                error = %e,
                                "failed to persist platform space mapping"
                            );
                        }
                        tracing::info!(
                            platform = platform_id,
                            space_id = %id,
                            "created platform space"
                        );
                        id
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        // Detect unsupported Space errors and disable future attempts.
                        // Common indicators: M_UNKNOWN (unrecognised room type),
                        // M_UNRECOGNIZED, or "m.space" appearing in the error body.
                        if err_str.contains("M_UNKNOWN")
                            || err_str.contains("M_UNRECOGNIZED")
                            || err_str.contains("m.space")
                        {
                            tracing::warn!(
                                "homeserver does not support Spaces — disabling space organization"
                            );
                            self.spaces_unsupported.store(true, Ordering::Relaxed);
                        } else {
                            tracing::warn!(
                                platform = platform_id,
                                error = %e,
                                "failed to create platform space — room will not be organized"
                            );
                        }
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    platform = platform_id,
                    error = %e,
                    "failed to query platform space"
                );
                return;
            }
        };

        // Add the room as a child of the space.
        if let Err(e) = self.matrix_client.set_space_child(&space_id, room_id).await {
            tracing::warn!(
                platform = platform_id,
                space_id,
                room_id,
                error = %e,
                "failed to add room to platform space"
            );
        }
    }
}
