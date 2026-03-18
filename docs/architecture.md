# Matrix Bridge Architecture

## System Overview

Matrix Bridge is a Rust application service that bridges Matrix rooms with external messaging platforms (Telegram, Slack, Discord, etc.) using puppet users, webhook-based message delivery, and optional end-to-bridge encryption.

```
                          +-------------------------------------------+
                          |              matrix-bridge (bin)           |
                          |                  src/main.rs               |
                          +--------+----------+-----------+-----------+
                                   |          |           |
            +----------------------+    +-----+-----+    +------------------+
            |   matrix-bridge-core |    |  appservice |    | matrix-bridge-   |
            |     crates/core      |    | crates/     |    |   store          |
            |                      |    | appservice  |    | crates/store     |
            +----------------------+    +-------------+    +------------------+
```

### Crate Structure

| Crate | Path | Responsibility |
|-------|------|----------------|
| `matrix-bridge-core` | `crates/core` | Shared types: `BridgeMessage`, `MessageContent`, `ExternalUser`, `AppConfig`, error types, registration YAML generation |
| `matrix-bridge-store` | `crates/store` | SQLite database layer: `Database`, migrations, CRUD for room_mappings, message_mappings, puppets, webhooks |
| `matrix-bridge-appservice` | `crates/appservice` | Application service runtime: HTTP server (axum), Dispatcher, PuppetManager, MatrixClient, CryptoManager, Bridge HTTP API, auth middleware |
| `matrix-bridge` (bin) | `src/main.rs` | Entry point: loads config, opens database, initializes all components, starts HTTP server |


### Main Components

```
+------------------+     +------------------+     +------------------+
|   MatrixClient   |     |  PuppetManager   |     |   CryptoManager  |
|                  |     |                  |     |                  |
| HTTP client for  |     | Creates/updates  |     | OlmMachine for   |
| Synapse CS API.  |     | puppet users on  |     | E2BE encryption  |
| as_token auth,   |     | first use. Caches|     | and decryption.  |
| MSC3202 device   |     | in DashMap +     |     | Megolm sessions, |
| masquerading.    |     | persists in DB.  |     | key exchange.    |
+------------------+     +------------------+     +------------------+
         |                        |                        |
         +------------+-----------+------------------------+
                      |
              +-------v--------+
              |   Dispatcher   |
              |                |
              | Routes events  |
              | between Matrix |
              | and external   |
              | platforms.     |
              | Access control,|
              | cross-platform |
              | forwarding +   |
              | loop prevention|
              +-------+--------+
                      |
              +-------v--------+
              |    Database    |
              |                |
              | SQLite (WAL).  |
              | room_mappings, |
              | message_map,   |
              | puppets,       |
              | webhooks.      |
              +----------------+
```

---

## Message Flow

### Inbound: External Platform -> Matrix

An external service sends a message via the Bridge HTTP API.

```
External Service                    Bridge                              Synapse
     |                               |                                    |
     |  POST /api/v1/message         |                                    |
     |  {platform, room_id,          |                                    |
     |   sender, content}            |                                    |
     |------------------------------>|                                    |
     |                               |                                    |
     |                   bridge_api::handle_send_message                  |
     |                               |                                    |
     |                   Dispatcher::handle_incoming_http                 |
     |                               |                                    |
     |                     1. DB: find_room_by_external_id                |
     |                     2. PuppetManager::ensure_puppet_direct         |
     |                               |                                    |
     |                               |  POST /register (if new puppet)   |
     |                               |----------------------------------->|
     |                               |                                    |
     |                               |  PUT /profile/.../displayname     |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     3. MatrixClient::join_room                     |
     |                               |  POST /join/{room_id}             |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     4. Dispatcher::send_to_matrix                  |
     |                               |  PUT /rooms/{room}/send/...       |
     |                               |----------------------------------->|
     |                               |                                    |
     |                     5. DB: create_message_mapping                  |
     |                               |                                    |
     |  {event_id, message_id}       |                                    |
     |<------------------------------|                                    |
```

