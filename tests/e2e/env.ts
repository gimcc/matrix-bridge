/**
 * Environment configuration for E2E tests.
 *
 * Reads from environment variables with sensible defaults for a local
 * docker-compose setup (Synapse + bridge).
 */
export const env = {
  /** Synapse homeserver URL (client-server API). */
  homeserverUrl: process.env.MATRIX_HOMESERVER_URL ?? "http://localhost:8008",
  /** Bridge HTTP API base URL. */
  bridgeUrl: process.env.BRIDGE_URL ?? "http://localhost:29320",
  /** Homeserver domain (for user IDs). */
  domain: process.env.MATRIX_DOMAIN ?? "im.fr.ds.cc",
  /** Admin user credentials (must be a Synapse admin). */
  adminUser: process.env.MATRIX_ADMIN_USER ?? "admin",
  adminPassword: process.env.MATRIX_ADMIN_PASSWORD ?? "admin",
  /** Bridge bot localpart (must match config.toml sender_localpart). */
  bridgeBotLocalpart: process.env.BRIDGE_BOT_LOCALPART ?? "bridge_bot",
  /** Bridge appservice hs_token (for simulating homeserver pushes). */
  hsToken: process.env.BRIDGE_HS_TOKEN ?? "CHANGE_ME_HS_TOKEN",
};
