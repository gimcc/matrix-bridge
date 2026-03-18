use serde::Deserialize;
use std::collections::HashMap;

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

#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
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
    /// Optional API key for the Bridge HTTP API (`/api/v1/*` routes).
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
    /// Default: `false` (allow private IPs — suitable for internal deployments
    /// where webhook targets are on the same private network).
    #[serde(default)]
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

/// Access control configuration.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct PermissionsConfig {
    /// Whitelist of Matrix user IDs allowed to invite the bridge bot.
    /// Empty list = allow everyone (open mode, default).
    ///
    /// Supports:
    /// - Exact user ID: `"@admin:example.com"`
    /// - Domain wildcard: `"@*:example.com"` (any user on that domain)
    /// - Full wildcard: `"*"` (same as empty, allow all)
    ///
    /// Puppet users bypass this check — their invites are always accepted
    /// since they are managed by the bridge itself.
    #[serde(default)]
    pub invite_whitelist: Vec<String>,
}

impl PermissionsConfig {
    /// Check if a Matrix user ID is allowed to invite the bridge bot.
    /// Returns true if the whitelist is empty (open mode) or user matches.
    pub fn is_invite_allowed(&self, sender: &str) -> bool {
        if self.invite_whitelist.is_empty() {
            return true;
        }
        for pattern in &self.invite_whitelist {
            if pattern == "*" {
                return true;
            }
            if pattern == sender {
                return true;
            }
            // Domain wildcard: "@*:example.com" matches any "@xxx:example.com"
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

    #[test]
    fn test_empty_whitelist_allows_all() {
        let p = PermissionsConfig::default();
        assert!(p.is_invite_allowed("@anyone:example.com"));
        assert!(p.is_invite_allowed("@evil:attacker.com"));
    }

    #[test]
    fn test_wildcard_allows_all() {
        let p = PermissionsConfig {
            invite_whitelist: vec!["*".to_string()],
        };
        assert!(p.is_invite_allowed("@anyone:example.com"));
    }

    #[test]
    fn test_exact_match() {
        let p = PermissionsConfig {
            invite_whitelist: vec!["@admin:example.com".to_string()],
        };
        assert!(p.is_invite_allowed("@admin:example.com"));
        assert!(!p.is_invite_allowed("@other:example.com"));
        assert!(!p.is_invite_allowed("@admin:other.com"));
    }

    #[test]
    fn test_domain_wildcard() {
        let p = PermissionsConfig {
            invite_whitelist: vec!["@*:trusted.org".to_string()],
        };
        assert!(p.is_invite_allowed("@alice:trusted.org"));
        assert!(p.is_invite_allowed("@bob:trusted.org"));
        assert!(!p.is_invite_allowed("@alice:untrusted.org"));
        assert!(!p.is_invite_allowed("@alice:sub.trusted.org"));
    }

    #[test]
    fn test_multiple_patterns() {
        let p = PermissionsConfig {
            invite_whitelist: vec!["@admin:a.com".to_string(), "@*:b.com".to_string()],
        };
        assert!(p.is_invite_allowed("@admin:a.com"));
        assert!(!p.is_invite_allowed("@user:a.com"));
        assert!(p.is_invite_allowed("@anyone:b.com"));
        assert!(!p.is_invite_allowed("@anyone:c.com"));
    }

    #[test]
    fn test_user_level_same_domain() {
        // Only @b:aa.im is whitelisted, @a:aa.im is NOT.
        let p = PermissionsConfig {
            invite_whitelist: vec!["@b:aa.im".to_string()],
        };
        assert!(p.is_invite_allowed("@b:aa.im"));
        assert!(!p.is_invite_allowed("@a:aa.im"));
        assert!(!p.is_invite_allowed("@c:aa.im"));
        // Different domain also blocked.
        assert!(!p.is_invite_allowed("@b:other.im"));
    }

    #[test]
    fn test_mixed_user_and_domain() {
        // User-level on domain A + domain-wide on domain B.
        let p = PermissionsConfig {
            invite_whitelist: vec!["@b:aa.im".to_string(), "@*:bb.im".to_string()],
        };
        assert!(p.is_invite_allowed("@b:aa.im"));
        assert!(!p.is_invite_allowed("@a:aa.im"));
        assert!(p.is_invite_allowed("@anyone:bb.im"));
        assert!(!p.is_invite_allowed("@x:cc.im"));
    }
}
