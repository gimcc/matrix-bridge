# End-to-Bridge Encryption (E2BE)

## Overview

Matrix Bridge implements end-to-bridge encryption using `matrix-sdk-crypto` (OlmMachine) directly, without the full `matrix-sdk` client. This is the recommended approach for Rust appservices since `matrix-sdk-appservice` was removed from the SDK in September 2023.

## Architecture

### Current: Single Bridge Bot Device

All encryption operations use a single `OlmMachine` instance for the bridge bot user. Puppet users send encrypted messages through the bridge bot's device via MSC3202/MSC4190 device masquerading.

```
┌─────────────┐     ┌──────────────┐     ┌────────────┐
│  Homeserver  │────▶│ CryptoManager │────▶│ OlmMachine │
│  (Synapse)   │◀────│  (bridge bot) │◀────│  (single)  │
└─────────────┘     └──────────────┘     └────────────┘
       │                    │
       │  MSC3202           │  Encrypts for all puppets
       │  device_id QP      │  via device masquerading
       ▼                    ▼
  ┌─────────┐        ┌──────────┐
  │ Puppet A │        │ Puppet B │
  │ (no own  │        │ (no own  │
  │  device) │        │  device) │
  └─────────┘        └──────────┘
```

### Per-User Crypto (Optional)

Each puppet can have its own `OlmMachine` with independent device keys. Enable with `per_user_crypto = true` in config. This eliminates the MismatchedSender warning and improves client compatibility.

```
┌─────────────────────────────────────────────┐
│            CryptoManagerPool                │
│                                             │
│  bot: Arc<CryptoManager>  (always init)     │
│  puppets: HashMap<UserId, Arc<CryptoMgr>>   │
│           (lazily initialized)              │
│                                             │
│  to-device routing:                         │
│    txn.to_user_id → lookup OlmMachine       │
│    → receive_sync_changes per machine       │
│                                             │
│  encrypt: puppet's own OlmMachine           │
│  decrypt: bot's OlmMachine (always in room) │
└─────────────────────────────────────────────┘
```

**Trade-offs:**

| | Single Device (default) | Per-User Crypto |
|---|---|---|
| Config | `per_user_crypto = false` | `per_user_crypto = true` |
| OlmMachine instances | 1 | 1 per active puppet (lazy) |
| Crypto stores | 1 SQLite DB | `{crypto_store}/puppets/{localpart}/` per puppet |
| MSC dependency | MSC3202/MSC4190 for masquerading | MSC3202 for to-device routing |
| Client compatibility | MismatchedSender warning | All clients work normally |
| to-device routing | All events go to bridge bot | Routed by `to_user_id` |
| OTK management | Single pool | Per-user pools from MSC3202 transaction |
| Device ID | Configured `device_id` | `{puppet_device_prefix}_{sha256(localpart)[0:16]}` |

## Encryption Flow

### Startup

```
1. Load config (EncryptionConfig)
2. Set device_id on MatrixClient for MSC3202
3. Register bridge bot with device (register_puppet_with_device)
4. Initialize CryptoManager:
   a. Open/create SqliteCryptoStore with passphrase
   b. Build OlmMachine with user_id + device_id
   c. Process outgoing requests (upload device keys)
   d. Verify device keys on homeserver
   e. If keys missing: rebuild crypto store from scratch
5. Wire crypto into Dispatcher
```

### Inbound Encrypted Message (decrypt)

```
Transaction received (PUT /_matrix/app/v1/transactions/{txnId})
  │
  ├─ 1. Process MSC2409/3202 crypto data (always, even if empty):
  │     - to-device events (Olm key exchange)
  │     - device list changes
  │     - OTK counts → replenish if low
  │     - fallback key types
  │
  ├─ 2. receive_sync_changes() [holds write lock]
  │     → OlmMachine processes key exchange
  │     → process_outgoing_requests() (key claims, uploads)
  │
  ├─ 3. For each m.room.encrypted event:
  │     a. Ensure room tracked as encrypted
  │     b. Update tracked users (device key queries)
  │     c. decrypt() → DecryptedEvent
  │     d. Route decrypted inner event (m.room.message → webhooks)
  │
  └─ 4. Flush outgoing crypto requests
```