The puppet user (e.g., `@telegram_user123:domain`) appears in the Matrix room as if the external user sent the message directly.

### Outbound: Matrix -> External Platform

When a Matrix user sends a message in a bridged room, Synapse delivers it to the appservice via the transaction endpoint.

```
Matrix Client          Synapse                  Bridge                  External Service
     |                    |                       |                          |
     |  send message      |                       |                          |
     |  in !room:domain   |                       |                          |
     |------------------->|                       |                          |
     |                    |                       |                          |
     |                    |  PUT /transactions/N  |                          |
     |                    |  {events: [...]}      |                          |
     |                    |---------------------->|                          |
     |                    |                       |                          |
     |                    |         verify_hs_token (Bearer or query param)  |
     |                    |                       |                          |
     |                    |         Dispatcher::handle_transaction           |
     |                    |         -> handle_event -> handle_room_message   |
     |                    |                       |                          |
     |                    |         1. Check: is sender bridge_bot? skip     |
     |                    |         2. Check: invite_whitelist (Layer 0)     |
     |                    |            - puppets bypass, others must match   |
     |                    |         3. Check: is sender a puppet? extract    |
     |                    |            source_platform for loop prevention   |
     |                    |         4. DB: find_all_mappings_by_matrix_id    |
     |                    |         5. For each mapping:                     |
     |                    |            - skip if mapping.platform == source  |
     |                    |            - deliver_to_webhooks                 |
     |                    |                       |                          |
     |                    |                       |  POST webhook_url       |
     |                    |                       |  {event, platform,      |
     |                    |                       |   source_platform,      |
     |                    |                       |   message: {...}}       |
     |                    |                       |------------------------->|
     |                    |                       |                          |
     |                    |         6. DB: create_message_mapping            |
     |                    |                       |                          |
     |                    |  200 OK {}            |                          |
     |                    |<----------------------|                          |
```

---

## Cross-Platform Forwarding

This is a core feature of the bridge. A single Matrix room can be bridged to multiple external platforms simultaneously, and messages flow between all of them through Matrix as the hub.

### Scenario

A room `!room:domain` is linked to both Telegram (`chat_123`) and Slack (`C456`).

A Telegram user "Alice" (ID `user123`) sends a message:

```
1. Telegram bot receives message
2. POST /api/v1/message to bridge
   {platform: "telegram", sender: {id: "user123", display_name: "Alice"}, ...}

3. Bridge creates/reuses puppet @telegram_user123:domain
4. Puppet sends message in !room:domain
5. Synapse delivers the event back to the bridge via /transactions

6. Dispatcher receives event from sender @telegram_user123:domain
7. puppet_source_platform("@telegram_user123:domain") => Some("telegram")
8. DB returns mappings: [{platform: "telegram", ...}, {platform: "slack", ...}]

9. SKIP: platform "telegram" == source "telegram"  (loop prevention)
10. FORWARD: platform "slack" != source "telegram"  (cross-platform delivery)

11. Webhook payload to Slack service:
    {
      "event": "message",
      "platform": "slack",
      "source_platform": "telegram",
      "message": {
        "sender": {
          "platform": "telegram",
          "external_id": "user123",
          "display_name": "Alice"
        },
        "content": { "type": "text", "body": "Hello from Telegram!" },
        ...
      }
    }
```

Key implementation details:

- **Source platform detection**: `Dispatcher::puppet_source_platform()` parses the puppet's Matrix user ID (`@{platform}_{userid}:domain`) to extract the originating platform.
- **Original sender preservation**: When cross-forwarding, the bridge looks up the puppet in the database to retrieve the original `platform`, `external_id`, and `display_name`; the webhook payload carries this real identity, not the Matrix puppet ID.
- **Message mapping**: `UNIQUE(matrix_event_id, platform_id)` allows one Matrix event to be mapped to multiple platforms simultaneously.

