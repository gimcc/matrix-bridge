# E2E Tests

Integration tests for the matrix-bridge using Bun + Matrix JS SDK.

## Prerequisites

- Running Synapse homeserver at `MATRIX_HOMESERVER_URL` (default: `http://localhost:8008`)
- Running bridge at `BRIDGE_URL` (default: `http://localhost:29320`)
- Admin user on Synapse (default: `admin` / `admin`)

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MATRIX_HOMESERVER_URL` | `http://localhost:8008` | Synapse client-server API |
| `BRIDGE_URL` | `http://localhost:29320` | Bridge HTTP API |
| `MATRIX_DOMAIN` | `im.fr.ds.cc` | Homeserver domain |
| `MATRIX_ADMIN_USER` | `admin` | Synapse admin username |
| `MATRIX_ADMIN_PASSWORD` | `admin` | Synapse admin password |
| `BRIDGE_BOT_LOCALPART` | `bridge_bot` | Bridge bot user localpart |
| `BRIDGE_HS_TOKEN` | `CHANGE_ME_HS_TOKEN` | Homeserver token for appservice auth |

## Running

```bash
cd tests/e2e
bun install
bun test                    # Run all tests
bun run test:health         # Health check only
bun run test:rooms          # Room mapping CRUD
bun run test:msg            # External -> Matrix messaging
bun run test:outbound       # Matrix -> External via webhook
bun run test:errors         # HTTP error code mapping
bun run test:exclude        # Webhook source exclusion
bun run test:e2ee           # E2EE flow
bun run test:migration      # Idempotent migration
```

## Test Plan

| # | Test File | Direction | Covers |
|---|---|---|---|
| 01 | health | - | Bridge health endpoint |
| 02 | room-mapping | - | CRUD + upsert (Bug #5) |
| 03 | message-bridge | External -> Matrix | Puppet creation, message delivery, error codes (Bug #1) |
| 04 | matrix-to-external | Matrix -> External | Transaction processing, webhook delivery, dedup, bot self-skip |
| 05 | error-codes | - | BridgeError -> HTTP status mapping (Bug #1) |
| 06 | webhook-exclude | Matrix -> External | forward_sources allowlist filtering, cross-platform puppet forwarding |
| 07 | e2ee-flow | Both | Encryption state tracking, decrypt attempt, to-device resilience |
| 08 | idempotent-migration | - | Migration 004 doesn't crash on restart (Bug #4) |
