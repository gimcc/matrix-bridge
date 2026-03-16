/**
 * Matrix Bridge Webhook Receiver (Bun + TypeScript)
 *
 * Minimal webhook endpoint that receives messages from the Matrix bridge,
 * auto-registers itself on startup, and demonstrates sending messages back.
 *
 * Usage:
 *   bun run server.ts
 *   bun run server.ts --send-test    # also send a test message on startup
 *
 * Environment variables:
 *   BRIDGE_URL     - Bridge base URL   (default: http://matrix-bridge:29320)
 *   PLATFORM       - Platform ID       (default: webhook-demo)
 *   ROOM_ID        - External room ID  (default: webhook-general)
 *   PORT           - Listen port       (default: 5050)
 *   HOST           - Public URL bridge can reach (default: http://localhost:5050)
 *   API_KEY        - API key if configured (default: none)
 */

const BRIDGE_URL = process.env.BRIDGE_URL ?? "http://matrix-bridge:29320";
const PLATFORM = process.env.PLATFORM ?? "webhook-demo";
const ROOM_ID = process.env.ROOM_ID ?? "webhook-general";
const PORT = Number(process.env.PORT ?? 5050);
const HOST = process.env.HOST ?? `http://localhost:${PORT}`;
const API_KEY = process.env.API_KEY ?? "";
const SEND_TEST = process.argv.includes("--send-test");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface MessagePayload {
  event: "message";
  platform: string;
  source_platform?: string;
  message: {
    id: string;
    sender: {
      platform: string;
      external_id: string;
      display_name: string | null;
      avatar_url: string | null;
    };
    room: {
      platform: string;
      external_id: string;
      name: string | null;
    };
    content: {
      type: string;
      body?: string;
      url?: string;
      caption?: string;
      emoji?: string;
      target_id?: string;
    };
    timestamp: number;
    reply_to: string | null;
  };
}

interface CommandPayload {
  event: "command";
  platform: string;
  sender: string;
  command: string;
  room_id: string;
}

type WebhookPayload = MessagePayload | CommandPayload;

// ---------------------------------------------------------------------------
// Bridge API helpers
// ---------------------------------------------------------------------------

function apiHeaders(): Record<string, string> {
  const h: Record<string, string> = { "Content-Type": "application/json" };
  if (API_KEY) h["Authorization"] = `Bearer ${API_KEY}`;
  return h;
}

async function bridgePost(path: string, body: unknown): Promise<unknown> {
  const resp = await fetch(`${BRIDGE_URL}${path}`, {
    method: "POST",
    headers: apiHeaders(),
    body: JSON.stringify(body),
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`${path} ${resp.status}: ${text}`);
  }
  return resp.json();
}

// ---------------------------------------------------------------------------
// Setup: register webhook + room mapping
// ---------------------------------------------------------------------------

async function setup(): Promise<void> {
  console.log("[setup] Registering with bridge...");

  // Register webhook with capabilities
  try {
    await bridgePost("/api/v1/webhooks", {
      platform: PLATFORM,
      url: `${HOST}/webhook`,
      events: "message,redaction",
      forward_sources: ["matrix"],
      capabilities: ["message", "image", "file", "reaction", "edit", "redaction"],
      owner: "", // use auto_invite from config
    });
    console.log(`[setup] Webhook registered: ${HOST}/webhook`);
  } catch (e) {
    console.warn("[setup] Webhook:", (e as Error).message);
  }

  // Create room mapping
  try {
    const result = await bridgePost("/api/v1/rooms", {
      platform: PLATFORM,
      external_room_id: ROOM_ID,
      room_name: `Webhook Demo (${PLATFORM})`,
    });
    console.log("[setup] Room mapping:", result);
  } catch (e) {
    console.warn("[setup] Room:", (e as Error).message);
  }
}

// ---------------------------------------------------------------------------
// Webhook handler
// ---------------------------------------------------------------------------

function handleMessage(payload: MessagePayload): void {
  const msg = payload.message;
  const sender = msg.sender.display_name ?? msg.sender.external_id;
  const source = payload.source_platform ? `[${payload.source_platform}] ` : "";
  const room = msg.room.external_id;

  switch (msg.content.type) {
    case "text":
      console.log(`[${room}] ${source}${sender}: ${msg.content.body}`);
      break;
    case "image":
      console.log(`[${room}] ${source}${sender}: [image] ${msg.content.caption ?? msg.content.url}`);
      break;
    case "file":
      console.log(`[${room}] ${source}${sender}: [file] ${msg.content.body ?? msg.content.url}`);
      break;
    case "reaction":
      console.log(`[${room}] ${source}${sender}: reacted ${msg.content.emoji}`);
      break;
    case "redaction":
      console.log(`[${room}] ${source}${sender}: deleted ${msg.content.target_id}`);
      break;
    default:
      console.log(`[${room}] ${source}${sender}: [${msg.content.type}]`, msg.content);
  }
}

function handleCommand(payload: CommandPayload): void {
  console.log(`[command] ${payload.sender}: ${payload.command} (room: ${payload.room_id})`);
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

Bun.serve({
  port: PORT,
  async fetch(req) {
    const url = new URL(req.url);

    if (url.pathname === "/webhook" && req.method === "POST") {
      const payload = (await req.json()) as WebhookPayload;

      if (payload.event === "message") {
        handleMessage(payload as MessagePayload);
      } else if (payload.event === "command") {
        handleCommand(payload as CommandPayload);
      } else {
        console.log("[webhook] Unknown event:", payload);
      }

      return Response.json({ status: "ok" });
    }

    if (url.pathname === "/health" && req.method === "GET") {
      return Response.json({ status: "ok", platform: PLATFORM });
    }

    return new Response("Not Found", { status: 404 });
  },
});

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

console.log("=== Matrix Bridge Webhook Receiver ===\n");
console.log(`Bridge:   ${BRIDGE_URL}`);
console.log(`Platform: ${PLATFORM}`);
console.log(`Room:     ${ROOM_ID}`);
console.log(`Listen:   http://0.0.0.0:${PORT}`);
console.log(`Webhook:  ${HOST}/webhook\n`);

await setup();

if (SEND_TEST) {
  console.log("\n[test] Sending test message...");
  try {
    const result = await bridgePost("/api/v1/message", {
      platform: PLATFORM,
      room_id: ROOM_ID,
      sender: { id: "webhook-bot", display_name: "Webhook Demo Bot" },
      content: { type: "text", body: "Hello from webhook demo!" },
    });
    console.log("[test] Sent:", result);
  } catch (e) {
    console.error("[test] Failed:", (e as Error).message);
  }
}

console.log("\nListening for webhooks... Press Ctrl+C to stop.\n");
