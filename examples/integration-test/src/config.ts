/**
 * Integration test configuration — all values from environment variables.
 *
 * Required:
 *   HOMESERVER_URL   — Matrix homeserver URL (e.g. https://matrix.example.com)
 *   BOT_ACCESS_TOKEN — Access token for the test bot user
 *   BOT_USER_ID      — Full Matrix user ID (@testbot:example.com)
 *   BRIDGE_URL       — Bridge API base URL (e.g. http://localhost:29320)
 *
 * Optional:
 *   BRIDGE_API_KEY   — API key for the bridge (if configured)
 *   TEST_ROOM_ID     — Existing encrypted room to use; auto-created if omitted
 *   PLATFORM         — Platform ID for bridge integration (default: "integration-test")
 *   EXTERNAL_ROOM_ID — External room ID for the bridge mapping (default: "test-room")
 *   CRYPTO_DIR       — Directory for E2EE crypto store (default: "./crypto-store")
 *   STORAGE_FILE     — Path to the bot SDK storage file (default: "./bot-storage.json")
 */

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing required environment variable: ${name}`);
    process.exit(1);
  }
  return value;
}

export const config = {
  homeserverUrl: requireEnv("HOMESERVER_URL"),
  botAccessToken: requireEnv("BOT_ACCESS_TOKEN"),
  botUserId: requireEnv("BOT_USER_ID"),
  bridgeUrl: requireEnv("BRIDGE_URL"),

  bridgeApiKey: process.env.BRIDGE_API_KEY ?? "",
  testRoomId: process.env.TEST_ROOM_ID ?? "",
  platform: process.env.PLATFORM ?? "integration-test",
  externalRoomId: process.env.EXTERNAL_ROOM_ID ?? `test-room-${Date.now()}`,
  cryptoDir: process.env.CRYPTO_DIR ?? "./crypto-store",
  storageFile: process.env.STORAGE_FILE ?? "./bot-storage.json",
  webhookHost: process.env.WEBHOOK_HOST ?? "host.docker.internal",
  bridgeAsToken: process.env.BRIDGE_AS_TOKEN ?? "",
  bridgeDomain: process.env.BRIDGE_DOMAIN ?? "",
} as const;
