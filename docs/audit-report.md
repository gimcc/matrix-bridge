# Audit Report

## 2026-03-27

Scope: static review of the current workspace head, focused on message bridging, persistence, and identity mapping paths.

### Findings

#### 1. High: inbound external messages are not idempotent

- The HTTP inbound path sends the Matrix event before persisting the external-to-Matrix mapping in [crates/appservice/src/dispatcher/platform_events.rs](../crates/appservice/src/dispatcher/platform_events.rs).
- The mapping insert in [crates/store/src/message_mapping.rs](../crates/store/src/message_mapping.rs) relies on the schema constraint in [crates/store/src/migrations/001_initial.sql](../crates/store/src/migrations/001_initial.sql), which is `UNIQUE(platform_id, external_message_id)`.
- This means a retry of the same external message, or a platform that only guarantees message IDs within a room, can create a new Matrix event first and only then fail on the database write.

Impact:
- Duplicate Matrix messages can be produced on retries.
- The API can return a server error after already delivering the message.
- Follow-up operations such as edit or redaction can become inconsistent because the mapping write did not complete.

Key references:
- [crates/appservice/src/dispatcher/platform_events.rs#L157](../crates/appservice/src/dispatcher/platform_events.rs#L157)
- [crates/appservice/src/dispatcher/platform_events.rs#L170](../crates/appservice/src/dispatcher/platform_events.rs#L170)
- [crates/store/src/message_mapping.rs#L16](../crates/store/src/message_mapping.rs#L16)
- [crates/store/src/migrations/001_initial.sql#L13](../crates/store/src/migrations/001_initial.sql#L13)

#### 2. High: room mappings with message history cannot be deleted

- `message_mappings.room_mapping_id` references `room_mappings(id)` in [crates/store/src/migrations/001_initial.sql](../crates/store/src/migrations/001_initial.sql), but the foreign key does not use `ON DELETE CASCADE`.
- SQLite foreign keys are enabled in [crates/store/src/db.rs](../crates/store/src/db.rs).
- The delete path in [crates/store/src/room_mapping.rs](../crates/store/src/room_mapping.rs) performs a raw delete without first removing dependent message mappings.
- Both the REST handler and the `!bridge unlink` command route directly into that delete path.

Impact:
- A room mapping becomes effectively undeletable after the room has bridged any message.
- The REST API returns 500 instead of a usable result.
- Operator-facing unlink flows break in normal production usage.

Key references:
- [crates/store/src/migrations/001_initial.sql#L18](../crates/store/src/migrations/001_initial.sql#L18)
- [crates/store/src/db.rs#L29](../crates/store/src/db.rs#L29)
- [crates/store/src/room_mapping.rs#L261](../crates/store/src/room_mapping.rs#L261)
- [crates/appservice/src/bridge_api/room_handlers.rs#L292](../crates/appservice/src/bridge_api/room_handlers.rs#L292)
- [crates/appservice/src/dispatcher/commands.rs#L75](../crates/appservice/src/dispatcher/commands.rs#L75)

#### 3. Medium: puppet identity mapping can collapse distinct external users

- Matrix puppet localparts are normalized to lowercase in [crates/core/src/platform.rs](../crates/core/src/platform.rs).
- Puppet lookup and caching still use the original `external_user_id` in [crates/appservice/src/puppet_manager.rs](../crates/appservice/src/puppet_manager.rs).
- When two external users differ only by case, they can resolve to the same Matrix localpart.
- On collision, registration treats `M_USER_IN_USE` as reuse of the existing Matrix user in [crates/appservice/src/matrix_client/puppets.rs](../crates/appservice/src/matrix_client/puppets.rs).
- The upsert logic in [crates/store/src/puppet_store.rs](../crates/store/src/puppet_store.rs) updates profile fields only and does not rewrite `platform_id` or `external_user_id` on conflict by `matrix_user_id`.

Impact:
- Distinct external users can be silently mapped to the same puppet.
- Database state can continue to point at the old external identity.
- Future lookups, auditability, and moderation trails can become incorrect.

Key references:
- [crates/core/src/platform.rs#L11](../crates/core/src/platform.rs#L11)
- [crates/core/src/platform.rs#L21](../crates/core/src/platform.rs#L21)
- [crates/appservice/src/puppet_manager.rs#L49](../crates/appservice/src/puppet_manager.rs#L49)
- [crates/appservice/src/puppet_manager.rs#L80](../crates/appservice/src/puppet_manager.rs#L80)
- [crates/appservice/src/matrix_client/puppets.rs#L44](../crates/appservice/src/matrix_client/puppets.rs#L44)
- [crates/store/src/puppet_store.rs#L17](../crates/store/src/puppet_store.rs#L17)

#### 4. Medium: webhook upsert returns an unreliable identifier

- The webhook registration path in [crates/store/src/webhook_store.rs](../crates/store/src/webhook_store.rs) uses `INSERT ... ON CONFLICT DO UPDATE`.
- The function then returns `last_insert_rowid()`.
- In the update branch, that value is not guaranteed to be the primary key of the updated webhook row.
- The API returns that `id` directly to callers.

Impact:
- Callers can receive an incorrect webhook identifier after updating an existing registration.
- Follow-up actions that rely on the returned `id` can target the wrong row or fail unexpectedly.

Key references:
- [crates/store/src/webhook_store.rs#L77](../crates/store/src/webhook_store.rs#L77)
- [crates/store/src/webhook_store.rs#L98](../crates/store/src/webhook_store.rs#L98)
- [crates/appservice/src/bridge_api/webhook_handlers.rs#L17](../crates/appservice/src/bridge_api/webhook_handlers.rs#L17)

### Review Notes

- This report is based on static analysis only.
- No new integration or end-to-end verification was run for this report.
- Existing test coverage does not appear to cover the retry/idempotency, delete-with-history, or case-collision scenarios above.
