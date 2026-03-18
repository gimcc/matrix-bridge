# Matrix Bridge API Reference

Base URL: `http://<bridge_host>:<port>` (default port: **29320**)

All request/response bodies are JSON unless otherwise noted.

---

## Authentication

The Bridge API (`/api/v1/admin/*`) supports optional API key authentication, configured separately from the Matrix `hs_token`.

| Config Field | Default | Description |
|---|---|---|
| `appservice.api_key` | _(empty)_ | When set, all `/api/v1/admin/*` requests require this key |

When `api_key` is configured, include it in every request via one of:

```
Authorization: Bearer <api_key>
```

or as a query parameter:

```
GET /api/v1/admin/rooms?platform=myapp&access_token=<api_key>
```

When `api_key` is not set (default), the Bridge API requires no authentication. This is suitable for internal/trusted-network deployments where access control is handled at the network level (firewall, reverse proxy, etc.).

> **Note:** `api_key` is independent from `hs_token`. The `hs_token` is a Matrix protocol secret used exclusively between Synapse and the bridge on `/_matrix/app/v1/*` routes. External services should never use or know the `hs_token`.

---

## Table of Contents

- [Authentication](#authentication)
- [Health Check](#health-check)
- [Server Info](#server-info)
- [Encryption Status](#encryption-status)
- [Send Inbound Message](#send-inbound-message)
- [Upload Media](#upload-media)
- [Room Mappings](#room-mappings)
- [Webhooks](#webhooks)
- [Puppet Users](#puppet-users)
- [Message Mappings](#message-mappings)
- [Webhook Callback Format](#webhook-callback-format-outbound)
- [SSRF Protection](#ssrf-protection)
- [Content Types](#content-types)
- [Puppet User Naming](#puppet-user-naming)

---

## Health Check

```
GET /health
```

**Response** `200`

```json
{
  "status": "ok"
}
```

---

## Server Info

```
GET /api/v1/admin/info
```

Returns server configuration, feature flags, and runtime statistics.

**Response `200`**

```json
{
  "version": "0.1.0",
  "homeserver": {
    "url": "https://matrix.example.com",
    "domain": "example.com"
  },
  "bot": {
    "user_id": "@bridge_bot:example.com",
    "puppet_prefix": "bot"
  },
  "features": {
    "encryption_enabled": true,
    "encryption_default": true,
    "webhook_ssrf_protection": false,
    "api_key_required": true
  },
  "permissions": {
    "invite_whitelist": ["@admin:example.com"]
  },
  "platforms": {
    "configured": ["telegram", "slack"],
    "active": ["telegram"]
  },
  "stats": {
    "room_mappings": 5,
    "webhooks": 3,
    "message_mappings": 1024,
    "puppets": 42
  }
}
```

| Field | Description |
|-------|-------------|
| `platforms.configured` | Platforms defined in config |
| `platforms.active` | Platforms with at least one room mapping |
| `stats.*` | Row counts from the database |

---

## Encryption Status

```
GET /api/v1/admin/crypto
```

Returns encryption key status for the bridge bot and all initialized puppet crypto devices. Queries the homeserver for actual device key state.

**Response `200` (encryption enabled)**

```json
{
  "enabled": true,
  "per_user_crypto": true,
  "bot": {
    "user_id": "@bridge_bot:example.com",
    "device_id": "BRIDGE_DEV",
    "has_master_key": true,
    "has_self_signing_key": true,
    "has_user_signing_key": true,
    "device_keys_uploaded": true,
    "device_keys": {
      "algorithms": ["m.olm.v1.curve25519-aes-sha2", "m.megolm.v1.aes-sha2"],
      "keys": {
        "curve25519:BRIDGE_DEV": "...",
        "ed25519:BRIDGE_DEV": "..."
      },
      "signatures": { "..." : { "..." : "..." } }
    }
  },
  "puppets": [
    {
      "user_id": "@telegram_user123:example.com",
      "device_id": "PUP_abc123",
      "has_master_key": true,
      "has_self_signing_key": true,
      "has_user_signing_key": true,
      "device_keys_uploaded": true,
      "device_keys": { "..." }
    }
  ]
}
```

**Response `200` (encryption disabled)**

```json
{
  "enabled": false,
  "per_user_crypto": false,
  "bot": null,
  "puppets": []
}
```

| Field | Description |
|-------|-------------|
| `enabled` | Whether E2EE is enabled in config |
| `per_user_crypto` | Whether per-user crypto mode is active (each puppet gets its own OlmMachine) |
| `bot` | Bridge bot's crypto status |
| `puppets` | Array of initialized puppet crypto statuses |
| `has_master_key` | Cross-signing master key exists in local store |
| `has_self_signing_key` | Cross-signing self-signing key exists |
| `has_user_signing_key` | Cross-signing user-signing key exists |
| `device_keys_uploaded` | Whether device keys are present on the homeserver |
| `device_keys` | Raw device key object from the homeserver (algorithms, identity keys, signatures) |

---

## Send Inbound Message

```
POST /api/v1/admin/message
```

Sends a message from an external platform into Matrix. The bridge creates a puppet user for the sender and delivers the message to the mapped Matrix room.

### Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `platform` | string | Yes | Platform identifier (`[a-z]+`), e.g. `telegram`, `slack` |
| `room_id` | string | Yes | External room ID (must have an existing room mapping) |
| `sender` | object | Yes | Sender information (see below) |
| `content` | object | Yes | Message content (see [Content Types](#content-types)) |
| `external_message_id` | string | No | Deduplication key; duplicate IDs are silently dropped |
| `reply_to` | string | No | `external_message_id` of the message being replied to |

**Sender Object**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | User ID on the external platform |
| `display_name` | string | No | Display name for the puppet user |
| `avatar_url` | string | No | Avatar URL for the puppet user |

### Example

```json
{
  "platform": "telegram",
  "room_id": "chat_12345",
  "sender": {
    "id": "user789",
    "display_name": "Alice",
    "avatar_url": "https://cdn.example.com/avatars/alice.jpg"
  },
  "content": {
    "type": "text",
    "body": "Hello!"
  },
  "external_message_id": "msg_001",
  "reply_to": "msg_000"
}
```

The puppet user created from this request would be `@telegram_user789:<homeserver_domain>`.

### Response `200`

```json
{
  "event_id": "$abc123...",
  "message_id": "01J..."
}
```

| Field | Description |
|-------|-------------|
| `event_id` | Matrix event ID |
| `message_id` | Internal bridge message ID |

---

## Upload Media

```
POST /api/v1/admin/upload
```

Uploads a file to the Matrix content repository. Use the returned `content_uri` in message content fields (e.g. `url` for image/file/video/audio types).

**Maximum file size: 200 MB.** Requests exceeding this limit receive a `413 Payload Too Large` response.

### Request

Multipart form-data with a single `file` field.

```bash
curl -X POST http://localhost:29320/api/v1/admin/upload \
  -F "file=@photo.jpg"
```

### Response `200`

```json
{
  "content_uri": "mxc://example.com/abc123",
  "filename": "photo.jpg",
  "size": 12345
}
```

---

## Room Mappings

Room mappings link an external platform room to a Matrix room. Messages are only bridged for rooms that have a mapping.

### Create Room Mapping

```
POST /api/v1/admin/rooms
```

Idempotent: if a mapping for `(platform, external_room_id)` already exists, returns the existing mapping (`200`). Otherwise creates a new one (`201`). When `matrix_room_id` is omitted, the bridge auto-creates a new Matrix room.

**Request Body**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `platform` | string | Yes | Platform identifier |
| `external_room_id` | string | Yes | Room ID on the external platform |
| `matrix_room_id` | string | No | Specific Matrix room ID; omit to auto-create |
| `room_name` | string | No | Room name for auto-creation (max 255 chars; ignored if `matrix_room_id` is provided) |
| `invite` | array | No | Extra Matrix user IDs to invite on auto-creation (max 50; requires `allow_api_invite = true` in config) |

**Example (explicit room)**

```json
{
  "platform": "telegram",
  "external_room_id": "chat_123",
  "matrix_room_id": "!abc:example.com"
}
```

**Example (auto-create)**

```json
{
  "platform": "telegram",
  "external_room_id": "chat_123",
  "room_name": "Telegram Chat",
  "invite": ["@admin:example.com"]
}
```

**Response `201`** (created)

```json
{
  "id": 1,
  "matrix_room_id": "!abc:example.com"
}
```

**Response `200`** (existing mapping returned)

```json
{
  "id": 1,
  "matrix_room_id": "!abc:example.com"
}
```

### List Room Mappings

```
GET /api/v1/admin/rooms?platform=telegram
```

| Parameter | Required | Description |
|-----------|----------|-------------|
| `platform` | No | Filter by platform; omit to list all mappings |

**Response `200`**

```json
{
  "rooms": [
    {
      "id": 1,
      "platform_id": "telegram",
      "external_room_id": "chat_123",
      "matrix_room_id": "!abc:example.com"
    }
  ]
}
```

### Delete Room Mapping

```
DELETE /api/v1/admin/rooms/{id}
```

**Response `200`**

```json
{
  "deleted": true
}
```

**Response `404`** (if mapping does not exist)

```json
{
  "error": "not found"
}
```

---

## Webhooks

Webhooks allow external platforms to receive messages that originate from Matrix (outbound direction). When a message is sent in a mapped Matrix room, the bridge dispatches it to all matching registered webhooks.

### Register Webhook

```
POST /api/v1/admin/webhooks
```

**Request Body**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `platform` | string | Yes | Platform identifier for this webhook |
| `url` | string | Yes | Callback URL that will receive POST requests (must use `http` or `https` scheme; see [SSRF Protection](#ssrf-protection) below) |
| `events` | string | No | Event types to subscribe to (default: `"message"`) |
| `forward_sources` | array or string | No | Allowlist of source platforms to forward; empty (default) = deny all, `["*"]` = forward all, `["telegram", "discord"]` = only those. Accepts a JSON array or comma-separated string |

**Example**

```json
{
  "platform": "myapp",
  "url": "http://myapp:8080/hook",
  "events": "message",
  "forward_sources": ["*"]
}
```

**Response `201`**

```json
{
  "id": 1
}
```

### List Webhooks

```
GET /api/v1/admin/webhooks?platform=myapp
```

| Parameter | Required | Description |
|-----------|----------|-------------|
| `platform` | No | Filter by platform; omit to list all webhooks |

**Response `200`**

```json
{
  "webhooks": [
    {
      "id": 1,
      "platform": "myapp",
      "url": "http://myapp:8080/hook",
      "events": "message",
      "forward_sources": ["*"]
    }
  ]
}
```

### Delete Webhook

```
DELETE /api/v1/admin/webhooks/{id}
```

**Response `200`**

```json
{
  "deleted": true
}
```

**Response `404`** (if webhook does not exist)

```json
{
  "error": "not found"
}
```

---

## Puppet Users

Puppet users are Matrix accounts created by the bridge to represent external platform users.

### List Puppets

```
GET /api/v1/admin/puppets?platform=telegram
```

| Parameter | Required | Description |
|-----------|----------|-------------|
| `platform` | No | Filter by platform; omit to list all puppets |

**Response `200`**

```json
{
  "puppets": [
    {
      "id": 1,
      "matrix_user_id": "@telegram_user123:example.com",
      "platform_id": "telegram",
      "external_user_id": "user123",
      "display_name": "Alice",
      "avatar_mxc": "mxc://example.com/abc123"
    }
  ]
}
```

---

## Message Mappings

Message mappings track the relationship between Matrix events and external platform messages. Supports cursor-based pagination for large datasets.

### List Message Mappings

```
GET /api/v1/admin/messages?platform=telegram&room_mapping_id=1&after=0&limit=100
```

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `platform` | No | — | Filter by platform |
| `room_mapping_id` | No | — | Filter by room mapping ID |
| `after` | No | `0` | Cursor: return messages with `id > after` |
| `limit` | No | `100` | Max results per page (max: 1000) |

**Response `200`**

```json
{
  "messages": [
    {
      "id": 1,
      "matrix_event_id": "$event123",
      "platform_id": "telegram",
      "external_message_id": "msg_456",
      "room_mapping_id": 1
    }
  ],
  "next_cursor": 1
}
```

| Field | Description |
|-------|-------------|
| `messages` | Array of message mapping objects |
| `next_cursor` | ID of the last result; pass as `after` for the next page. `null` when the result set is empty |

**Pagination example:**

```
GET /api/v1/admin/messages?limit=100           → next_cursor: 100
GET /api/v1/admin/messages?after=100&limit=100 → next_cursor: 200
GET /api/v1/admin/messages?after=200&limit=100 → next_cursor: null (no more)
```

---

## Webhook Callback Format (Outbound)

When a message is sent in a mapped Matrix room, the bridge POSTs a JSON payload to each matching webhook. The payload format differs depending on whether the sender is a real Matrix user or a cross-platform puppet.

### Real Matrix User

A native Matrix user sends a message in a room mapped to `myapp`:

```json
{
  "event": "message",
  "platform": "myapp",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "matrix",
      "external_id": "@alice:example.com",
      "display_name": null,
      "avatar_url": null
    },
    "room": {
      "platform": "myapp",
      "external_id": "general"
    },
    "content": {
      "type": "text",
      "body": "Hello!"
    },
    "timestamp": 1710000000000,
    "reply_to": null
  }
}
```

### Cross-Platform Forwarded Message

A Telegram puppet user sends a message in a room that is also mapped to Slack. The Slack webhook receives:

```json
{
  "event": "message",
  "platform": "slack",
  "source_platform": "telegram",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "telegram",
      "external_id": "user123",
      "display_name": "Alice",
      "avatar_url": "mxc://example.com/abc123"
    },
    "room": {
      "platform": "slack",
      "external_id": "C123"
    },
    "content": {
      "type": "text",
      "body": "Hello!"
    },
    "timestamp": 1710000000000,
    "reply_to": null
  }
}
```

**Key difference:** Cross-platform payloads include `source_platform` to indicate where the message originally came from. The `sender` object reflects the original external user, not the Matrix puppet.

### Callback Fields

| Field | Type | Description |
|-------|------|-------------|
| `event` | string | Event type (e.g. `"message"`) |
| `platform` | string | Target platform (matches the webhook's platform) |
| `source_platform` | string | Present only for cross-platform messages; the originating platform |
| `message.id` | string | Matrix event ID |
| `message.sender.platform` | string | `"matrix"` for real users, or the originating platform for puppets |
| `message.sender.external_id` | string | Matrix user ID or external platform user ID |
| `message.sender.display_name` | string or null | Display name if available |
| `message.sender.avatar_url` | string or null | Avatar URL if available |
| `message.room.platform` | string | Target platform |
| `message.room.external_id` | string | External room ID from the room mapping |
| `message.content` | object | Message content (see [Content Types](#content-types)) |
| `message.timestamp` | number | Unix timestamp in milliseconds |
| `message.reply_to` | string or null | Event ID of the message being replied to |

---

## SSRF Protection

Webhook URLs always require `http` or `https` scheme. When `appservice.webhook_ssrf_protection = true` is set in the config, additional checks block URLs targeting private/reserved networks:

- **Blocked IPs:** loopback (127.0.0.0/8, ::1), RFC1918 (10/8, 172.16/12, 192.168/16), link-local (169.254/16, fe80::/10), CGNAT (100.64/10), IPv6 ULA (fc00::/7), unspecified (0.0.0.0, ::), broadcast, documentation ranges, cloud metadata (169.254.169.254)
- **DNS resolution:** hostnames are resolved and all resulting IPs are checked, preventing rebinding attacks (e.g. `127.0.0.1.nip.io`)
- **IPv4-mapped IPv6:** addresses like `::ffff:10.0.0.1` are unwrapped and checked against IPv4 rules

Default is `false` (allow all targets), suitable for internal deployments.

---

## Content Types

| Type | Required Fields | Optional Fields |
|------|----------------|-----------------|
| `text` | `body` | `html` |
| `image` | `url` | `caption`, `mimetype` (default: `image/png`) |
| `file` | `url`, `filename` | `mimetype` (default: `application/octet-stream`) |
| `video` | `url` | `caption`, `mimetype` (default: `video/mp4`) |
| `audio` | `url` | `mimetype` (default: `audio/ogg`) |
| `location` | `latitude`, `longitude` | -- |
| `notice` | `body` | -- |
| `emote` | `body` | -- |
| `reaction` | `target_id`, `emoji` | -- |
| `redaction` | `target_id` | -- |
| `edit` | `target_id`, `new_content` | -- |

### Examples

**Text message:**
```json
{ "type": "text", "body": "Hello!" }
```

**Text with HTML:**
```json
{ "type": "text", "body": "Hello!", "html": "<b>Hello!</b>" }
```

**Image (via uploaded mxc URI):**
```json
{ "type": "image", "url": "mxc://example.com/abc123", "caption": "A sunset" }
```

**File:**
```json
{ "type": "file", "url": "mxc://example.com/def456", "filename": "report.pdf", "mimetype": "application/pdf" }
```

**Location:**
```json
{ "type": "location", "latitude": 37.7749, "longitude": -122.4194 }
```

**Reaction to a message:**
```json
{ "type": "reaction", "target_id": "msg_001", "emoji": "👍" }
```

**Edit a previous message:**
```json
{ "type": "edit", "target_id": "msg_001", "new_content": { "type": "text", "body": "Updated text" } }
```

**Delete a message:**
```json
{ "type": "redaction", "target_id": "msg_001" }
```

---

## Puppet User Naming

The bridge creates Matrix puppet users for external platform senders. The localpart follows this format:

```
@{puppet_prefix}_{platform}_{sender.id}:{homeserver_domain}
```

**Constraints:**

- `puppet_prefix`: configurable (default: `bot`)
- `platform`: lowercase letters only (`[a-z]+`)
- `sender.id`: lowercase alphanumeric plus `.` `_` `-` `=` `/` (`[a-z0-9._\-=/]+`)

**Examples:**

| Platform | Sender ID | Matrix User ID |
|----------|-----------|----------------|
| telegram | `12345` | `@bot_telegram_12345:example.com` |
| slack | `u.bob` | `@bot_slack_u.bob:example.com` |
| discord | `98765` | `@bot_discord_98765:example.com` |
