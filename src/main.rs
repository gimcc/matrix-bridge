use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::RwLock;
use tracing::info;

use matrix_bridge_appservice::crypto_manager::CryptoManager;
use matrix_bridge_appservice::crypto_pool::CryptoManagerPool;
use matrix_bridge_appservice::dispatcher::Dispatcher;
use matrix_bridge_appservice::matrix_client::MatrixClient;
use matrix_bridge_appservice::puppet_manager::PuppetManager;
use matrix_bridge_appservice::server::{self, AppState, BridgeInfo};
use matrix_bridge_appservice::ws::WsRegistry;
use matrix_bridge_core::config::AppConfig;
use matrix_bridge_core::registration;
use matrix_bridge_store::Database;
use secrecy::SecretString;

/// Generate a URL-safe random token (32 bytes, base64).
fn generate_token() -> anyhow::Result<String> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf)
        .map_err(|e| anyhow::anyhow!("failed to generate random bytes: {e}"))?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// Write a default config.toml with freshly generated tokens.
fn write_default_config(path: &str) -> anyhow::Result<()> {
    let as_token = generate_token()?;
    let hs_token = generate_token()?;
    let crypto_passphrase = generate_token()?;

    let content = format!(
        r#"[homeserver]
url = "http://matrix:8008"
domain = "example.com"      # Your Matrix server domain

[appservice]
id = "matrix-bridge"
sender_localpart = "bridge_bot"
as_token = "{as_token}"
hs_token = "{hs_token}"
# api_key = ""              # Uncomment to require auth on Bridge API
# webhook_ssrf_protection = true
# Matrix users to auto-invite into rooms created by the bridge.
# auto_invite = ["@admin:example.com"]
# allow_api_invite = false   # Whether API callers can invite arbitrary users

[permissions]
# admin = ["@admin:example.com"]   # Full access to all bot commands + can invite
# relay = ["@*:example.com"]       # Messages forwarded in bridged rooms
# relay_min_power_level = 0        # Min room power level to relay (0 = everyone)
# Both empty = open mode (everyone is admin)

[database]
path = "/data/bridge.db"

[logging]
level = "info"

[encryption]
allow = true
default = true
appservice = true
crypto_store = "/data/crypto"
crypto_store_passphrase = "{crypto_passphrase}"
"#
    );

    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config — auto-generate a default if the file does not exist.
    let config_path = std::env::var("BRIDGE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());

    if !Path::new(&config_path).exists() {
        write_default_config(&config_path)?;
        eprintln!("Generated default config at {config_path} — review and restart.");
        eprintln!("At minimum, set [homeserver].domain to your Matrix server domain.");
        return Ok(());
    }

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {config_path}"))?;

    let config: AppConfig =
        toml::from_str(&config_str).with_context(|| "failed to parse config")?;

    // Validate config before any service starts.
    config.validate().with_context(|| "invalid configuration")?;

    // Initialize logging.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.logging.level));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    info!("starting matrix-bridge");

    // Open database.
    let db = Database::open(&config.database.path)
        .with_context(|| format!("failed to open database: {}", config.database.path))?;
    db.migrate().await?;

    // Create Matrix client.
    let mut matrix_client = MatrixClient::new(
        &config.homeserver.url,
        &config.appservice.as_token,
        &config.homeserver.domain,
    )?;

    // Set device_id for MSC3202 device masquerading if encryption is enabled.
    if config.encryption.allow {
        matrix_client.set_device_id(
            &config.encryption.device_id,
            &config.appservice.sender_localpart,
        );
    }

    // Create puppet manager.
    // In per-user crypto mode, puppets get their own devices (managed by CryptoManagerPool),
    // so we don't pass the bridge bot's device_id.
    // In single-device mode, puppets share the bridge bot's device for MSC3202 masquerading.
    let puppet_device_id = if config.encryption.allow && !config.encryption.per_user_crypto {
        Some(config.encryption.device_id.clone())
    } else {
        None
    };
    let puppet_manager = Arc::new(PuppetManager::new(
        matrix_client.clone(),
        db.clone(),
        puppet_device_id,
    ));

    // Manage registration YAML: generate if missing, regenerate if stale,
    // and verify token consistency.
    let reg_path =
        std::env::var("BRIDGE_REGISTRATION").unwrap_or_else(|_| "registration.yaml".to_string());
    let force_generate = std::env::args().any(|a| a == "--generate-registration");

    let needs_generate = if !Path::new(&reg_path).exists() {
        true
    } else if !force_generate {
        // Check if the existing registration matches the current config.
        let reg_str = std::fs::read_to_string(&reg_path)
            .with_context(|| format!("failed to read registration: {reg_path}"))?;
        let reg_yaml: serde_yaml::Value =
            serde_yaml::from_str(&reg_str).with_context(|| "failed to parse registration YAML")?;

        let reg_as = reg_yaml.get("as_token").and_then(serde_yaml::Value::as_str);
        let reg_hs = reg_yaml.get("hs_token").and_then(serde_yaml::Value::as_str);

        // Token mismatch or absence is a fatal error — user must resolve manually.
        match reg_as {
            Some(val) if val != config.appservice.as_token => {
                anyhow::bail!(
                    "as_token mismatch: config.toml and {reg_path} have different as_token values. \
                     Update one to match the other, or delete {reg_path} to regenerate."
                );
            }
            None => {
                anyhow::bail!("as_token missing in {reg_path}. Delete {reg_path} to regenerate.");
            }
            _ => {}
        }
        match reg_hs {
            Some(val) if val != config.appservice.hs_token => {
                anyhow::bail!(
                    "hs_token mismatch: config.toml and {reg_path} have different hs_token values. \
                     Update one to match the other, or delete {reg_path} to regenerate."
                );
            }
            None => {
                anyhow::bail!("hs_token missing in {reg_path}. Delete {reg_path} to regenerate.");
            }
            _ => {}
        }

        // Detect encryption config drift: if config enables encryption but
        // registration is missing MSC fields (or vice versa), regenerate.
        let has_msc3202 = reg_yaml.get("de.sorunome.msc3202").is_some();
        let has_push_ephemeral = reg_yaml.get("de.sorunome.msc2409.push_ephemeral").is_some();
        if config.encryption.allow != has_msc3202 || config.encryption.allow != has_push_ephemeral {
            info!("encryption config changed, regenerating registration");
            true
        } else {
            false
        }
    } else {
        true // force_generate
    };

    if needs_generate || force_generate {
        let prefix = &config.appservice.puppet_prefix;
        let user_regexes = vec![format!("@{prefix}_.*:.*")];

        let reg = registration::build_registration(
            &config.appservice,
            user_regexes,
            vec![],
            config.encryption.allow,
        );
        let yaml = registration::to_yaml(&reg)?;
        std::fs::write(&reg_path, &yaml)?;
        info!(path = reg_path, "registration YAML generated");

        if force_generate {
            return Ok(());
        }
    } else {
        info!("registration verified");
    }

    // Register the bridge bot user on the homeserver (idempotent).
    // When encryption is enabled, pass device_id so the device is created during registration.
    if config.encryption.allow {
        matrix_client
            .register_puppet_with_device(
                &config.appservice.sender_localpart,
                Some(&config.encryption.device_id),
            )
            .await
            .with_context(|| "failed to register bridge bot user with device")?;
        info!(
            localpart = config.appservice.sender_localpart,
            device_id = config.encryption.device_id,
            "bridge bot user registered with device"
        );
    } else {
        matrix_client
            .register_puppet(&config.appservice.sender_localpart)
            .await
            .with_context(|| "failed to register bridge bot user")?;
        info!(
            localpart = config.appservice.sender_localpart,
            "bridge bot user registered"
        );
    }

    // Initialize CryptoManagerPool if encryption is enabled.
    let crypto_pool = if config.encryption.allow {
        let bot_user_id: ruma::OwnedUserId = format!(
            "@{}:{}",
            config.appservice.sender_localpart, config.homeserver.domain
        )
        .try_into()
        .with_context(|| "invalid bot user ID")?;

        let device_id: ruma::OwnedDeviceId = config.encryption.device_id.clone().into();

        let cm = CryptoManager::new(
            &bot_user_id,
            &device_id,
            &config.encryption,
            matrix_client.clone(),
        )
        .await
        .with_context(|| "failed to initialize crypto manager")?;

        let pool = CryptoManagerPool::new(
            Arc::new(cm),
            matrix_client.clone(),
            &config.encryption.crypto_store,
            config.encryption.crypto_store_passphrase.as_deref(),
            &config.encryption.puppet_device_prefix,
            config.encryption.per_user_crypto,
            &config.appservice.puppet_prefix,
        );

        if config.encryption.per_user_crypto {
            info!("end-to-bridge encryption enabled (per-user crypto mode)");
        } else {
            info!("end-to-bridge encryption enabled (single-device mode)");
        }

        Some(Arc::new(pool))
    } else {
        None
    };

    // Create WebSocket registry.
    let ws_registry = Arc::new(WsRegistry::new());

    // Create dispatcher.
    let mut dispatcher = Dispatcher::new(
        puppet_manager,
        matrix_client,
        db,
        &config.homeserver.domain,
        &config.appservice.sender_localpart,
        &config.appservice.puppet_prefix,
        config.permissions.clone(),
        Arc::clone(&ws_registry),
        config.appservice.webhook_ssrf_protection,
        config.appservice.allow_relay,
        config.appservice.auto_invite.clone(),
    )
    .with_context(|| "failed to create dispatcher")?;

    // Wire up crypto pool to dispatcher for outbound encryption.
    if let Some(ref pool) = crypto_pool {
        dispatcher.set_crypto(Arc::clone(pool), config.encryption.default);
    }

    // Build bridge info (non-sensitive config for the info API).
    let bridge_info = BridgeInfo {
        homeserver_url: config.homeserver.url.clone(),
        homeserver_domain: config.homeserver.domain.clone(),
        bot_user_id: format!(
            "@{}:{}",
            config.appservice.sender_localpart, config.homeserver.domain
        ),
        puppet_prefix: config.appservice.puppet_prefix.clone(),
        encryption_enabled: config.encryption.allow,
        encryption_default: config.encryption.default,
        webhook_ssrf_protection: config.appservice.webhook_ssrf_protection,
        api_key_required: config
            .appservice
            .api_key
            .as_ref()
            .is_some_and(|k| !k.is_empty()),
        configured_platforms: config.platforms.keys().cloned().collect(),
        admin_users: config.permissions.admin.clone(),
        relay_users: config.permissions.relay.clone(),
    };

    // Build app state.
    let state = Arc::new(AppState {
        dispatcher: Arc::new(RwLock::new(dispatcher)),
        processed_txns: tokio::sync::Mutex::new(indexmap::IndexSet::new()),
        crypto_pool,
        webhook_ssrf_protection: config.appservice.webhook_ssrf_protection,
        auto_invite: config.appservice.auto_invite.clone(),
        allow_api_invite: config.appservice.allow_api_invite,
        encryption_default: config.encryption.default,
        bridge_info,
        ws_registry,
        api_key: config
            .appservice
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .map(SecretString::from),
    });

    // Start HTTP server.
    info!(
        address = config.appservice.address,
        port = config.appservice.port,
        "starting appservice server"
    );

    server::run_server(
        state,
        &config.appservice.address,
        config.appservice.port,
        SecretString::from(config.appservice.hs_token.clone()),
        config.appservice.api_key.clone().map(SecretString::from),
    )
    .await?;

    Ok(())
}
