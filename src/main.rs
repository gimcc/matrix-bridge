use std::sync::Arc;

use anyhow::Context;
use tokio::sync::Mutex;
use tracing::info;

use matrix_bridge_appservice::crypto_manager::CryptoManager;
use matrix_bridge_appservice::dispatcher::Dispatcher;
use matrix_bridge_appservice::matrix_client::MatrixClient;
use matrix_bridge_appservice::puppet_manager::PuppetManager;
use matrix_bridge_appservice::server::{self, AppState};
use matrix_bridge_core::config::AppConfig;
use matrix_bridge_core::registration;
use matrix_bridge_store::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config.
    let config_path = std::env::var("BRIDGE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());

    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {config_path}"))?;

    let config: AppConfig =
        toml::from_str(&config_str).with_context(|| "failed to parse config")?;

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
    );

    // Set device_id for MSC3202 device masquerading if encryption is enabled.
    if config.encryption.allow {
        matrix_client.set_device_id(
            &config.encryption.device_id,
            &config.appservice.sender_localpart,
        );
    }

    // Create puppet manager.
    let puppet_manager = Arc::new(PuppetManager::new(matrix_client.clone(), db.clone()));

    // Generate registration YAML if requested.
    if std::env::args().any(|a| a == "--generate-registration") {
        // Use a single regex based on the puppet prefix: @bot_.*:.*
        // This matches all puppet users regardless of platform.
        let prefix = &config.appservice.puppet_prefix;
        let user_regexes = vec![format!("@{prefix}_.*:.*")];

        let reg = registration::build_registration(
            &config.appservice,
            user_regexes,
            vec![],
            config.encryption.allow,
        );
        let yaml = registration::to_yaml(&reg)?;
        let reg_path = std::env::var("BRIDGE_REGISTRATION")
            .unwrap_or_else(|_| "registration.yaml".to_string());
        std::fs::write(&reg_path, &yaml)?;
        info!(path = reg_path, "registration YAML written");
        return Ok(());
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

    // Initialize CryptoManager if encryption is enabled.
    let crypto_manager = if config.encryption.allow {
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

        // Upload initial device keys.
        cm.process_outgoing_requests().await?;
        info!("end-to-bridge encryption enabled");

        Some(Arc::new(cm))
    } else {
        None
    };

    // Create dispatcher.
    let mut dispatcher = Dispatcher::new(
        puppet_manager,
        matrix_client,
        db,
        &config.homeserver.domain,
        &config.appservice.sender_localpart,
        &config.appservice.puppet_prefix,
        config.permissions.clone(),
    );

    // Wire up crypto manager to dispatcher for outbound encryption.
    if let Some(ref cm) = crypto_manager {
        dispatcher.set_crypto(Arc::clone(cm), config.encryption.default);
    }

    // Build app state.
    let state = Arc::new(AppState {
        dispatcher: Arc::new(Mutex::new(dispatcher)),
        processed_txns: Mutex::new(indexmap::IndexSet::new()),
        crypto_manager,
        webhook_ssrf_protection: config.appservice.webhook_ssrf_protection,
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
        config.appservice.hs_token.clone(),
        config.appservice.api_key.clone(),
    )
    .await?;

    Ok(())
}