### Outbound Encrypted Message (encrypt)

```
send_to_matrix() called (from bridge API or webhook response)
  │
  ├─ 1. Check room encryption (local store → server state event)
  │
  ├─ 2. encrypt() [holds write lock through entire flow]:
  │     a. update_tracked_users(room_members)
  │     b. process_keys_query_requests() — get device keys
  │     c. claim_missing_sessions() — establish Olm sessions
  │     d. share_room_key() — distribute Megolm session key
  │     e. encrypt_room_event_raw() — encrypt content
  │     f. process_outgoing_requests() — flush remaining
  │
  └─ 3. send_encrypted_message() with device_id query param
```

## Concurrency Model

A `tokio::sync::RwLock` serializes crypto operations to prevent races between sync processing and encryption (inspired by matrix-bot-sdk's `AsyncLock`):

- **`receive_sync_changes()`** — acquires write lock
- **`encrypt()`** — acquires write lock through all 5 preparation steps + encryption
- **`update_tracked_users()`** — acquires write lock
- **`decrypt()`** — no lock needed (read-only from OlmMachine's perspective)

## Room Encryption Detection

Inspired by matrix-bot-sdk's `RoomTracker`:

1. **Fast path:** Check local crypto store (`is_room_encrypted_local`)
2. **Server query:** `GET /rooms/{roomId}/state/m.room.encryption/`
3. **Auto-sync:** If server says encrypted but local store disagrees, update local state

This handles rooms encrypted after bridge startup or rooms the bridge joins mid-lifecycle.

## MSC Field Name Compatibility

The transaction handler supports both unstable and stable MSC prefixes:

| Data | Unstable prefix | Stable prefix |
|------|----------------|---------------|
| to-device events | `de.sorunome.msc2409.to_device` | `org.matrix.msc2409.to_device` |
| device lists | `de.sorunome.msc3202.device_lists` | `org.matrix.msc3202.device_lists` |
| OTK counts | `de.sorunome.msc3202.device_one_time_keys_count` | `org.matrix.msc3202.device_one_time_keys_count` |
| fallback keys | `de.sorunome.msc3202.device_unused_fallback_key_types` | `org.matrix.msc3202.device_unused_fallback_key_types` |

Also handles the `device_one_time_key_counts` (with `s`) variant used by some Synapse versions.

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `matrix-sdk-crypto` | 0.16 | OlmMachine, E2EE state machine |
| `matrix-sdk-sqlite` | 0.16 | SqliteCryptoStore (passphrase-encrypted) |
| `ruma` | 0.14 | Matrix protocol types (events, API requests/responses) |

## Key Files

| File | Responsibility |
|------|----------------|
| `crates/appservice/src/crypto_manager.rs` | OlmMachine wrapper, encrypt/decrypt, key management |
| `crates/appservice/src/matrix_client.rs` | HTTP calls for keys/upload, keys/query, keys/claim, sendToDevice |
| `crates/appservice/src/server.rs` | Transaction handler — MSC2409/3202 data extraction |
| `crates/appservice/src/dispatcher.rs` | Event routing with encrypt/decrypt integration |
| `crates/core/src/config.rs` | `EncryptionConfig` struct |
| `crates/core/src/registration.rs` | MSC2409/3202/4190 registration YAML fields |

## References

- [matrix-bot-sdk encryption implementation](https://github.com/turt2live/matrix-bot-sdk/tree/main/src/e2ee) — TypeScript reference using `@matrix-org/matrix-sdk-crypto-nodejs`
- [MSC2409](https://github.com/matrix-org/matrix-spec-proposals/pull/2409) — Appservice to-device events
- [MSC3202](https://github.com/matrix-org/matrix-spec-proposals/pull/3202) — Appservice device list/OTK counts
- [MSC4190](https://github.com/matrix-org/matrix-spec-proposals/pull/4190) — Appservice device management
