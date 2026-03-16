use dashmap::DashMap;
use tracing::{info, warn};

use matrix_bridge_store::Database;

use crate::matrix_client::MatrixClient;

/// Validate an avatar URL. Only mxc:// and https:// are allowed.
fn is_valid_avatar_url(url: &str) -> bool {
    url.starts_with("mxc://") || url.starts_with("https://")
}

/// Manages the lifecycle of Matrix puppet (ghost) users.
///
/// When a message arrives from an external platform, the puppet manager
/// ensures that a corresponding Matrix user exists with the correct
/// display name and avatar.
pub struct PuppetManager {
    matrix_client: MatrixClient,
    db: Database,
    /// Cached puppet data: (matrix_user_id, display_name, avatar_url)
    cache: DashMap<String, (String, Option<String>, Option<String>)>,
    /// Bridge bot device ID for MSC4326 device masquerading.
    /// When set, each puppet is registered with this device so that
    /// encrypted messages sent via `user_id=puppet&device_id=X` are accepted.
    device_id: Option<String>,
}

impl PuppetManager {
    pub fn new(matrix_client: MatrixClient, db: Database, device_id: Option<String>) -> Self {
        Self {
            matrix_client,
            db,
            cache: DashMap::new(),
            device_id,
        }
    }

    /// Ensure a puppet user exists.
    /// Used by the HTTP bridge API where the localpart is computed by the caller.
    pub async fn ensure_puppet_direct(
        &self,
        localpart: &str,
        platform_id: &str,
        external_user_id: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
    ) -> anyhow::Result<String> {
        let cache_key = format!("{platform_id}:{external_user_id}");

        if let Some(entry) = self.cache.get(&cache_key) {
            let (ref user_id, ref cached_name, ref cached_avatar) = *entry;
            let name_changed = display_name != cached_name.as_deref();
            let avatar_changed = avatar_url != cached_avatar.as_deref();
            if !name_changed && !avatar_changed {
                return Ok(user_id.clone());
            }
            // Profile changed — update via homeserver and DB
            if name_changed {
                if let Some(name) = display_name {
                    self.matrix_client.set_display_name(user_id, name).await?;
                }
            }
            if avatar_changed {
                if let Some(url) = avatar_url {
                    if is_valid_avatar_url(url) {
                        self.matrix_client.set_avatar(user_id, url).await?;
                    } else {
                        warn!(url, "rejected invalid avatar URL (only mxc:// and https:// allowed)");
                    }
                }
            }
            self.db.upsert_puppet(user_id, platform_id, external_user_id, display_name, avatar_url).await?;
            let new_entry = (user_id.clone(), display_name.map(|s| s.to_string()), avatar_url.map(|s| s.to_string()));
            drop(entry); // release the read lock before inserting
            self.cache.insert(cache_key, new_entry.clone());
            return Ok(new_entry.0);
        }

        let existing = self
            .db
            .find_puppet_by_external_id(platform_id, external_user_id)
            .await?;

        let matrix_user_id = if let Some(puppet) = existing {
            // Ensure the bridge bot device exists on this puppet (MSC4326).
            // This is a no-op if the device was already created; needed after
            // server restarts when the puppet is in the DB but not yet active.
            if let Some(ref did) = self.device_id {
                self.matrix_client
                    .register_puppet_with_device(localpart, Some(did))
                    .await?;
            }

            let name_changed = display_name != puppet.display_name.as_deref();
            let avatar_changed = avatar_url != puppet.avatar_mxc.as_deref();

            if name_changed && let Some(name) = display_name {
                self.matrix_client
                    .set_display_name(&puppet.matrix_user_id, name)
                    .await?;
            }
            if avatar_changed && let Some(url) = avatar_url {
                if is_valid_avatar_url(url) {
                    self.matrix_client
                        .set_avatar(&puppet.matrix_user_id, url)
                        .await?;
                } else {
                    warn!(url, "rejected invalid avatar URL (only mxc:// and https:// allowed)");
                }
            }
            if name_changed || avatar_changed {
                self.db
                    .upsert_puppet(
                        &puppet.matrix_user_id,
                        platform_id,
                        external_user_id,
                        display_name,
                        avatar_url,
                    )
                    .await?;
            }

            puppet.matrix_user_id
        } else {
            let user_id = if let Some(ref did) = self.device_id {
                self.matrix_client
                    .register_puppet_with_device(localpart, Some(did))
                    .await?
            } else {
                self.matrix_client.register_puppet(localpart).await?
            };
            info!(
                platform = platform_id,
                external_id = external_user_id,
                matrix_user_id = user_id,
                "registered new puppet via HTTP API"
            );

            if let Some(name) = display_name {
                self.matrix_client.set_display_name(&user_id, name).await?;
            }
            if let Some(url) = avatar_url {
                if is_valid_avatar_url(url) {
                    self.matrix_client.set_avatar(&user_id, url).await?;
                } else {
                    warn!(url, "rejected invalid avatar URL (only mxc:// and https:// allowed)");
                }
            }

            self.db
                .upsert_puppet(
                    &user_id,
                    platform_id,
                    external_user_id,
                    display_name,
                    avatar_url,
                )
                .await?;

            user_id
        };

        self.cache.insert(cache_key, (matrix_user_id.clone(), display_name.map(|s| s.to_string()), avatar_url.map(|s| s.to_string())));
        Ok(matrix_user_id)
    }
}
