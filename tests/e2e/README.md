# E2E Tests

Bun-based end-to-end tests for the bridge HTTP API and Matrix appservice flow.

## Prerequisites

- Running Synapse at `MATRIX_HOMESERVER_URL` (default: `http://localhost:8008`)
- Running bridge at `BRIDGE_URL` (default: `http://localhost:29320`)
- A Synapse admin user available to create test users and rooms
- The bridge `hs_token`, because some tests simulate homeserver pushes directly

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MATRIX_HOMESERVER_URL` | `http://localhost:8008` | Synapse client-server API |
| `BRIDGE_URL` | `http://localhost:29320` | Bridge HTTP API base URL |
| `MATRIX_DOMAIN` | `im.fr.ds.cc` | Homeserver domain used in test MXIDs |
| `MATRIX_ADMIN_USER` | `admin` | Synapse admin username |
| `MATRIX_ADMIN_PASSWORD` | `admin` | Synapse admin password |
| `BRIDGE_BOT_LOCALPART` | `bridge_bot` | Must match `appservice.sender_localpart` |
| `BRIDGE_HS_TOKEN` | `CHANGE_ME_HS_TOKEN` | Appservice token used for transaction simulation |

## Running

```bash
cd tests/e2e
bun install
bun test
```

Available shortcut scripts include:

- `bun run test:health`
- `bun run test:rooms`
- `bun run test:msg`
- `bun run test:outbound`
- `bun run test:e2ee`
- `bun run test:multi`
- `bun run test:commands`
- `bun run test:edge`

## Coverage

The suite currently covers:

- Health and admin endpoints
- Room mapping CRUD and auto-created portal rooms
- External -> Matrix message flow
- Matrix -> webhook forwarding
- Webhook filtering and multiple-webhook behavior
- E2EE flow, media types, redactions, and migration idempotency
- Bot commands, auto-join, concurrency, and edge cases