### Visual Summary

```
    Telegram                    Matrix Room                      Slack
       |                       !room:domain                        |
       |                            |                              |
  Alice sends    puppet             |                              |
  "Hello"  ----> @telegram_user123  |                              |
       |         sends in room ---->|                              |
       |                            |----> Dispatcher              |
       |                            |      source = "telegram"     |
       |      SKIP (loop) <--------|                              |
       |                            |-----> webhook to Slack ----->|
       |                            |       (original sender info) |
       |                            |                              |
       |                            |<---- @slack_bob456 sends <---|
       |                            |      (Bob from Slack)        |
       |  webhook to Telegram <-----|                              |
       |  (original sender: Bob)    |-----> SKIP (loop)           |
```

---

## Access Control (Invite Whitelist)

The bridge enforces a configurable whitelist that controls who can interact with the bridge bot and puppet users. This is implemented in `PermissionsConfig` (`crates/core/src/config.rs`) and enforced by the `Dispatcher`.

### Configuration

```toml
[permissions]
invite_whitelist = ["@*:example.com"]
```

### Pattern Syntax

| Pattern | Matches |
|---------|---------|
| _(empty list)_ | Everyone (open mode, default) |
| `"*"` | Everyone (explicit wildcard) |
| `"@admin:example.com"` | Exact user only |
| `"@*:example.com"` | Any user on that domain |

Multiple patterns can be combined:

```toml
invite_whitelist = ["@admin:a.com", "@*:b.com"]
# @admin:a.com  → allowed
# @other:a.com  → blocked
# @anyone:b.com → allowed
```

### Three Enforcement Points

The whitelist is checked at three distinct points in the Dispatcher:

```
                        Invite Event                    Message Event
                             |                               |
                    ┌────────▼────────┐             ┌────────▼────────┐
                    │ Is target the   │             │ Is sender a     │
                    │ bot or puppet?  │             │ puppet user?    │
                    └──┬──────────┬───┘             └──┬──────────┬───┘
                    No │          │ Yes             Yes │          │ No
                       │          ▼                     │          ▼
                    Ignore   ┌────────────┐          Bypass  ┌────────────┐
                             │ Is inviter │                  │ Is sender  │
                             │ bridge_bot?│                  │ whitelisted│
                             └──┬──────┬──┘                  └──┬──────┬──┘
                             Yes│      │No                   Yes│      │No
                                ▼      ▼                       ▼      ▼
                             Accept  Check                  Forward  Block
                                     whitelist
```

**Point 1: Bot invite** — When someone invites `@bridge_bot:domain` to a room, the sender must be in the whitelist.

**Point 2: Puppet invite** — When someone invites a puppet user (e.g., `@bot_telegram_123:domain`), the sender must be in the whitelist. The bridge bot itself always bypasses this check (it invites puppets as part of normal operation).

**Point 3: Message forwarding** — When a Matrix user sends a message in a bridged room, their message is only forwarded to external platform webhooks if the sender is in the whitelist. Puppet users bypass this check since they relay messages from authorized external platforms.

### Why This Matters

Without the whitelist, any Matrix user could:
- Invite the bridge bot to arbitrary rooms and bridge them to external platforms
- Invite puppet users directly, bypassing normal bridge flows
- Send messages through bridged rooms to external platforms

The whitelist ensures only authorized users (e.g., users on your own homeserver) can use the bridge.

### Implementation

- `PermissionsConfig::is_invite_allowed()` in `crates/core/src/config.rs` — pattern matching logic
- `Dispatcher::handle_membership()` in `crates/appservice/src/dispatcher.rs` — invite enforcement
- `Dispatcher::handle_room_message()` in `crates/appservice/src/dispatcher.rs` — forwarding enforcement

---

## Three-Layer Filtering

