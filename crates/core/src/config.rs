use std::collections::HashMap;
use std::fmt;

use anyhow::ensure;
use serde::Deserialize;
use tracing::warn;

/// Maximum media file size for uploads and downloads (200 MB).
pub const MAX_MEDIA_SIZE: usize = 200 * 1024 * 1024;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub homeserver: HomeserverConfig,
    pub appservice: AppserviceConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// End-to-bridge encryption configuration.
    #[serde(default)]
    pub encryption: EncryptionConfig,
    /// Access control and permission settings.
    #[serde(default)]
    pub permissions: PermissionsConfig,
    /// Platform-specific configuration sections.
    /// Each key is a platform ID (e.g., "telegram", "discord").
    #[serde(default)]
    pub platforms: HashMap<String, toml::Value>,
}

impl AppConfig {
    /// Validate config invariants after loading. Call before any service starts.
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.homeserver.url.is_empty(),
            "homeserver.url must not be empty"
        );
        ensure!(
            !self.homeserver.domain.is_empty(),
            "homeserver.domain must not be empty"
        );
        ensure!(
            !self.appservice.as_token.is_empty(),
            "appservice.as_token must not be empty"
        );
        ensure!(
            !self.appservice.hs_token.is_empty(),
            "appservice.hs_token must not be empty"
        );
        ensure!(
            !self.appservice.sender_localpart.is_empty(),
            "appservice.sender_localpart must not be empty"
        );
        ensure!(self.appservice.port > 0, "appservice.port must be > 0");
        ensure!(
            ["trace", "debug", "info", "warn", "error"].contains(&self.logging.level.as_str()),
            "logging.level must be one of: trace, debug, info, warn, error"
        );
        for user_id in &self.appservice.auto_invite {
            ensure!(
                crate::platform::is_valid_matrix_user_id(user_id),
                "invalid auto_invite entry '{user_id}': must be a valid Matrix user ID (@localpart:domain)"
            );
        }
        if self.homeserver.url.starts_with("http://") {
            warn!(
                "homeserver.url uses plaintext HTTP — consider using HTTPS for production deployments"
            );
        }
        if self.encryption.allow && self.encryption.crypto_store_passphrase.is_none() {
            warn!(
                "encryption is enabled but crypto_store_passphrase is not set — the crypto store will not be encrypted at rest"
            );
        }
        Ok(())
    }
}

#[derive(Deserialize, Clone)]
pub struct EncryptionConfig {
    /// Enable end-to-bridge encryption.
    #[serde(default)]
    pub allow: bool,
    /// Automatically enable encryption for new portal rooms.
    #[serde(default)]
    pub default: bool,
    /// Use appservice mode (MSC2409/MSC3202) instead of /sync.
    #[serde(default = "default_true")]
    pub appservice: bool,
    /// Path to the crypto store directory.
    #[serde(default = "default_crypto_store")]
    pub crypto_store: String,
    /// Passphrase for encrypting the crypto store.
    #[serde(default)]
    pub crypto_store_passphrase: Option<String>,
    /// Device display name for the bridge bot.
    #[serde(default = "default_device_name")]
    pub device_display_name: String,
    /// Device ID for the bridge bot. Must be alphanumeric/underscore (no spaces).
    #[serde(default = "default_device_id")]
    pub device_id: String,
    /// Enable per-user crypto: each puppet gets its own OlmMachine and device keys.
    /// When false (default), all puppets share the bridge bot's single device (MSC3202 masquerading).
    #[serde(default)]
    pub per_user_crypto: bool,
    /// Prefix for generating deterministic puppet device IDs (per-user crypto mode).
    #[serde(default = "default_puppet_device_prefix")]
    pub puppet_device_prefix: String,
}

