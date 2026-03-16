use serde_json::Value;
use tracing::warn;

use super::Dispatcher;

impl Dispatcher {
    /// Enable encryption on a room and register it with the crypto manager.
    pub async fn enable_room_encryption(&self, room_id: &str) -> anyhow::Result<()> {
        self.matrix_client.enable_room_encryption(room_id).await?;
        if let Some(pool) = &self.crypto_pool {
            let ruma_room_id: &ruma::RoomId = room_id.try_into()?;
            pool.bot().set_room_encrypted(ruma_room_id).await?;
        }
        Ok(())
    }

    /// Track that a room uses encryption and update member device keys.
    ///
    /// Called when an `m.room.encryption` state event is received.
    /// No-op when crypto is disabled.
    pub(super) async fn track_room_encryption(&self, room_id: &str) {
        let Some(pool) = &self.crypto_pool else {
            return;
        };
        let ruma_room_id: Result<&ruma::RoomId, _> = room_id.try_into();
        let Ok(ruma_room_id) = ruma_room_id else {
            return;
        };
        if let Err(e) = pool.bot().set_room_encrypted(ruma_room_id).await {
            warn!(room_id, error = %e, "failed to mark room as encrypted");
        }
        if let Err(e) = self.update_tracked_users(room_id).await {
            warn!(room_id, error = %e, "failed to track users on encryption event");
        }
    }

    /// Ensure a room is marked as encrypted in the local crypto store.
    ///
    /// Called before decryption to handle the case where we missed the
    /// `m.room.encryption` state event.
    pub(super) async fn ensure_room_encrypted(&self, room_id: &str) {
        let Some(pool) = &self.crypto_pool else {
            return;
        };
        let ruma_room_id: Result<&ruma::RoomId, _> = room_id.try_into();
        let Ok(ruma_room_id) = ruma_room_id else {
            return;
        };
        if !pool.bot().is_room_encrypted_local(ruma_room_id).await {
            if let Err(e) = pool.bot().set_room_encrypted(ruma_room_id).await {
                warn!(room_id, error = %e, "failed to mark room as encrypted");
            }
        }
    }

    /// Track a single member's device keys in an encrypted room.
    ///
    /// Called on `m.room.member` join/invite events. No-op when the room
    /// is not encrypted or crypto is disabled.
    pub(super) async fn track_member_device(&self, room_id: &str, state_key: &str) {
        let Some(pool) = &self.crypto_pool else {
            return;
        };
        let ruma_room_id: Result<&ruma::RoomId, _> = room_id.try_into();
        let Ok(ruma_room_id) = ruma_room_id else {
            return;
        };
        if !pool
            .bot()
            .is_room_encrypted(ruma_room_id, &self.matrix_client)
            .await
        {
            return;
        }
        if let Ok(user_id) = state_key.parse::<ruma::OwnedUserId>() {
            if let Err(e) = pool.bot().update_tracked_users(&[user_id]).await {
                warn!(room_id, state_key, error = %e, "failed to track member devices");
            }
        }
    }

    /// Track all room members' device keys if the room is encrypted.
    ///
    /// Called when the bot joins a room.
    pub(super) async fn track_room_members_if_encrypted(&self, room_id: &str) {
        let Some(pool) = &self.crypto_pool else {
            return;
        };
        let ruma_room_id: Result<&ruma::RoomId, _> = room_id.try_into();
        let Ok(ruma_room_id) = ruma_room_id else {
            return;
        };
        if pool
            .bot()
            .is_room_encrypted(ruma_room_id, &self.matrix_client)
            .await
        {
            if let Err(e) = self.update_tracked_users(room_id).await {
                warn!(room_id, error = %e, "failed to track users after bot join");
            }
        }
    }

    /// Track all room members' device keys (unconditional).
    ///
    /// Called before encrypt/decrypt to ensure device keys are available.
    pub(super) async fn track_all_room_members(&self, room_id: &str) {
        if let Err(e) = self.update_tracked_users(room_id).await {
            warn!(
                room_id,
                error = %e, "failed to update tracked users"
            );
        }
    }

    /// Query device keys for all members of a room.
    async fn update_tracked_users(&self, room_id: &str) -> anyhow::Result<()> {
        let Some(pool) = &self.crypto_pool else {
            return Ok(());
        };
        let members_str = self.matrix_client.get_room_members(room_id).await?;
        let members: Vec<ruma::OwnedUserId> =
            members_str.iter().filter_map(|m| m.parse().ok()).collect();
        if !members.is_empty() {
            pool.bot().update_tracked_users(&members).await?;
        }
        Ok(())
    }

    /// Send a message to a Matrix room, encrypting if the room has encryption enabled.
    ///
    /// When `encrypted_hint` is `Some(true)`, the room is assumed encrypted without
    /// re-querying — this avoids TOCTOU races between attachment encryption and
    /// event encryption. When `None`, the room state is checked dynamically.
    ///
    /// In per-user crypto mode, the puppet's own OlmMachine is used for encryption
    /// and the puppet's device_id is sent in the request.
    pub(super) async fn send_to_matrix(
        &self,
        room_id: &str,
        content: &Value,
        as_user: &str,
        txn_id: &str,
        encrypted_hint: Option<bool>,
    ) -> anyhow::Result<String> {
        if let Some(pool) = &self.crypto_pool {
            let ruma_room_id: &ruma::RoomId = room_id.try_into()?;
            let is_encrypted = match encrypted_hint {
                Some(hint) => hint,
                None => {
                    pool.bot()
                        .is_room_encrypted(ruma_room_id, &self.matrix_client)
                        .await
                }
            };
            if is_encrypted {
                let members_str = self.matrix_client.get_room_members(room_id).await?;
                let members: Vec<ruma::OwnedUserId> =
                    members_str.iter().filter_map(|m| m.parse().ok()).collect();

                // In per-user mode, use the puppet's own OlmMachine.
                let sender_user_id: ruma::OwnedUserId = as_user.parse()?;
                let encrypted = pool
                    .encrypt(
                        &sender_user_id,
                        ruma_room_id,
                        "m.room.message",
                        content,
                        &members,
                    )
                    .await?;

                // Use per-user MatrixClient for sending if in per-user mode.
                if pool.is_per_user()
                    && let Some(device_id) = pool.device_id_for_user(&sender_user_id).await
                {
                    let puppet_client = self.matrix_client.with_user_device(as_user, &device_id);
                    return puppet_client
                        .send_encrypted_message(room_id, &encrypted, as_user, txn_id)
                        .await;
                }

                return self
                    .matrix_client
                    .send_encrypted_message(room_id, &encrypted, as_user, txn_id)
                    .await;
            }
        }
        self.matrix_client
            .send_message(room_id, content, as_user, txn_id)
            .await
    }
}
