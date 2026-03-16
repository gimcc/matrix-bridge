# Getting Started

This guide covers the real startup flow implemented by the current codebase: config generation, registration generation, Synapse wiring, and runtime behavior.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust 1.88+ | Needed only when running from source |
| Synapse | Any appservice-capable homeserver setup |
| SQLite | Embedded; no separate server required |
| Docker / Compose | Optional if you package the bridge in containers |

## Startup Behavior

The binary reads:

| Variable | Default | Purpose |
|----------|---------|---------|
| `BRIDGE_CONFIG` | `config.toml` | Main TOML config |
| `BRIDGE_REGISTRATION` | `registration.yaml` | Generated appservice registration |
| `RUST_LOG` | falls back to `logging.level` | Runtime log filter |

On startup:

1. If `BRIDGE_CONFIG` does not exist, the bridge writes a default config with fresh random tokens and exits.
2. The config is parsed and validated before any network service starts.
3. The SQLite database is opened and migrated.
4. If `registration.yaml` is missing, stale, or explicitly requested via `--generate-registration`, the bridge regenerates it from the current config.
5. The bridge bot is registered on the homeserver.
6. The HTTP server starts on `appservice.address:appservice.port`.

The generated config is written with mode `0600` on Unix.

## Minimal Config

```toml
[homeserver]
url = "http://matrix:8008"
domain = "example.com"

[appservice]
id = "matrix-bridge"
sender_localpart = "bridge_bot"
as_token = "CHANGE_ME_AS_TOKEN"
hs_token = "CHANGE_ME_HS_TOKEN"

[database]
path = "/data/bridge.db"

[logging]
level = "info"

[encryption]
allow = false
default = false
appservice = true
crypto_store = "/data/crypto"

[permissions]
admin = ["@admin:example.com"]
relay = []
relay_min_power_level = 0
```

## Configuration Reference

### `[homeserver]`

| Field | Required | Description |
|-------|----------|-------------|
| `url` | Yes | Base URL the bridge uses to reach Synapse |
| `domain` | Yes | Matrix homeserver domain used for MXIDs |

### `[appservice]`

| Field | Default | Description |
|-------|---------|-------------|
| `id` | none | Appservice ID used in config and registration |
| `address` | `0.0.0.0` | Bind address for the HTTP server |
| `port` | `29320` | Bind port for the HTTP server |
| `sender_localpart` | none | Localpart of the bridge bot user |
| `as_token` | none | Token Synapse uses when calling the bridge |
| `hs_token` | none | Token the bridge uses when talking to Synapse appservice endpoints |
| `puppet_prefix` | `bot` | Prefix for generated puppet MXIDs |
| `api_key` | unset | Optional Bridge API auth key; Bridge API accepts header auth only |
| `webhook_ssrf_protection` | `false` | Reject webhook URLs that target localhost or private/reserved networks |
| `auto_invite` | `[]` | Matrix users auto-invited to bridge-created rooms |
| `allow_api_invite` | `false` | Whether the `invite` field on `POST /api/v1/rooms` is honored |
| `allow_relay` | `false` | Whether messages from one external platform may be forwarded to another |

### `[database]`

| Field | Required | Description |
|-------|----------|-------------|
| `path` | Yes | SQLite database file path |

### `[logging]`

| Field | Default | Description |
|-------|---------|-------------|
| `level` | `info` | One of `trace`, `debug`, `info`, `warn`, `error` |

### `[encryption]`

| Field | Default | Description |
|-------|---------|-------------|
| `allow` | `false` | Enable end-to-bridge encryption support |
| `default` | `false` | Auto-enable encryption on bridge-created rooms |
| `appservice` | `true` | Use appservice-mode crypto handling |
| `crypto_store` | `/data/crypto` | Crypto state directory |
| `crypto_store_passphrase` | unset | Optional passphrase for encrypting the crypto store |
| `device_display_name` | `Matrix Bridge` | Display name for the bridge bot device |
| `device_id` | `matrix_bridge` | Device ID for the bridge bot |
| `per_user_crypto` | `false` | Give each puppet its own crypto device |
| `puppet_device_prefix` | `puppet` | Prefix used to derive per-user device IDs |

### `[permissions]`

| Field | Default | Description |
|-------|---------|-------------|
| `admin` | `[]` | Users with full access: invite and DM commands |
| `relay` | `[]` | Lower-privilege tier used by the permission model; DM commands and invites remain admin-only |
| `relay_min_power_level` | `0` | Minimum room power level required for relaying |

Pattern syntax for `admin` and `relay`:

- Exact user: `@alice:example.com`
- Domain wildcard: `@*:example.com`
- Global wildcard: `*`

If both lists are empty, everyone is treated as admin.

### `[platforms]`

The bridge core accepts arbitrary platform-specific TOML subtrees:

```toml
[platforms.telegram]
bot_username = "my_bot"

[platforms.slack]
workspace = "example"
```

These values are stored in the config model and used to advertise configured platform IDs, but their interpretation is up to your integration layer.

## Registration Generation

Generate the appservice registration explicitly:

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run -- --generate-registration
```

The generated file includes:

- Bridge bot namespace
- Puppet user namespace based on `appservice.puppet_prefix`
- MSC crypto fields when encryption is enabled

Normal startup also verifies that `registration.yaml` still matches the current `as_token`, `hs_token`, and encryption mode. If it does not, startup aborts with an actionable error.

## Synapse Setup

Add the generated registration to `homeserver.yaml`:

```yaml
app_service_config_files:
  - /data/registration.yaml
```

Then restart Synapse.

## Run From Source

```bash
# first boot: writes config if missing
cargo run

# explicit registration generation
cargo run -- --generate-registration

# normal runtime
cargo run --release
```

Useful developer commands:

```bash
just fmt
just test
just check
```

## Example Container Deployment

The repository does not ship a ready-made `docker-compose.yml`, but the following example matches the current runtime behavior:

```yaml
services:
  bridge:
    build: .
    restart: unless-stopped
    environment:
      BRIDGE_CONFIG: /data/config.toml
      BRIDGE_REGISTRATION: /data/registration.yaml
      RUST_LOG: info
    volumes:
      - ./data:/data
    ports:
      - "29320:29320"
```

You still need to mount the generated `registration.yaml` into the Synapse container and reference it from `homeserver.yaml`.

## First API Smoke Test

```bash
curl http://localhost:29320/health

curl http://localhost:29320/api/v1/admin/info \
  -H "Authorization: Bearer <api_key>"
```

Skip the `Authorization` header when `appservice.api_key` is unset.
