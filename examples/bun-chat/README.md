# Bun Chat Demo

A minimal web chat UI that bridges to Matrix via the Matrix Bridge HTTP API.
Messages typed in the browser appear in the Matrix room as a puppet user;
messages from Matrix users are pushed to the browser in real time via SSE.

## Requirements

- [Bun](https://bun.sh) 1.1+
- A running Matrix Bridge instance

## Quick Start

```bash
cd examples/bun-chat

export BRIDGE_URL=http://localhost:29320
export MATRIX_ROOM_ID='!your_room:example.com'

bun run server.ts
```

Open `http://localhost:3030` in your browser.

## Environment Variables

| Variable         | Default                  | Description                        |
|------------------|--------------------------|------------------------------------|
| `BRIDGE_URL`     | `http://localhost:29320` | Base URL of the bridge API         |
| `PLATFORM`       | `web`                    | Platform identifier                |
| `ROOM_ID`        | `general`                | External room ID to bridge         |
| `MATRIX_ROOM_ID` | `!changeme:example.com`  | Matrix room ID                     |
| `PORT`           | `3030`                   | Port the chat server listens on    |
| `HOST`           | `http://localhost:3030`  | Public URL the bridge can reach    |

## How It Works

```
Browser (SSE) <---- server.ts <---- Bridge webhook (POST /webhook)
Browser (form) ----> server.ts ----> Bridge API (POST /api/v1/message)
```

1. On startup, the server registers a webhook and room mapping with the bridge.
2. When a Matrix user sends a message, the bridge POSTs to `/webhook`.
   The server broadcasts it to all connected browsers via SSE.
3. When you type a message, the browser POSTs to `/send`.
   The server forwards it to the bridge, which delivers it to Matrix as a puppet user.
