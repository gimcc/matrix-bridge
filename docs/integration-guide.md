# Integration Guide

This guide is for external services that want to bridge their own platform into Matrix through the current Bridge API.

## Integration Model

```text
Your Service  <->  Matrix Bridge  <->  Synapse / Matrix
   REST / WS        appservice
```

Typical flow:

1. Create or reuse a room mapping with `POST /api/v1/rooms`
2. Register a webhook or connect a WebSocket client
3. Send external messages with `POST /api/v1/message`
4. Receive Matrix-originated messages as webhook or WebSocket payloads

## Quick Start

### 1. Register a webhook

```bash
curl -X POST http://localhost:29320/api/v1/webhooks \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "url": "https://your-service.example.com/webhook",
    "forward_sources": ["matrix"],
    "capabilities": ["message", "image", "file", "reaction", "command"],
    "owner": "@bridge-admin:example.com"
  }'
```

Field notes:

| Field | Notes |
|-------|-------|
| `platform` | Platform ID, max 64 chars, alphanumeric / `_` / `-` / `.` |
| `url` | Must use `http` or `https` |
| `forward_sources` | Controls which non-Matrix source platforms may be forwarded to this integration |
| `capabilities` | Advertised feature list used by bot help / inspection endpoints |
| `owner` | Matrix user auto-invited into portal rooms for this platform |
| `events` | Stored with the webhook, but current message delivery logic is driven by routing and `forward_sources`, not by `events` filtering |

### 2. Create a room mapping

```bash
curl -X POST http://localhost:29320/api/v1/rooms \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "external_room_id": "-1001234567890",
    "room_name": "Telegram / General"
  }'
```

Notes:

- Omit `matrix_room_id` to let the bridge create a new Matrix room.
- Include `matrix_room_id` if you want to bind an existing Matrix room.
- The bridge may auto-create a platform Matrix Space and attach the room to it.

### 3. Send messages into Matrix

Use `room_id`, not `external_room_id`, on `POST /api/v1/message`.

```bash
curl -X POST http://localhost:29320/api/v1/message \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <api_key>" \
  -d '{
    "platform": "telegram",
    "room_id": "-1001234567890",
    "sender": {
      "id": "12345",
      "display_name": "Alice",
      "avatar_url": "https://example.com/alice.jpg"
    },
    "content": {
      "type": "text",
      "body": "Hello from Telegram",
      "html": "<b>Hello from Telegram</b>"
    }
  }'
```

### 4. Receive Matrix messages

When a Matrix user posts in a mapped room, the bridge emits a webhook payload like:

```json
{
  "event": "message",
  "platform": "telegram",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "matrix",
      "external_id": "@alice:example.com",
      "display_name": null,
      "avatar_url": null
    },
    "room": {
      "platform": "telegram",
      "external_id": "-1001234567890",
      "name": null
    },
    "content": {
      "type": "text",
      "body": "Hello from Matrix",
      "formatted_body": "<b>Hello from Matrix</b>"
    },
    "timestamp": 1711234567000,
    "reply_to": null
  }
}
```

Important:

- Outbound text payloads use `formatted_body`, not `html`.
- `source_platform` is added only when the message was cross-forwarded from another external platform.

## WebSocket Integration

Use WebSocket when you want a long-lived outbound subscription instead of webhook delivery.

```text
ws://localhost:29320/api/v1/ws?platform=telegram&forward_sources=*&capabilities=message,image,command
```

Query parameters:

| Parameter | Notes |
|-----------|-------|
| `platform` | Required subscription key |
| `forward_sources` | Optional comma-separated source allowlist |
| `capabilities` | Optional comma-separated capability list |

When `appservice.api_key` is configured, send this as the first frame within 10 seconds:

```json
{ "access_token": "<api_key>" }
```

The bridge closes the connection if authentication is missing or invalid.

## Supported Inbound Content Types

### Text

```json
{ "type": "text", "body": "Hello", "html": "<b>Hello</b>" }
```

### Notice and emote

```json
{ "type": "notice", "body": "Bridge notice" }
{ "type": "emote", "body": "waves" }
```

### Media

```json
{ "type": "image", "url": "mxc://example.com/abc", "caption": "Photo", "mimetype": "image/png" }
{ "type": "file", "url": "mxc://example.com/abc", "filename": "doc.pdf", "mimetype": "application/pdf" }
{ "type": "video", "url": "mxc://example.com/abc", "caption": "Clip", "mimetype": "video/mp4", "duration": 30 }
{ "type": "audio", "url": "mxc://example.com/abc", "mimetype": "audio/ogg", "duration": 5 }
```

### Location

```json
{ "type": "location", "latitude": 48.8566, "longitude": 2.3522 }
```

### Reaction, edit, redaction

```json
{ "type": "reaction", "target_id": "msg-001", "emoji": "👍" }
{ "type": "redaction", "target_id": "msg-001" }
{
  "type": "edit",
  "target_id": "msg-001",
  "new_content": { "type": "text", "body": "corrected text" }
}
```

## Commands and Permissions

### DM commands to the bridge bot

Only users with `admin` permission can use DM commands such as:

- `!help`
- `!platforms`
- `!rooms [platform]`
- `!spaces`
- `!<platform>`
- `!<platform> <command>`

### Room commands

In bridged rooms, the bridge recognizes:

| Command | Requirement |
|---------|-------------|
| `!bridge link <platform> <external_id>` | sender power level >= 50 |
| `!bridge unlink <platform>` | sender power level >= 50 |
| `!bridge status` | no power-level gate |

### Relay behavior

Two controls affect outbound delivery:

1. `appservice.allow_relay`
2. webhook / WS `forward_sources`

`relay_min_power_level` adds a per-room Matrix power-level threshold for normal room-message forwarding.

## Authentication Summary

When `appservice.api_key` is configured:

- HTTP Bridge API: `Authorization: Bearer <api_key>`
- WebSocket: first frame must be `{"access_token":"<api_key>"}` within 10 seconds

Matrix appservice routes use the separate `hs_token` and are not called by your external service.
