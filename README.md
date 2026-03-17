# Matrix Bridge

A generic, bidirectional Matrix bridge. Any service with HTTP can bridge to Matrix — no platform-specific code required.

通用双向 Matrix 桥接服务。任何支持 HTTP 的服务都可以接入 Matrix，无需编写平台特定代码。

```
                       ┌──────────────────────┐
  External App ──────► │   Matrix Bridge      │ ──────► Matrix Room
  POST /api/v1/message │                      │   (puppet user sends)
                       │  ┌────────────────┐  │
  External App ◄────── │  │ Appservice API │  │ ◄────── Matrix Room
  Webhook callback     │  │ Puppet Manager │  │   (real user sends)
                       │  │ Crypto Manager │  │
                       │  └────────────────┘  │
                       └──────────────────────┘
```

## Features

- **Platform-agnostic HTTP API** — bridge any service without plugins
- **Bidirectional** — inbound via REST, outbound via webhooks
- **Puppet users** — external users appear as `@{prefix}_{platform}_{id}:domain`
- **Multi-platform rooms** — one Matrix room bridges to Telegram + Slack + ...
- **Cross-platform forwarding** — Telegram message auto-forwards to Slack (with original sender info)
- **Access control** — invite whitelist for bot/puppet users + message forwarding restrictions
- **E2BE encryption** — optional end-to-bridge encryption (mautrix approach)
- **Rich content** — text, images, files, video, audio, reactions, edits, redactions

## Quick Start

```bash
# 1. Configure
cp config.toml /data/config.toml   # edit tokens and homeserver URL

# 2. Generate appservice registration
BRIDGE_CONFIG=/data/config.toml matrix-bridge --generate-registration

# 3. Add registration to Synapse homeserver.yaml
#    app_service_config_files:
#      - /data/appservices/registration.yaml

# 4. Run
docker compose up -d
```

## Usage Example

```bash
# If api_key is configured, add: -H "Authorization: Bearer <api_key>"

# Link an external room to a Matrix room
curl -X POST http://bridge:29320/api/v1/rooms \
  -H "Content-Type: application/json" \
  -d '{"platform":"myapp","external_room_id":"general","matrix_room_id":"!abc:example.com"}'

# Register a webhook to receive Matrix messages
curl -X POST http://bridge:29320/api/v1/webhooks \
  -H "Content-Type: application/json" \
  -d '{"platform":"myapp","url":"http://myapp:8080/from-matrix"}'

# Send a message from external platform to Matrix
curl -X POST http://bridge:29320/api/v1/message \
  -H "Content-Type: application/json" \
  -d '{
    "platform": "myapp",
    "room_id": "general",
    "sender": {"id": "alice", "display_name": "Alice"},
    "content": {"type": "text", "body": "Hello from external!"}
  }'
```

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/getting-started.md) | Configuration, deployment, Docker setup |
| [API Reference](docs/api-reference.md) | Complete HTTP API specification |
| [Architecture](docs/architecture.md) | System design, cross-platform forwarding, encryption |
| [Examples](examples/) | Python and Node.js webhook integration demos |

### Chinese / 中文文档

| 文档 | 说明 |
|------|------|
| [快速入门](docs/getting-started.zh.md) | 配置、部署、Docker 设置 |
| [API 参考](docs/api-reference.zh.md) | 完整 HTTP API 规范 |
| [架构说明](docs/architecture.zh.md) | 系统设计、跨平台转发、加密 |
| [示例代码](examples/) | Python 和 Node.js webhook 集成 demo |

## Project Structure

```
crates/
├── core/          # Shared types: BridgeMessage, ExternalUser, config
├── store/         # SQLite: room mappings, message mappings, puppets, webhooks
└── appservice/    # HTTP server, dispatcher, puppet manager, crypto, auth
src/main.rs        # Entry point
docs/              # Documentation (EN + ZH)
examples/          # Integration demo code
```

## License

MIT
