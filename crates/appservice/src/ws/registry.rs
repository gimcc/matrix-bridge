use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::warn;

use super::CLIENT_CHANNEL_CAPACITY;

/// A connected WebSocket client.
pub(super) struct WsClient {
    pub(super) id: String,
    pub(super) sender: mpsc::Sender<String>,
    pub(super) forward_sources: Vec<String>,
}

impl WsClient {
    /// Check if messages from the given source platform should be forwarded.
    pub(super) fn should_forward_source(&self, source_platform: &str) -> bool {
        matrix_bridge_store::should_forward_source(
            self.forward_sources.iter().map(|s| s.as_str()),
            source_platform,
        )
    }
}

/// Registry of active WebSocket connections, keyed by platform ID.
///
/// Uses `DashMap` with per-shard locking for concurrent access — safe to
/// call from the Dispatcher while holding its own lock.
pub struct WsRegistry {
    clients: DashMap<String, Vec<WsClient>>,
    /// Atomic counter for fast total_clients() without iterating the map.
    count: AtomicUsize,
}

impl Default for WsRegistry {
    fn default() -> Self {
        Self {
            clients: DashMap::new(),
            count: AtomicUsize::new(0),
        }
    }
}

impl WsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to register a new client for the given platform.
    ///
    /// Returns `Ok((client_id, receiver))` on success, or `Err(())` if the
    /// connection limit has been reached. Uses atomic increment-then-check
    /// to avoid TOCTOU races on the connection count.
    pub(super) fn try_register(
        &self,
        platform_id: &str,
        forward_sources: Vec<String>,
        max_clients: usize,
    ) -> Result<(String, mpsc::Receiver<String>), ()> {
        // Atomically increment first, then check. If over limit, roll back.
        let prev = self.count.fetch_add(1, Ordering::AcqRel);
        if prev >= max_clients {
            self.count.fetch_sub(1, Ordering::AcqRel);
            return Err(());
        }

        let id = ulid::Ulid::new().to_string();
        let (tx, rx) = mpsc::channel(CLIENT_CHANNEL_CAPACITY);

        let client = WsClient {
            id: id.clone(),
            sender: tx,
            forward_sources,
        };

        self.clients
            .entry(platform_id.to_string())
            .or_default()
            .push(client);

        Ok((id, rx))
    }

    /// Remove a client from the registry.
    pub(super) fn unregister(&self, platform_id: &str, client_id: &str) {
        if let Some(mut entry) = self.clients.get_mut(platform_id) {
            let before = entry.len();
            entry.retain(|c| c.id != client_id);
            let removed = before - entry.len();
            if removed > 0 {
                self.count.fetch_sub(removed, Ordering::Relaxed);
            }
            if entry.is_empty() {
                drop(entry);
                self.clients.remove(platform_id);
            }
        }
    }

    /// Broadcast a JSON payload to all clients subscribed to the given platform.
    ///
    /// Only delivers to clients whose `forward_sources` allowlist includes
    /// `source_platform`. Uses `try_send` to avoid blocking on slow consumers.
    pub fn broadcast(&self, platform_id: &str, payload: &str, source_platform: Option<&str>) {
        let effective_source = source_platform.unwrap_or("matrix");

        // Phase 1: read lock — iterate and send, collect dead client IDs.
        let closed_ids = {
            let Some(entry) = self.clients.get(platform_id) else {
                return;
            };

            let mut closed = Vec::new();

            for client in entry.iter() {
                if !client.should_forward_source(effective_source) {
                    continue;
                }
                match client.sender.try_send(payload.to_string()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            client_id = client.id,
                            platform = platform_id,
                            "ws client channel full, dropping message"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        closed.push(client.id.clone());
                    }
                }
            }

            closed
        };
        // Read lock dropped here.

        // Phase 2: write lock — remove dead clients (only if needed).
        if !closed_ids.is_empty()
            && let Some(mut entry) = self.clients.get_mut(platform_id)
        {
            let before = entry.len();
            entry.retain(|c| !closed_ids.contains(&c.id));
            let removed = before - entry.len();
            if removed > 0 {
                self.count.fetch_sub(removed, Ordering::Relaxed);
            }
        }
    }

    /// Total number of connected WebSocket clients across all platforms.
    pub fn total_clients(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}