The bridge uses three complementary mechanisms to control message flow. Layer 0 (access control) determines _who_ can use the bridge. Layers 1 and 2 determine _where_ messages are delivered.

### Layer 0: Access Control (Invite Whitelist)

See the [Access Control](#access-control-invite-whitelist) section above. This is the first check applied to both invites and message forwarding.

### Layer 1: Built-in Loop Prevention

Automatic. When the Dispatcher processes an outbound event from a puppet user, it extracts the source platform from the puppet's Matrix user ID and skips forwarding to that same platform.

```
puppet_source_platform("@telegram_user123:domain")  =>  Some("telegram")

for each mapping:
    if mapping.platform_id == source_platform:
        SKIP   // prevents Telegram -> Matrix -> Telegram loop
    else:
        FORWARD // delivers to other platforms
```

This is always active and cannot be disabled.

### Layer 2: Per-Webhook `forward_sources` Allowlist

Configurable. Each webhook specifies which source platforms it accepts:

- **Empty** (default) = deny all — nothing is forwarded.
- `"*"` = forward all sources.
- `"telegram,matrix"` = forward only those platforms.

```
POST /api/v1/webhooks
{
  "platform": "slack",
  "url": "https://slack-bot.example.com/webhook",
  "forward_sources": ["telegram", "matrix"]
}
```

In this example, the Slack webhook will receive messages originating from Telegram and native Matrix users, but NOT messages originating from Discord.

The check happens in `Dispatcher::deliver_to_webhooks()`:

```
for webhook in webhooks:
    if NOT webhook.should_forward_source(source_platform):
        SKIP this webhook
    else:
        POST to webhook.url
```

### Filtering Example

```
Message from @telegram_user123:domain in a room bridged to Slack + Discord:

Layer 0 (access control):
  - @telegram_user123 is a puppet user → BYPASS whitelist check

Layer 1 (loop prevention):
  - telegram mapping: SKIP (source == telegram)
  - slack mapping:    PASS
  - discord mapping:  PASS

Layer 2 (forward_sources on each webhook):
  - Slack webhook (forward_sources="*"): DELIVER
  - Discord webhook (forward_sources="matrix"): SKIP (telegram not in allowlist)

Result: message delivered to Slack only
```

```
Message from @alice:example.com (non-whitelisted) in a bridged room:

Layer 0 (access control):
  - @alice:example.com is not in invite_whitelist → BLOCK

Result: message not forwarded to any webhook
```

---

## End-to-Bridge Encryption (E2BE)

The bridge supports end-to-bridge encryption using the mautrix approach. Messages are encrypted between Matrix clients and the bridge, then decrypted at the bridge before being forwarded to external platforms.

### Architecture

```
Matrix Client A                Bridge Bot                    External Platform
     |                            |                               |
     |  Olm key exchange          |                               |
     |  (to-device events)        |                               |
     |<-------------------------->|                               |
     |                            |                               |
     |  Megolm-encrypted msg      |                               |
     |  m.room.encrypted          |                               |
     |--------------------------->|                               |
     |                     CryptoManager                          |
     |                     .decrypt()                             |
     |                            |                               |
     |                     Plaintext message                      |
     |                            |                               |
     |                     Forward to webhook  ------------------>|
     |                            |                               |
     |                     Incoming from platform  <--------------|
     |                            |                               |
     |                     CryptoManager                          |
     |                     .encrypt()                             |
     |                            |                               |
     |  m.room.encrypted          |                               |
     |<---------------------------|                               |
```

### Key MSCs

| MSC | Purpose | Implementation |
|-----|---------|----------------|
| MSC2409 | To-device events via appservice transactions | `de.sorunome.msc2409.to_device` field in transaction payload, processed by `CryptoManager::receive_sync_changes()` |
| MSC3202 | Device list changes and OTK counts for appservices | `de.sorunome.msc3202.device_lists`, `device_one_time_keys_count`, `device_unused_fallback_key_types` in transactions |
| MSC3202 | Device masquerading | `user_id` + `device_id` query params on E2EE API calls via `MatrixClient::e2ee_query_params()` |

### Implementation Details

- **Single crypto device**: The bridge bot operates as one `OlmMachine` (from `matrix-sdk-crypto`) with a single device ID configured in `config.toml`.
- **Persistent crypto store**: Olm/Megolm keys are stored in a SQLite crypto store (`matrix-sdk-sqlite`) at the configured `crypto_store` path, encrypted with a mandatory passphrase.
- **Key management**: On startup, device keys and one-time keys are uploaded. The `process_outgoing_requests()` method handles key uploads, queries, claims, and to-device sends.
- **Room encryption tracking**: When an `m.room.encryption` state event is seen, the room is marked as encrypted in the `OlmMachine`'s room settings.
- **Auto-enable**: When `encryption.default = true`, the bridge automatically sends the `m.room.encryption` state event when a room is linked via `!bridge link`.
- **Auth**: Requires Synapse 1.149+ for `Authorization: Bearer` header support on appservice requests (used for all key management endpoints).

---

## Puppet User Management

Puppet (ghost) users represent external platform users inside Matrix rooms.

### Naming Convention

```
@{prefix}_{platform}_{external_user_id}:{server_name}
```

The prefix is configurable via `appservice.puppet_prefix` (default: `"bot"`).

Examples:
- `@bot_telegram_user123:im.fr.ds.cc`
- `@bot_slack_U05ABC:im.fr.ds.cc`
- `@bot_discord_123456789:im.fr.ds.cc`

The localpart must match `[a-z0-9._\-=/]+` per the Matrix spec.

### Lifecycle

```
1. Inbound message arrives for user "Alice" (platform=telegram, id=user123)

2. PuppetManager::ensure_puppet_direct("telegram_user123", ...)
   a. Check DashMap in-memory cache -> miss
   b. DB: find_puppet_by_external_id("telegram", "user123") -> miss
   c. MatrixClient::register_puppet("telegram_user123")
      POST /_matrix/client/v3/register {type: "m.login.application_service", username: "telegram_user123"}
   d. MatrixClient::set_display_name("@telegram_user123:domain", "Alice")
   e. MatrixClient::set_avatar("@telegram_user123:domain", "mxc://...")
   f. DB: upsert_puppet(...)
   g. Cache insert: "telegram:user123" -> "@telegram_user123:domain"

3. Subsequent messages: cache hit, skip registration.

4. If display_name or avatar changes: update via CS API + DB.
```

### Storage

Puppets are stored in the `puppets` table with a unique constraint on `(platform_id, external_user_id)` and a separate unique constraint on `matrix_user_id`.

---

## Database Schema

SQLite with WAL mode and foreign keys enabled. Four tables across four migrations.

### Tables

#### `room_mappings`

Links a Matrix room to an external platform room. One Matrix room can be linked to multiple platforms (one per platform).

```sql
CREATE TABLE room_mappings (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_room_id    TEXT NOT NULL,
    platform_id       TEXT NOT NULL,
    external_room_id  TEXT NOT NULL,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_room_id, platform_id),
    UNIQUE(platform_id, external_room_id)
);
```

#### `message_mappings`

Tracks which Matrix events correspond to which external messages. The unique constraint is `(matrix_event_id, platform_id)`, allowing one Matrix event to map to multiple platforms (essential for cross-platform forwarding).

```sql
CREATE TABLE message_mappings (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_event_id       TEXT NOT NULL,
    platform_id           TEXT NOT NULL,
    external_message_id   TEXT NOT NULL,
    room_mapping_id       INTEGER NOT NULL REFERENCES room_mappings(id),
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(matrix_event_id, platform_id),
    UNIQUE(platform_id, external_message_id)
);
```

#### `puppets`

Stores puppet user identity mappings and profile data.

```sql
CREATE TABLE puppets (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    matrix_user_id    TEXT NOT NULL UNIQUE,
    platform_id       TEXT NOT NULL,
    external_user_id  TEXT NOT NULL,
    display_name      TEXT,
    avatar_mxc        TEXT,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, external_user_id)
);
```

#### `webhooks`

Registered webhook endpoints that receive outbound messages (Matrix -> external).

```sql
CREATE TABLE webhooks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    platform_id      TEXT NOT NULL,
    webhook_url      TEXT NOT NULL,
    secret           TEXT,
    events           TEXT NOT NULL DEFAULT 'message',
    enabled          INTEGER NOT NULL DEFAULT 1,
    forward_sources  TEXT NOT NULL DEFAULT '',  -- allowlist: empty=deny all, "*"=all, "telegram,matrix"=specific
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(platform_id, webhook_url)
);
```

### Entity Relationships

```
room_mappings 1---* message_mappings
    |                    (via room_mapping_id FK)
    |
    +-- UNIQUE(matrix_room_id, platform_id)
    +-- UNIQUE(platform_id, external_room_id)

puppets
    +-- UNIQUE(matrix_user_id)
    +-- UNIQUE(platform_id, external_user_id)

webhooks
    +-- UNIQUE(platform_id, webhook_url)

message_mappings
    +-- UNIQUE(matrix_event_id, platform_id)  -- one event per platform
    +-- UNIQUE(platform_id, external_message_id)
```

---

## HTTP Endpoints

### Appservice Endpoints (hs_token auth)

| Method | Path | Purpose |
|--------|------|---------|
| PUT | `/_matrix/app/v1/transactions/{txnId}` | Receive events from Synapse (including MSC2409/3202 E2EE data) |
| GET | `/_matrix/app/v1/users/{userId}` | User existence query |
| GET | `/_matrix/app/v1/rooms/{roomAlias}` | Room alias query |

### Bridge API Endpoints (optional `api_key` auth)

When `appservice.api_key` is set, every request to `/api/v1/*` must include the key via `Authorization: Bearer <api_key>` or `?access_token=<api_key>`. When omitted (default), no authentication is required -- suitable for internal/trusted-network deployments.

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/v1/message` | Send message from external platform to Matrix |
| POST | `/api/v1/upload` | Upload media (max 200 MB), returns `mxc://` URI |
| POST | `/api/v1/rooms` | Create room mapping |
| GET | `/api/v1/rooms?platform=X` | List room mappings |
| DELETE | `/api/v1/rooms/{id}` | Delete room mapping |
| POST | `/api/v1/webhooks` | Register webhook |
| GET | `/api/v1/webhooks[?platform=X]` | List webhooks |
| DELETE | `/api/v1/webhooks/{id}` | Delete webhook |
| GET | `/health` | Health check |

### Webhook SSRF Protection

When `appservice.webhook_ssrf_protection = true`, webhook URL registration blocks:
- RFC1918 private addresses (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
- Loopback (127.0.0.0/8, ::1), link-local (169.254.0.0/16, fe80::/10)
- CGNAT (100.64.0.0/10), IPv6 ULA (fc00::/7)
- Cloud metadata endpoints (169.254.169.254, metadata.google.internal)
- IPv4-mapped IPv6 addresses (::ffff:x.x.x.x)
- DNS names resolving to any of the above (prevents rebinding attacks)

Default is `false` (allow private IPs), suitable for internal deployments where webhook targets are on the same private network. Enable when the bridge is exposed to untrusted networks.

### In-Room Commands

| Command | Permission | Action |
|---------|-----------|--------|
| `!bridge link <platform> <external_room_id>` | Power level >= 50 | Create room mapping |
| `!bridge unlink <platform>` | Power level >= 50 | Remove room mapping |
| `!bridge status` | Any | Show registered platforms |