impl fmt::Debug for EncryptionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncryptionConfig")
            .field("allow", &self.allow)
            .field("default", &self.default)
            .field("appservice", &self.appservice)
            .field("crypto_store", &self.crypto_store)
            .field(
                "crypto_store_passphrase",
                &self.crypto_store_passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("device_display_name", &self.device_display_name)
            .field("device_id", &self.device_id)
            .field("per_user_crypto", &self.per_user_crypto)
            .field("puppet_device_prefix", &self.puppet_device_prefix)
            .finish()
    }
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            allow: false,
            default: false,
            appservice: true,
            crypto_store: default_crypto_store(),
            crypto_store_passphrase: None,
            device_display_name: default_device_name(),
            device_id: default_device_id(),
            per_user_crypto: false,
            puppet_device_prefix: default_puppet_device_prefix(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_crypto_store() -> String {
    "/data/crypto".to_string()
}

fn default_device_name() -> String {
    "Matrix Bridge".to_string()
}

fn default_device_id() -> String {
    "matrix_bridge".to_string()
}

fn default_puppet_device_prefix() -> String {
    "puppet".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct HomeserverConfig {
    pub url: String,
    pub domain: String,
}

#[derive(Deserialize, Clone)]
pub struct AppserviceConfig {
    pub id: String,
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub sender_localpart: String,
    pub as_token: String,
    pub hs_token: String,
    /// Prefix for puppet user localparts.
    /// Puppet users are registered as `@{prefix}_{platform}_{user_id}:domain`.
    /// Default: `"bot"` → `@bot_telegram_12345:domain`.
    #[serde(default = "default_puppet_prefix")]
    pub puppet_prefix: String,
    /// Optional API key for the Bridge HTTP API (`/api/v1/admin/*` routes).
    /// When set, every Bridge API request must include this key via
    /// `Authorization: Bearer <api_key>` header or `access_token` query param.
    /// When empty (default), the Bridge API requires no authentication —
    /// suitable for internal/trusted-network deployments where access control
    /// is handled by a reverse proxy or network-level middleware.
    ///
    /// This is intentionally separate from `hs_token` (which is a Matrix
    /// protocol secret between the homeserver and the appservice).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Block webhook URLs targeting private/reserved IP ranges (SSRF protection).
    /// When `true`, webhook registration rejects targets on loopback, RFC1918,
    /// link-local, CGNAT, and other non-routable addresses.
    /// Default: `true` (blocks private IPs — set to `false` for internal
    /// deployments where webhook targets are on the same private network).
    #[serde(default = "default_true")]
    pub webhook_ssrf_protection: bool,
    /// Matrix user IDs to auto-invite when the bridge creates a room.
    /// Example: `["@admin:example.com"]`
    /// Without this, auto-created rooms are empty (only the bridge bot).
    #[serde(default)]
    pub auto_invite: Vec<String>,
    /// Allow the Bridge API `invite` field in room creation requests.
    /// When `false` (default), the `invite` field is silently ignored —
    /// only `auto_invite` from config is used. This prevents external
    /// services from inviting arbitrary Matrix users.
    #[serde(default)]
    pub allow_api_invite: bool,
    /// Allow cross-platform relay: forward messages originating from one
    /// external platform (e.g. telegram) to another (e.g. discord) via webhooks
    /// and WebSocket. When `false` (default), only messages from real Matrix
    /// users are forwarded to external platforms. When `true`, per-webhook
    /// `forward_sources` allowlists take effect for cross-platform routing.
    #[serde(default)]
    pub allow_relay: bool,
}

impl fmt::Debug for AppserviceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppserviceConfig")
            .field("id", &self.id)
            .field("address", &self.address)
            .field("port", &self.port)
            .field("sender_localpart", &self.sender_localpart)
            .field("as_token", &"<redacted>")
            .field("hs_token", &"<redacted>")
            .field("puppet_prefix", &self.puppet_prefix)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("webhook_ssrf_protection", &self.webhook_ssrf_protection)
            .field("auto_invite", &self.auto_invite)
            .field("allow_api_invite", &self.allow_api_invite)
            .field("allow_relay", &self.allow_relay)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

fn default_address() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    29320
}

fn default_puppet_prefix() -> String {
    "bot".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Permission level for bridge access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    /// No access — cannot use commands, messages not forwarded.
    None,
    /// Messages forwarded to external platforms. Cannot invite bot or use
    /// bot DM commands. Must be invited by an admin first.
    Relay,
    /// Full access — invite bot + all bot DM commands + message relay.
    Admin,
}

/// Unified access control configuration.
///
/// A single `[permissions]` section controls who can interact with the bridge,
/// via two role lists:
///
/// - `admin`: full access — can invite bot + bot DM commands + message relay.
/// - `relay`: message relay only — messages are forwarded to external platforms,
///   but cannot invite the bot (must be invited by an admin) and cannot use
///   bot DM commands.
///
/// Users not matching either list have no access at all.
/// When both lists are empty, everyone gets admin access (open mode).
///
/// Each list supports:
/// - Exact user ID: `"@admin:example.com"`
/// - Domain wildcard: `"@*:example.com"` (any user on that domain)
/// - Full wildcard: `"*"` (everyone)
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PermissionsConfig {
    /// Matrix user IDs with full admin access.
    #[serde(default)]
    pub admin: Vec<String>,

    /// Matrix user IDs with relay-level access.
    #[serde(default)]
    pub relay: Vec<String>,

    /// Minimum Matrix power level required to have messages forwarded in
    /// bridged rooms. Default: 0 (all room members). Set to e.g. 50 to
    /// restrict forwarding to moderators and above.
    ///
    /// This is checked per-message in bridged rooms and uses the sender's
    /// actual room power level from the homeserver.
    #[serde(default)]
    pub relay_min_power_level: i64,
}

impl PermissionsConfig {
    /// Check if a Matrix user ID is allowed to invite the bridge bot.
    ///
    /// Only admin users can invite. Relay users must be invited by an admin
    /// first. In open mode (both lists empty), everyone can invite.
    /// Puppet users bypass this externally.
    pub fn is_invite_allowed(&self, sender: &str) -> bool {
        self.permission_level(sender) >= PermissionLevel::Admin
    }

    /// Determine the permission level for a given Matrix user ID.
    ///
    /// Resolution order: admin > relay > none.
    /// When both `admin` and `relay` are empty, everyone gets Admin
    /// (backward-compatible open mode).
    pub fn permission_level(&self, sender: &str) -> PermissionLevel {
        if self.admin.is_empty() && self.relay.is_empty() {
            return PermissionLevel::Admin;
        }
        if Self::matches(sender, &self.admin) {
            return PermissionLevel::Admin;
        }
        if Self::matches(sender, &self.relay) {
            return PermissionLevel::Relay;
        }
        PermissionLevel::None
    }

    /// Check if a sender matches any pattern in the list.
    fn matches(sender: &str, patterns: &[String]) -> bool {
        for pattern in patterns {
            if pattern == "*" || pattern == sender {
                return true;
            }
            if let Some(domain_suffix) = pattern.strip_prefix("@*")
                && sender.starts_with('@')
                && sender.ends_with(domain_suffix)
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── open mode (both lists empty) ──

    #[test]
    fn test_open_mode_everyone_is_admin() {
        let p = PermissionsConfig::default();
        assert_eq!(
            p.permission_level("@anyone:example.com"),
            PermissionLevel::Admin
        );
        assert!(p.is_invite_allowed("@anyone:example.com"));
    }

    // ── admin role ──

    #[test]
    fn test_admin_exact() {
        let p = PermissionsConfig {
            admin: vec!["@admin:example.com".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@admin:example.com"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@other:example.com"),
            PermissionLevel::None
        );
    }

    #[test]
    fn test_admin_domain_wildcard() {
        let p = PermissionsConfig {
            admin: vec!["@*:trusted.org".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@alice:trusted.org"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@alice:untrusted.org"),
            PermissionLevel::None
        );
    }

    // ── relay role ──

    #[test]
    fn test_relay_exact() {
        let p = PermissionsConfig {
            relay: vec!["@user:example.com".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@user:example.com"),
            PermissionLevel::Relay
        );
        assert_eq!(
            p.permission_level("@other:example.com"),
            PermissionLevel::None
        );
    }

    #[test]
    fn test_relay_domain_wildcard() {
        let p = PermissionsConfig {
            relay: vec!["@*:trusted.org".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@bob:trusted.org"),
            PermissionLevel::Relay
        );
        assert_eq!(
            p.permission_level("@bob:untrusted.org"),
            PermissionLevel::None
        );
    }

    // ── admin > relay precedence ──

    #[test]
    fn test_admin_over_relay() {
        let p = PermissionsConfig {
            admin: vec!["@admin:example.com".to_string()],
            relay: vec![
                "@admin:example.com".to_string(),
                "@user:example.com".to_string(),
            ],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@admin:example.com"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@user:example.com"),
            PermissionLevel::Relay
        );
        assert_eq!(
            p.permission_level("@nobody:example.com"),
            PermissionLevel::None
        );
    }

    #[test]
    fn test_relay_wildcard_admin_specific() {
        let p = PermissionsConfig {
            admin: vec!["@admin:example.com".to_string()],
            relay: vec!["*".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@admin:example.com"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@anyone:example.com"),
            PermissionLevel::Relay
        );
    }

    // ── invite = admin only ──

    #[test]
    fn test_invite_admin_only() {
        let p = PermissionsConfig {
            admin: vec!["@admin:example.com".to_string()],
            relay: vec!["@user:example.com".to_string()],
            ..Default::default()
        };
        assert!(p.is_invite_allowed("@admin:example.com"));
        assert!(
            !p.is_invite_allowed("@user:example.com"),
            "relay cannot invite"
        );
        assert!(!p.is_invite_allowed("@nobody:example.com"));
    }

    #[test]
    fn test_invite_denied_when_no_role() {
        let p = PermissionsConfig {
            admin: vec!["@admin:a.com".to_string()],
            ..Default::default()
        };
        assert!(p.is_invite_allowed("@admin:a.com"));
        assert!(!p.is_invite_allowed("@other:a.com"));
    }

    // ── admin-only, no relay list ──

    #[test]
    fn test_admin_only_no_relay() {
        let p = PermissionsConfig {
            admin: vec!["@admin:example.com".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@admin:example.com"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@other:example.com"),
            PermissionLevel::None
        );
        assert!(!p.is_invite_allowed("@other:example.com"));
    }

    // ── mixed domain patterns ──

    #[test]
    fn test_mixed_admin_relay_domains() {
        let p = PermissionsConfig {
            admin: vec!["@admin:trusted.org".to_string()],
            relay: vec!["@*:trusted.org".to_string()],
            ..Default::default()
        };
        assert_eq!(
            p.permission_level("@admin:trusted.org"),
            PermissionLevel::Admin
        );
        assert_eq!(
            p.permission_level("@user:trusted.org"),
            PermissionLevel::Relay
        );
        assert_eq!(
            p.permission_level("@user:untrusted.org"),
            PermissionLevel::None
        );
    }
}
