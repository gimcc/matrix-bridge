/**
 * Matrix Bridge Webhook Integration Demo (Node.js / Express)
 *
 * This demo shows how an external platform integrates with the Matrix Bridge
 * via HTTP webhooks. It covers two directions:
 *
 *   Outbound (Matrix -> External):
 *     The bridge POSTs events to our /webhook endpoint whenever something
 *     happens in a bridged Matrix room.
 *
 *   Inbound (External -> Matrix):
 *     We call the bridge's REST API to send messages, upload media, and
 *     manage room mappings.
 *
 * Requirements: Node.js 18+, express
 *     npm install
 *
 * Environment variables:
 *     BRIDGE_URL   - Base URL of the bridge API (default: http://localhost:29320)
 *     PLATFORM     - Platform identifier registered with the bridge (default: myapp)
 *     ROOM_ID      - External room ID to bridge (default: general)
 *     WEBHOOK_PORT - Port this demo listens on (default: 5050)
 *     WEBHOOK_HOST - Host for the webhook callback URL (default: http://localhost:5050)
 */

import express from "express";
import { readFileSync } from "node:fs";
import { basename } from "node:path";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const BRIDGE_URL = process.env.BRIDGE_URL ?? "http://localhost:29320";
const PLATFORM = process.env.PLATFORM ?? "myapp";
const ROOM_ID = process.env.ROOM_ID ?? "general";
const WEBHOOK_PORT = Number(process.env.WEBHOOK_PORT ?? 5050);
const WEBHOOK_HOST =
  process.env.WEBHOOK_HOST ?? `http://localhost:${WEBHOOK_PORT}`;

const app = express();
app.use(express.json());

// ---------------------------------------------------------------------------
// Outbound: receive webhook callbacks from the bridge
// ---------------------------------------------------------------------------

/**
 * Receive an event pushed by the bridge (Matrix -> External).
 *
 * The bridge sends a JSON payload for every message in the bridged room.
 * Cross-platform forwarded messages include a `source_platform` field at the
 * top level, indicating which platform the message originally came from.
 *
 * Payload example (see README for full schema):
 * {
 *   "event": "message",
 *   "platform": "myapp",
 *   "source_platform": "telegram",       // optional
 *   "message": { ... }
 * }
 */
app.post("/webhook", (req, res) => {
  const payload = req.body ?? {};

  const eventType = payload.event ?? "unknown";
  const sourcePlatform = payload.source_platform; // may be undefined
  const message = payload.message ?? {};

  const sender = message.sender ?? {};
  const content = message.content ?? {};

  const displayName = sender.display_name ?? "Unknown";
  const body = content.body ?? "";
  const contentType = content.type ?? "text";

  // Distinguish cross-platform forwarded messages from native Matrix ones.
  if (sourcePlatform) {
    console.log(
      `[cross-platform] ${displayName} (via ${sourcePlatform}): [${contentType}] ${body}`
    );
  } else {
    console.log(`[${eventType}] ${displayName}: [${contentType}] ${body}`);
  }

  // Respond with 200 so the bridge knows we processed the event.
  res.json({ status: "ok" });
});

// ---------------------------------------------------------------------------
// Inbound helpers: send data TO the bridge (External -> Matrix)
// ---------------------------------------------------------------------------

/**
 * Send a plain-text message to Matrix through the bridge.
 *
 * POST /api/v1/message
 *
 * @param {string} text         - Message body
 * @param {string} [senderId]   - Sender identifier (default: "bot")
 * @param {string} [senderName] - Display name (default: "My Bot")
 * @returns {Promise<object>}   - Bridge response
 */
async function sendTextMessage(
  text,
  senderId = "bot",
  senderName = "My Bot"
) {
  const payload = {
    platform: PLATFORM,
    room_id: ROOM_ID,
    sender: {
      id: senderId,
      display_name: senderName,
    },
    content: {
      type: "text",
      body: text,
    },
  };

  const resp = await fetch(`${BRIDGE_URL}/api/v1/message`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });

  if (!resp.ok) {
    throw new Error(`sendTextMessage failed: ${resp.status} ${resp.statusText}`);
  }

  const result = await resp.json();
  console.log("Sent text message:", result);
  return result;
}

/**
 * Upload a file to the bridge and return media metadata.
 *
 * POST /api/v1/upload  (multipart/form-data)
 *
 * Returns an object that typically includes an `mxc_url` (or similar) which
 * you then reference in a subsequent message.
 *
 * @param {string} filePath - Path to the file on disk
 * @returns {Promise<object>}
 */
