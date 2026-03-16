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
