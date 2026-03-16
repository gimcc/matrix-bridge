# Matrix Bridge Webhook Integration Examples

Demo implementations showing how external platforms integrate with the Matrix
Bridge via HTTP webhooks.

## What these demos do

Each demo (Python, Node.js, and Bun) is a standalone HTTP server that:

1. **Receives webhook callbacks** (or subscribes via **WebSocket**) from the bridge
   when messages appear in a bridged Matrix room (outbound: Matrix -> External).
2. **Sends messages** to Matrix through the bridge REST API (inbound:
   External -> Matrix).
3. **Uploads and sends media** (images, files) via a two-step upload flow.
4. **Handles cross-platform forwarded messages** by inspecting the
   `source_platform` field in the webhook payload.
5. **Auto-registers** itself with the bridge on startup (webhook URL + room
   mapping).

---

## Configuration

All configuration is done through environment variables:

| Variable       | Default                    | Description                           |
|----------------|----------------------------|---------------------------------------|
| `BRIDGE_URL`   | `http://localhost:29320`   | Base URL of the bridge API            |
| `PLATFORM`     | `myapp`                    | Platform identifier for this app      |
| `ROOM_ID`      | `general`                  | External room ID to bridge            |
| `MATRIX_ROOM_ID` | *(auto-create)*          | Matrix room ID (optional; auto-created if omitted) |
| `WEBHOOK_PORT` | `5050`                     | Port the demo server listens on       |
| `WEBHOOK_HOST` | `http://localhost:5050`    | Public URL the bridge can reach       |

---

## Running the Python demo

```bash
cd examples/python
pip install -r requirements.txt

# Start the server
python webhook_demo.py

# Start and send a test message
python webhook_demo.py --send-test
```

Requires Python 3.10+.

## Running the Node.js demo

```bash
cd examples/nodejs
npm install

# Start the server
npm start

# Start and send a test message
npm run start:test
```

Requires Node.js 18+.

## Running the Bun chat demo

```bash
cd examples/bun

# Optional: specify an existing Matrix room
# export MATRIX_ROOM_ID='!your_room:example.com'

bun run server.ts
```

Opens a web chat UI at `http://localhost:3030` that bridges to Matrix in
real time via SSE. Requires Bun 1.1+.

## Running the integration tests

```bash
cd examples/integration-test
npm install

HOMESERVER_URL=https://matrix.example.com \
BOT_ACCESS_TOKEN=syt_... \
BOT_USER_ID=@testbot:example.com \
BRIDGE_URL=http://localhost:29320 \
  npm test
```

E2E integration tests using `matrix-bot-sdk` with full E2EE support. Tests
inbound/outbound message flows, encrypted file upload/download/decryption,
webhook delivery, and media URL conversion. See
[integration-test/README.md](integration-test/README.md) for details.

Requires Node.js 18+.

---

## API Reference (quick summary)

### Webhook callback payload (bridge -> your app)

```json
{
  "event": "message",
  "platform": "myapp",
  "source_platform": "telegram",
  "message": {
    "id": "$event_id",
    "sender": {
      "platform": "telegram",
      "external_id": "user123",
      "display_name": "Alice",
      "avatar_url": "mxc://..."
    },
    "room": { "platform": "myapp", "external_id": "general" },
    "content": { "type": "text", "body": "Hello!" },
    "timestamp": 1710000000000
  }
}
```

The `source_platform` field is **only present** for cross-platform forwarded
messages. When absent, the message originates from Matrix directly.

### Send message (your app -> bridge)

```
POST /api/v1/message
```

```json
{
  "platform": "myapp",
  "room_id": "general",
  "sender": { "id": "bot", "display_name": "My Bot" },
  "content": { "type": "text", "body": "Hello from external!" }
}
```

### Upload media

```
POST /api/v1/upload   (multipart/form-data, field name: "file")
```

Returns a JSON object containing the `content_uri` to use in a subsequent message
with `content.type` set to `"image"` (or other media type).

### Register webhook

```
POST /api/v1/webhooks
```

```json
{ "platform": "myapp", "url": "http://localhost:5050/webhook" }
```

### Create room mapping

```
POST /api/v1/rooms
```

```json
{ "platform": "myapp", "external_room_id": "general" }
```

`matrix_room_id` is optional. When omitted the bridge auto-creates a Matrix room.
The response always includes the `matrix_room_id` that was used:

```json
{ "id": 1, "matrix_room_id": "!newroom:example.com" }
```

---

## Expected behavior

1. Start the bridge (`http://localhost:29320`).
2. Start one of the demo servers.
3. The demo auto-registers its webhook and room mapping.
4. Messages sent in the bridged Matrix room are forwarded to the demo's
   `/webhook` endpoint and logged to the console.
5. Use `--send-test` to have the demo push a test message into the Matrix
   room on startup.