async function uploadMedia(filePath) {
  const fileBuffer = readFileSync(filePath);
  const fileName = basename(filePath);

  // Build a multipart/form-data body using the built-in FormData (Node 18+).
  const form = new FormData();
  form.append("file", new Blob([fileBuffer]), fileName);

  const resp = await fetch(`${BRIDGE_URL}/api/v1/upload`, {
    method: "POST",
    body: form,
  });

  if (!resp.ok) {
    throw new Error(`uploadMedia failed: ${resp.status} ${resp.statusText}`);
  }

  const result = await resp.json();
  console.log("Uploaded media:", result);
  return result;
}

/**
 * Upload an image then send it as an image message to Matrix.
 *
 * Two-step process:
 *   1. Upload the file via /api/v1/upload to obtain an mxc:// URI.
 *   2. Send a message with content type "image" referencing that URI.
 *
 * @param {string} filePath     - Path to the image on disk
 * @param {string} [senderId]   - Sender identifier
 * @param {string} [senderName] - Display name
 * @returns {Promise<object>}
 */
async function sendImageMessage(
  filePath,
  senderId = "bot",
  senderName = "My Bot"
) {
  // Step 1: upload
  const media = await uploadMedia(filePath);
  const mxcUrl = media.mxc_url ?? media.url ?? "";

  // Step 2: send image message
  const payload = {
    platform: PLATFORM,
    room_id: ROOM_ID,
    sender: {
      id: senderId,
      display_name: senderName,
    },
    content: {
      type: "image",
      url: mxcUrl,
      body: basename(filePath),
    },
  };

  const resp = await fetch(`${BRIDGE_URL}/api/v1/message`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });

  if (!resp.ok) {
    throw new Error(
      `sendImageMessage failed: ${resp.status} ${resp.statusText}`
    );
  }

  const result = await resp.json();
  console.log("Sent image message:", result);
  return result;
}

// ---------------------------------------------------------------------------
// Setup: register webhook + room mapping on startup
// ---------------------------------------------------------------------------

/**
 * Register this server as a webhook receiver with the bridge.
 *
 * POST /api/v1/webhooks
 */
async function registerWebhook() {
  const payload = {
    platform: PLATFORM,
    url: `${WEBHOOK_HOST}/webhook`,
  };

  const resp = await fetch(`${BRIDGE_URL}/api/v1/webhooks`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });

  if (!resp.ok) {
    throw new Error(
      `registerWebhook failed: ${resp.status} ${resp.statusText}`
    );
  }

  const result = await resp.json();
  console.log("Registered webhook:", result);
  return result;
}

/**
 * Create a mapping between an external room and a Matrix room.
 *
 * POST /api/v1/rooms
 */
async function createRoomMapping() {
  const payload = {
    platform: PLATFORM,
    external_id: ROOM_ID,
  };

  const resp = await fetch(`${BRIDGE_URL}/api/v1/rooms`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });

  if (!resp.ok) {
    throw new Error(
      `createRoomMapping failed: ${resp.status} ${resp.statusText}`
    );
  }

  const result = await resp.json();
  console.log("Created room mapping:", result);
  return result;
}

/**
 * Run one-time setup: register webhook and room mapping.
 */
async function setup() {
  console.log("Setting up bridge integration...");
  console.log(`  Bridge URL : ${BRIDGE_URL}`);
  console.log(`  Platform   : ${PLATFORM}`);
  console.log(`  Room ID    : ${ROOM_ID}`);
  console.log(`  Webhook URL: ${WEBHOOK_HOST}/webhook`);

  try {
    await registerWebhook();
  } catch (err) {
    console.warn(
      "Failed to register webhook (bridge may not be running):",
      err.message
    );
  }

  try {
    await createRoomMapping();
  } catch (err) {
    console.warn("Failed to create room mapping:", err.message);
  }

  console.log("Setup complete. Listening for webhook callbacks...");
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

app.listen(WEBHOOK_PORT, async () => {
  console.log(`Webhook server listening on port ${WEBHOOK_PORT}`);
  await setup();

  // Optionally send a test message on startup.
  if (process.argv.includes("--send-test")) {
    await sendTextMessage("Hello from the Node.js webhook demo!");
  }
});

// Export functions for external use / testing.
export {
  sendTextMessage,
  sendImageMessage,
  uploadMedia,
  registerWebhook,
  createRoomMapping,
};
