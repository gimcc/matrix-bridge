# Matrix Bridge Architecture

This document describes the architecture that the current code actually implements.

## High-Level Layout

```text
External Services
   |  REST / webhook / WebSocket
   v
matrix-bridge-appservice
   |  Matrix client-server + appservice APIs
   v
Synapse / Matrix Homeserver
```

The repository is split into three workspace crates plus the binary entrypoint.

| Component | Path | Responsibility |
|-----------|------|----------------|
| `matrix-bridge-core` | `crates/core` | Config model, shared message types, registration generation, ID sanitization helpers |
| `matrix-bridge-store` | `crates/store` | SQLite schema and persistence for mappings, puppets, webhooks, spaces |
| `matrix-bridge-appservice` | `crates/appservice` | HTTP server, dispatcher, Matrix client, webhook / WS delivery, crypto runtime |
| `matrix-bridge` | `src/main.rs` | Startup orchestration, config loading, registration management, server boot |

## Runtime Components

### `MatrixClient`

Wrapper over Matrix client-server and appservice HTTP calls. It handles:

- user and device registration
- room creation, join, leave, invite, and space operations
- message sending
- media upload and download
- encryption-related endpoints and MSC query parameters

### `PuppetManager`

Creates and updates puppet users on demand. A puppet represents an external user as:

```text
@{puppet_prefix}_{platform}_{sanitized_external_id}:{domain}
```

The manager caches known puppets and persists profile data in SQLite.

### `Dispatcher`

Central router for:

- external -> Matrix delivery
- Matrix -> webhook / WS delivery
- bot command handling
- cross-platform relay decisions
- permission checks
- platform space maintenance

### `WsRegistry`

Tracks active WebSocket subscribers per platform, including:

- forward source filters
- capability declarations
- connection counting and limits

### `CryptoManager` and `CryptoManagerPool`

Provide end-to-bridge encryption support:

- single-device mode: all encrypted sends use the bridge bot device
- per-user mode: each puppet may get its own device and crypto store

## Storage Model

SQLite stores the bridge state in five main tables.

| Table | Purpose |
|-------|---------|
| `room_mappings` | `(platform_id, external_room_id)` <-> `matrix_room_id` |
| `message_mappings` | External message IDs and Matrix event IDs per platform |
| `puppets` | Puppet MXIDs and cached profile data |
| `webhooks` | Registered outbound HTTP integrations, filters, capabilities, owner |
| `platform_spaces` | One Matrix Space per platform |

Notable schema properties:

- `room_mappings` is unique both by Matrix room + platform and by platform + external room.
- `message_mappings` is unique by `(platform_id, external_message_id)` and `(matrix_event_id, platform_id)`.
- `webhooks` upserts on `(platform_id, webhook_url)`.

## Startup Sequence

`src/main.rs` performs the following steps:

1. Load `BRIDGE_CONFIG` or generate a default config if missing.
2. Parse and validate the config.
3. Open and migrate SQLite.
4. Build the Matrix client.
5. Initialize the puppet manager.
6. Generate or verify `registration.yaml`.
7. Register the bridge bot user, with device information when encryption is enabled.
8. Initialize the crypto pool when encryption is enabled.
9. Build the dispatcher and shared app state.
10. Start the Axum HTTP server.

## HTTP Surface

The server exposes three groups of routes.

### Matrix appservice routes

Protected by `hs_token`:

- `PUT /_matrix/app/v1/transactions/{txnId}`
- `GET /_matrix/app/v1/users/{userId}`
- `GET /_matrix/app/v1/rooms/{roomAlias}`

### Bridge HTTP API routes

Optionally protected by `appservice.api_key`, plus IP-based rate limiting:

- operational routes under `/api/v1/*`
- admin routes under `/api/v1/admin/*`

### WebSocket route

- `GET /api/v1/ws`
- optional auth through the first WS frame, not through the query string

## Message Flow

