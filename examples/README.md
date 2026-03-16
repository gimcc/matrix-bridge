# Matrix Bridge Webhook Integration Examples

Demo implementations showing how external platforms integrate with the Matrix
Bridge via HTTP webhooks.

## What these demos do

Each demo (Python and Node.js) is a standalone HTTP server that:

1. **Receives webhook callbacks** from the bridge when messages appear in a
   bridged Matrix room (outbound: Matrix -> External).
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

Returns a JSON object containing the `mxc_url` to use in a subsequent message
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
{ "platform": "myapp", "external_id": "general" }
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
