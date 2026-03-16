#!/usr/bin/env tsx
/**
 * Setup helper — verify environment and create a test user if needed.
 *
 * Usage:
 *   HOMESERVER_URL=... BOT_ACCESS_TOKEN=... BOT_USER_ID=... BRIDGE_URL=... \
 *     npx tsx src/setup.ts
 *
 * This script:
 * 1. Checks that all required environment variables are set.
 * 2. Verifies the homeserver is reachable.
 * 3. Verifies the bridge is reachable.
 * 4. Tests the bot access token is valid.
 * 5. Checks E2EE crypto store status.
 */

import { config } from "./config.js";

async function main(): Promise<void> {
  console.log("Matrix Bridge Integration Test — Setup Check\n");

  // 1. Environment.
  console.log("[env] Configuration:");
  console.log(`  HOMESERVER_URL   = ${config.homeserverUrl}`);
  console.log(`  BOT_USER_ID      = ${config.botUserId}`);
  console.log(`  BRIDGE_URL       = ${config.bridgeUrl}`);
  console.log(`  BRIDGE_API_KEY   = ${config.bridgeApiKey ? "(set)" : "(not set)"}`);
  console.log(`  TEST_ROOM_ID     = ${config.testRoomId || "(auto-create)"}`);
  console.log(`  PLATFORM         = ${config.platform}`);
  console.log(`  CRYPTO_DIR       = ${config.cryptoDir}`);
  console.log();

  // 2. Homeserver reachability.
  try {
    const resp = await fetch(`${config.homeserverUrl}/_matrix/client/versions`);
    const body = (await resp.json()) as { versions?: string[] };
    console.log(`[homeserver] reachable, versions: ${body.versions?.join(", ")}`);
  } catch (err) {
    console.error(`[homeserver] UNREACHABLE: ${err}`);
    process.exit(1);
  }

  // 3. Bridge reachability.
  try {
    const resp = await fetch(`${config.bridgeUrl}/health`);
    const body = (await resp.json()) as { status?: string };
    console.log(`[bridge] reachable, status: ${body.status}`);
  } catch (err) {
    console.error(`[bridge] UNREACHABLE: ${err}`);
    process.exit(1);
  }

  // 4. Access token validity.
  try {
    const resp = await fetch(`${config.homeserverUrl}/_matrix/client/v3/account/whoami`, {
      headers: { Authorization: `Bearer ${config.botAccessToken}` },
    });
    const body = (await resp.json()) as { user_id?: string; device_id?: string };
    console.log(`[auth] token valid: user=${body.user_id}, device=${body.device_id}`);
    if (body.user_id !== config.botUserId) {
      console.warn(`  WARNING: token user_id (${body.user_id}) does not match BOT_USER_ID (${config.botUserId})`);
    }
  } catch (err) {
    console.error(`[auth] token validation failed: ${err}`);
    process.exit(1);
  }

  // 5. Crypto store.
  const fs = await import("node:fs");
  if (fs.existsSync(config.cryptoDir)) {
    const files = fs.readdirSync(config.cryptoDir);
    console.log(`[crypto] store exists at ${config.cryptoDir} (${files.length} files)`);
  } else {
    console.log(`[crypto] no store at ${config.cryptoDir} (will be created on first run)`);
  }

  console.log("\nSetup check complete. Run 'npm test' to execute integration tests.");
}

main();
