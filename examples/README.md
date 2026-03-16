# Matrix Bridge Examples

This directory contains small integration demos that exercise the current Bridge API.

## Examples

| Directory | Runtime | Description |
|-----------|---------|-------------|
| [`webhook/`](webhook/) | Bun | Minimal webhook receiver that registers itself and logs outbound bridge events |
| [`websocket/`](websocket/) | Bun | WebSocket client with capability declaration and command handling |
| [`bun/`](bun/) | Bun | Browser chat demo using webhook callbacks plus SSE for the browser UI |
| [`integration-test/`](integration-test/) | Node.js | Matrix SDK based integration suite with E2EE coverage |

## Notes Before Running

- `examples/webhook` and `examples/websocket` support `API_KEY` and send `Authorization: Bearer <API_KEY>` when configured.
- `examples/bun` does not currently add Bridge API authentication headers, so it only works as-is when `appservice.api_key` is unset.
- `examples/integration-test` is Node.js-based, not Bun-based.

## Quick Start

### Webhook receiver

```bash
cd examples/webhook
bun install
BRIDGE_URL=http://localhost:29320 bun run server.ts
```

### WebSocket client

```bash
cd examples/websocket
bun install
BRIDGE_URL=http://localhost:29320 bun run client.ts
```

### Chat demo

```bash
cd examples/bun
bun install
BRIDGE_URL=http://localhost:29320 bun run server.ts
```

Then open `http://localhost:3030`.

### Integration tests

```bash
cd examples/integration-test
npm install
npm run setup
npm test
```

## Shared Bridge Concepts

All examples use the same core endpoints:

- `POST /api/v1/rooms` to create or reuse a mapping
- `POST /api/v1/webhooks` to register outbound delivery
- `POST /api/v1/message` to inject external messages into Matrix
- `GET /api/v1/ws` for persistent outbound subscriptions

Refer to [../docs/api-reference.md](../docs/api-reference.md) for exact payloads.
