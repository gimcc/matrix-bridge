use dashmap::DashMap;
use tracing::info;

use matrix_bridge_store::Database;

use crate::matrix_client::MatrixClient;

/// Manages the lifecycle of Matrix puppet (ghost) users.
///
/// When a message arrives from an external platform, the puppet manager
/// ensures that a corresponding Matrix user exists with the correct
/// display name and avatar.
pub struct PuppetManager {
    matrix_client: MatrixClient,
    db: Database,
    /// In-memory cache of already-ensured puppet user IDs.
    cache: DashMap<String, String>,
}

impl PuppetManager {
    pub fn new(matrix_client: MatrixClient, db: Database) -> Self {
        Self {
            matrix_client,
            db,
            cache: DashMap::new(),
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

        if let Some(user_id) = self.cache.get(&cache_key) {
            return Ok(user_id.clone());
        }

        let existing = self
            .db
            .find_puppet_by_external_id(platform_id, external_user_id)
            .await?;

        let matrix_user_id = if let Some(puppet) = existing {
            let name_changed = display_name != puppet.display_name.as_deref();
            let avatar_changed = avatar_url != puppet.avatar_mxc.as_deref();

            if name_changed && let Some(name) = display_name {
                self.matrix_client
                    .set_display_name(&puppet.matrix_user_id, name)
                    .await?;
            }
            if avatar_changed && let Some(url) = avatar_url {
                self.matrix_client
                    .set_avatar(&puppet.matrix_user_id, url)
                    .await?;
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
            let user_id = self.matrix_client.register_puppet(localpart).await?;
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
                self.matrix_client.set_avatar(&user_id, url).await?;
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

        self.cache.insert(cache_key, matrix_user_id.clone());
        Ok(matrix_user_id)
    }
}
