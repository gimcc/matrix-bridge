# Getting Started

This guide walks you through setting up the Matrix Bridge from scratch -- prerequisites, configuration, registration, and deployment.

---

## Prerequisites

| Requirement | Minimum Version | Notes |
|-------------|----------------|-------|
| Rust | 1.85+ | Only needed when building from source |
| Synapse | 1.149+ | The Matrix homeserver |
| Docker | 24+ | Optional; recommended for production |
| Docker Compose | 2.20+ | Optional; for the all-in-one deployment |

---

## Configuration Reference

The bridge reads its configuration from a TOML file (default path: `config.toml`, overridden by `BRIDGE_CONFIG`).

Below is a complete reference with every field explained.

```toml
# ─── Homeserver ───────────────────────────────────────────────
[homeserver]
# The internal URL the bridge uses to reach Synapse.
# In Docker Compose this is typically the container name.
url = "http://matrix:8008"

# The server_name of your Synapse instance.
# Must match the value in Synapse's homeserver.yaml.
domain = "im.fr.ds.cc"

# ─── Appservice Identity ─────────────────────────────────────
[appservice]
# A unique identifier for this appservice; must match the
# registration YAML that Synapse loads.
id = "unified-bridge"

# The address the bridge listens on. Use 0.0.0.0 inside a
# container so Synapse can reach it.
address = "0.0.0.0"

# TCP port the bridge listens on. Must match the port exposed
# in your Dockerfile / Compose config.
port = 29320

# The localpart of the bridge bot user.
# The full MXID will be @bridge_bot:<domain>.
sender_localpart = "bridge_bot"

# Appservice token -- Synapse sends this to authenticate
# requests TO the bridge. Generate a random string.
as_token = "CHANGE_ME_AS_TOKEN"

# Homeserver token -- the bridge sends this to authenticate
# requests TO Synapse. Generate a random string.
hs_token = "CHANGE_ME_HS_TOKEN"

# ─── Database ─────────────────────────────────────────────────
[database]
# Path to the SQLite database file.
# The directory must be writable by the bridge process.
path = "/data/bridge.db"

# ─── Logging ──────────────────────────────────────────────────
[logging]
# Log level: trace, debug, info, warn, error.
# Can be overridden at runtime with the RUST_LOG env var.
level = "info"

# ─── End-to-End Encryption ────────────────────────────────────
[encryption]
# Whether the bridge accepts encrypted rooms.
allow = true

# Whether the bridge enables encryption in newly created rooms.
default = true

# Enable appservice-mode encryption (recommended).
appservice = true

# Directory where Olm/Megolm session data is persisted.
crypto_store = "/data/crypto"

# Passphrase used to encrypt the crypto store at rest.
# Generate a strong random string and keep it secret.
crypto_store_passphrase = "CHANGE_ME_CRYPTO_PASSPHRASE"

# The display name shown for the bridge's encryption device.
device_display_name = "Matrix Bridge"
```

> **Security note:** Replace every `CHANGE_ME_*` value before running the bridge. Use `openssl rand -hex 32` or a similar tool to generate tokens and passphrases.

---

## Appservice Registration

Synapse needs a registration YAML that tells it about the bridge. Create a file called `registration.yaml`:

```yaml
id: unified-bridge
url: "http://bridge:29320"        # Synapse must be able to reach this
as_token: "CHANGE_ME_AS_TOKEN"    # Must match config.toml
hs_token: "CHANGE_ME_HS_TOKEN"    # Must match config.toml
sender_localpart: bridge_bot
namespaces:
  users:
    - exclusive: true
      regex: "@[a-z]+_.*:.*"      # Puppet user namespace
    - exclusive: true
      regex: "@bridge_bot:.*"     # The bridge bot itself
rate_limited: false
```

### Register with Synapse

1. Place `registration.yaml` where Synapse can read it (e.g. `/data/registration.yaml`).
2. Add the path to Synapse's `homeserver.yaml`:

   ```yaml
   app_service_config_files:
     - /data/registration.yaml
   ```

3. Restart Synapse so it picks up the new registration.

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `BRIDGE_CONFIG` | `config.toml` | Path to the bridge configuration file |
| `BRIDGE_REGISTRATION` | `registration.yaml` | Path to the appservice registration YAML |
| `RUST_LOG` | _(uses `logging.level` from config)_ | Override log level at runtime (e.g. `debug`, `matrix_bridge=trace`) |

---

## Deployment with Docker Compose

The following `docker-compose.yaml` runs Synapse and the bridge together.

```yaml
services:
  # ── Synapse ──────────────────────────────────────────────
  matrix:
    image: matrixdotorg/synapse:latest
    restart: unless-stopped
    volumes:
      - synapse_data:/data
      - ./registration.yaml:/data/registration.yaml:ro
    environment:
      SYNAPSE_SERVER_NAME: im.fr.ds.cc
      SYNAPSE_REPORT_STATS: "no"
    ports:
      - "8008:8008"

  # ── Bridge ───────────────────────────────────────────────
  bridge:
    build: .
    restart: unless-stopped
    depends_on:
      - matrix
    volumes:
      - bridge_data:/data
      - ./config.toml:/data/config.toml:ro
      - ./registration.yaml:/data/registration.yaml:ro
    environment:
      BRIDGE_CONFIG: /data/config.toml
      BRIDGE_REGISTRATION: /data/registration.yaml
      RUST_LOG: info
    ports:
      - "29320:29320"

volumes:
  synapse_data:
  bridge_data:
```

### Quick start

```bash
# 1. Generate tokens
export AS_TOKEN=$(openssl rand -hex 32)
export HS_TOKEN=$(openssl rand -hex 32)
export CRYPTO_PASS=$(openssl rand -hex 32)

# 2. Write config.toml and registration.yaml with the tokens above
#    (replace every CHANGE_ME_* placeholder)

# 3. Launch
docker compose up -d

# 4. Check logs
docker compose logs -f bridge
```

---

## Building from Source

```bash
# Clone the repository
git clone <repo-url> matrix-bridge
cd matrix-bridge

# Build in release mode (Rust 1.85+ required)
cargo build --release

# The binary is at:
#   target/release/matrix-bridge
```

### Running directly

```bash
export BRIDGE_CONFIG=config.toml
export BRIDGE_REGISTRATION=registration.yaml

./target/release/matrix-bridge
```

---

## First Run Checklist

When the bridge starts for the first time, the following happens automatically:

1. **Configuration loaded** -- the bridge reads `BRIDGE_CONFIG` and validates all fields.
2. **Registration loaded** -- the bridge reads `BRIDGE_REGISTRATION` and confirms token consistency with the config.
3. **Database created** -- if the SQLite file at `database.path` does not exist, it is created and migrations are applied.
4. **Crypto store initialized** -- when encryption is enabled, the Olm/Megolm store is created at `encryption.crypto_store` and encrypted with the configured passphrase.
5. **Bot user registered** -- the bridge registers `@bridge_bot:<domain>` on the homeserver via the appservice API (idempotent; safe to restart).
6. **HTTP listener started** -- the bridge begins accepting requests on the configured address and port.

### Verifying everything works

```bash
# The bridge should respond to health checks
curl http://localhost:29320/health

# The bot user should exist on the homeserver
curl "http://localhost:8008/_matrix/client/v3/profile/@bridge_bot:im.fr.ds.cc/displayname"
```

If the bridge fails to start, check:

- Token mismatch between `config.toml` and `registration.yaml`.
- Synapse is unreachable at the configured `homeserver.url`.
- The `/data` directory is not writable.
- The appservice registration was not added to Synapse's `homeserver.yaml`.
