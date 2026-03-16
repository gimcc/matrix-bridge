# Matrix Bridge

A generic, bidirectional Matrix bridge for HTTP-based integrations.

中文文档入口：[README.zh.md](README.zh.md).

External services talk to the bridge over REST, webhook callbacks, or WebSocket. The bridge handles Matrix appservice plumbing, puppet users, room mappings, optional E2BE, and cross-platform fan-out.

## Features

- Platform-agnostic Bridge API for any HTTP-capable service
- Bidirectional delivery: external -> Matrix and Matrix -> external
- Puppet users named as `@{prefix}_{platform}_{id}:domain`
- Optional room auto-creation and platform space auto-organization
- Optional cross-platform relay through a shared Matrix room
- Webhooks and WebSocket clients can declare capabilities
- Bot DM commands for platform inspection and command passthrough
- Admin / relay permission model with room power-level gating
- Optional end-to-bridge encryption with single-device or per-user crypto
- Rich content support: text, notice, emote, image, file, video, audio, reaction, edit, redaction, location
- Media upload endpoint backed by the Matrix media repository
- Input sanitization, webhook SSRF protection, and Bridge API rate limiting

## Quick Start

### 1. Generate a default config

On first start, the binary writes a default `config.toml` and exits if the file does not exist.

```bash
BRIDGE_CONFIG=/data/config.toml cargo run
```

Edit the generated file before restarting. At minimum, set:

- `homeserver.url`
- `homeserver.domain`
- `appservice.as_token`
- `appservice.hs_token`

### 2. Generate the appservice registration

The bridge can generate `registration.yaml` directly from `config.toml`.

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run -- --generate-registration
```

Add the generated file to Synapse:

```yaml
app_service_config_files:
  - /data/registration.yaml
```

### 3. Run the bridge

```bash
BRIDGE_CONFIG=/data/config.toml \
BRIDGE_REGISTRATION=/data/registration.yaml \
cargo run --release
```

If `registration.yaml` is missing on normal startup, the bridge generates it automatically. If the tokens in `config.toml` and `registration.yaml` drift, startup fails with a clear mismatch error instead of silently continuing.

## Example API Flow

If `appservice.api_key` is configured, include `Authorization: Bearer <api_key>` on Bridge API requests.

```bash
# Create or reuse a Matrix room mapping
curl -X POST http://localhost:29320/api/v1/rooms \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myapp",
    "external_room_id": "general",
    "room_name": "My App / General"
  }'

# Register a webhook receiver
curl -X POST http://localhost:29320/api/v1/webhooks \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myapp",
    "url": "https://myapp.example.com/webhook",
    "events": "message,redaction",
    "capabilities": ["message", "image", "reaction", "command"]
  }'

# Send a message from the external platform to Matrix
curl -X POST http://localhost:29320/api/v1/message \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myapp",
    "room_id": "general",
    "sender": {"id": "alice", "display_name": "Alice"},
    "content": {"type": "text", "body": "Hello from external"}
  }'
```

## Permissions

```toml
[permissions]
admin = ["@admin:example.com"]       # bot commands + invite + relay
relay = ["@*:trusted.example"]       # relay only
relay_min_power_level = 0            # minimum room power level for relaying
```

When both lists are empty, the bridge runs in open mode and everyone is treated as admin.

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/getting-started.md) | Setup, configuration, registration, and running |
| [Integration Guide](docs/integration-guide.md) | How an external service should integrate |
| [API Reference](docs/api-reference.md) | Public Bridge API, WebSocket, payload formats |
| [Architecture](docs/architecture.md) | Runtime model, storage, routing, permissions |
| [Encryption](docs/encryption.md) | E2BE implementation and crypto modes |
| [Examples](examples/README.md) | Demo integrations and example clients |
| [E2E Tests](tests/e2e/README.md) | Bun-based end-to-end test suite |

## Chinese README

[README.zh.md](README.zh.md) provides the Chinese entry point and links to the Chinese sub-documents.

## Development

```bash
just fmt
just test
just check
```

## Project Structure

```text
crates/
├── core/          # Shared config, message types, registration helpers, ID sanitization
├── store/         # SQLite schema and CRUD for mappings, puppets, webhooks, spaces
└── appservice/    # HTTP server, dispatcher, Matrix client, WS registry, crypto runtime
src/main.rs        # Startup, config loading, registration management
docs/              # Project documentation
examples/          # Demo integrations and clients
tests/e2e/         # Bun-based end-to-end tests
```

## License

Apache-2.0