### External -> Matrix

1. An external service sends `POST /api/v1/message`.
2. The dispatcher sanitizes the external sender and room identifiers.
3. The room mapping is looked up in SQLite.
4. The puppet is created or refreshed if needed.
5. The puppet joins the Matrix room if necessary.
6. The content is delivered to Matrix, encrypted if the room requires it.
7. A `message_mappings` row is written for deduplication and later reverse lookups.

### Matrix -> External

1. Synapse pushes events through the appservice transaction endpoint.
2. The server deduplicates transaction IDs.
3. The dispatcher skips bridge-originated loops and enforces permissions.
4. Room mappings are looked up for the Matrix room.
5. Matching webhooks receive HTTP callbacks.
6. Matching WebSocket subscribers receive the same payload.
7. A `message_mappings` row is written per platform target.

## Cross-Platform Relay

Cross-platform relay means:

- a Telegram-originated message can land in Matrix
- the same Matrix event can then be forwarded to Slack or another platform

This is controlled by two layers:

1. Global config: `appservice.allow_relay`
2. Per integration allowlist: webhook / WS `forward_sources`

Rules:

- Matrix-user messages are always eligible for delivery.
- Non-Matrix source delivery only happens when `allow_relay = true`.
- `forward_sources = []` means "Matrix only".
- `forward_sources = ["*"]` means "allow any source platform".

## Permissions

The bridge does not use the historical `invite_whitelist` model anymore. It uses:

```toml
[permissions]
admin = ["@admin:example.com"]
relay = ["@*:trusted.example"]
relay_min_power_level = 0
```

Semantics:

- `admin`: full access, including bot DM commands and inviting the bridge into rooms
- `relay`: lower-privilege permission tier; DM commands and invites are still denied
- `relay_min_power_level`: per-room floor for normal Matrix-user message forwarding
- both lists empty: open mode, everyone is treated as admin

## Platform Spaces

The bridge can keep one Matrix Space per platform. When a new mapping is created:

1. the platform space is created or reused
2. the mapped room is attached as a child

This keeps multi-room integrations organized without requiring clients to manage spaces manually.

## Security Model

### Auth separation

- `hs_token` is strictly for Matrix appservice traffic
- `api_key` is strictly for external Bridge API traffic

### Webhook validation

When `appservice.webhook_ssrf_protection = true`, webhook registration blocks:

- localhost
- `metadata.google.internal`
- private IPv4 ranges
- loopback and link-local addresses
- CGNAT ranges
- reserved / documentation ranges
- IPv6 loopback and unique-local ranges
- hostnames that resolve into blocked IPs

### Input hardening

The bridge:

- sanitizes external IDs before using them in storage or puppet MXIDs
- bounds request field lengths
- caps reaction emoji length
- caps upload body size at `200 MiB`

### Rate limiting

Bridge HTTP API routes use a governor layer configured for:

- `120` requests per second
- `300` burst

## Encryption Modes

When encryption is disabled, all messages are sent in plain form through the normal Matrix APIs.

When encryption is enabled:

- the bridge bot gets a persistent crypto store
- appservice MSC fields are added to the registration
- encrypted rooms are detected dynamically
- outbound encrypted sends go through the crypto manager

Two runtime modes exist:

| Mode | Config | Behavior |
|------|--------|----------|
| Single-device | `per_user_crypto = false` | Puppets send using bridge-bot device masquerading |
| Per-user | `per_user_crypto = true` | Each active puppet gets its own derived device ID and crypto store |

For implementation details, see [encryption.md](encryption.md).

## Why the Architecture Looks This Way

The design is intentionally narrow:

- external integrations only need HTTP or WebSocket support
- Matrix-specific complexity stays inside the bridge
- persistence stays in SQLite to simplify deployment
- crypto support remains optional and isolated from the basic message path

That keeps the common path small while still allowing advanced deployments with relay, spaces, and encrypted rooms.
