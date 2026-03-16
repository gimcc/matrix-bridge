# Matrix Bridge Integration Tests

End-to-end integration tests for the Matrix Bridge using
[matrix-bot-sdk](https://github.com/turt2live/matrix-bot-sdk) with full E2EE
(end-to-end encryption) support.

## What is tested

### Inbound (External → Matrix)

| Scenario | Plain Room | Encrypted Room |
|----------|:----------:|:--------------:|
| Text message | x | x |
| HTML formatted text | x | |
| Notice message | x | |
| Emote message | x | |
| Location message | x | |
| Image upload + send | x | x |
| File upload + send | x | x |
| Reaction (as text) | x | |
| Redaction (as notice) | x | |
| Edit (as new message) | x | |

### Outbound (Matrix → Webhook)

| Scenario | Plain Room | Encrypted Room |
|----------|:----------:|:--------------:|
| Text message forwarding | x | x |
| HTML message forwarding | x | |
| Notice forwarding | x | |
| Emote forwarding | x | |
| Location forwarding | x | |
| Image with mxc→HTTP URL | x | x |
| File with mxc→HTTP URL | x | x |
| Video with mxc→HTTP URL | | x |
| Audio with mxc→HTTP URL | | x |
| Encrypted media decryption + re-upload | | x |
| Decrypted file content integrity | | x |

### Encrypted File Roundtrip

- Encrypt → Upload → Download → Decrypt → Verify content matches
- Image roundtrip integrity
- File roundtrip integrity

### Edge Cases

- Duplicate `external_message_id` deduplication
- Large file upload (5MB, skipped in `--quick` mode)

---

## Prerequisites

- **Node.js 18+**
- A running **Matrix homeserver** (Synapse recommended)
- A running **Matrix Bridge** instance
- A **test bot user** registered on the homeserver with an access token

### Creating a test bot user

```bash
# Register a user on Synapse (admin API or register script)
register_new_matrix_user -c /path/to/homeserver.yaml \
  -u integration-test-bot -p <password> --no-admin

# Get an access token
curl -X POST https://matrix.example.com/_matrix/client/v3/login \
  -H 'Content-Type: application/json' \
  -d '{"type":"m.login.password","user":"integration-test-bot","password":"<password>"}'
```

Save the `access_token` and `device_id` from the response.

---

## Setup

```bash
cd examples/integration-test
npm install
```

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `HOMESERVER_URL` | Yes | — | Matrix homeserver URL |
| `BOT_ACCESS_TOKEN` | Yes | — | Access token for the test bot |
| `BOT_USER_ID` | Yes | — | Full Matrix user ID (`@bot:example.com`) |
| `BRIDGE_URL` | Yes | — | Bridge API base URL |
| `BRIDGE_API_KEY` | No | — | Bridge API key (if configured) |
| `TEST_ROOM_ID` | No | auto-create | Existing encrypted room ID |
| `PLATFORM` | No | `integration-test` | Platform ID for bridge mappings |
| `EXTERNAL_ROOM_ID` | No | `test-room` | External room ID for mappings |
| `CRYPTO_DIR` | No | `./crypto-store` | E2EE crypto store directory |
| `STORAGE_FILE` | No | `./bot-storage.json` | Bot SDK storage file |

### Verify Setup

```bash
HOMESERVER_URL=https://matrix.example.com \
BOT_ACCESS_TOKEN=syt_... \
BOT_USER_ID=@integration-test-bot:example.com \
BRIDGE_URL=http://localhost:29320 \
  npm run setup
```

---

## Running Tests

```bash
# Full test suite
HOMESERVER_URL=https://matrix.example.com \
BOT_ACCESS_TOKEN=syt_... \
BOT_USER_ID=@integration-test-bot:example.com \
BRIDGE_URL=http://localhost:29320 \
  npm test

# Quick mode (skip large file tests)
npm run test:quick
```

### Webhook Server

The tests automatically start a temporary HTTP server on a random port to
receive webhook callbacks from the bridge. The webhook URL is registered with
the bridge during setup and cleaned up on teardown.

> **Note:** If the bridge runs in Docker, the webhook URL uses
> `host.docker.internal` to reach the test machine. Adjust the webhook host
> in `run-tests.ts` if your setup differs.

---

## Architecture

```
examples/integration-test/
├── src/
│   ├── config.ts            — Environment variable configuration
│   ├── bridge-client.ts     — Bridge REST API client
│   ├── matrix-test-client.ts — matrix-bot-sdk wrapper with E2EE
│   ├── webhook-server.ts    — Temporary webhook receiver
│   ├── test-harness.ts      — Minimal test framework
│   ├── setup.ts             — Pre-flight environment checker
│   └── run-tests.ts         — Main test runner (all scenarios)
├── package.json
├── tsconfig.json
└── README.md
```

---

## Crypto Store

The E2EE crypto store (`./crypto-store/`) persists Olm/Megolm session keys
across test runs. The store is tied to the bot's `device_id` from the access
token.

**Important:** If you regenerate the access token (new device), delete the
crypto store directory to avoid key mismatches:

```bash
rm -rf ./crypto-store ./bot-storage.json
```
