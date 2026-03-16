/**
 * Matrix Bridge WebSocket Client (Bun + TypeScript)
 *
 * Connects to the bridge via WebSocket, receives real-time messages from
 * bridged Matrix rooms, and demonstrates sending commands back.
 *
 * Usage:
 *   bun run client.ts
 *
 * Environment variables:
 *   BRIDGE_URL  - Bridge base URL       (default: http://matrix-bridge:29320)
 *   PLATFORM    - Platform to subscribe (default: ws-demo)
 *   API_KEY     - API key if configured (default: none)
 *   ROOM_ID     - External room ID      (default: ws-general)
 */

const BRIDGE_URL = process.env.BRIDGE_URL ?? "http://matrix-bridge:29320";
const PLATFORM = process.env.PLATFORM ?? "ws-demo";
const API_KEY = process.env.API_KEY ?? "";
const ROOM_ID = process.env.ROOM_ID ?? "ws-general";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface WebhookPayload {
  event: "message" | "command";
  platform: string;
  // message event
  message?: {
    id: string;
    sender: { platform: string; external_id: string; display_name: string | null };
    room: { platform: string; external_id: string };
    content: { type: string; body?: string; emoji?: string; target_id?: string };
    timestamp: number;
    reply_to: string | null;
  };
  source_platform?: string;
  // command event
  sender?: string;
  command?: string;
  room_id?: string;
}

// ---------------------------------------------------------------------------
// Setup: register room mapping
// ---------------------------------------------------------------------------

async function setup(): Promise<void> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (API_KEY) headers["Authorization"] = `Bearer ${API_KEY}`;

  // Create room mapping (idempotent)
  try {
    const resp = await fetch(`${BRIDGE_URL}/api/v1/rooms`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        platform: PLATFORM,
        external_room_id: ROOM_ID,
        room_name: `WS Demo (${PLATFORM})`,
      }),
    });
    const data = await resp.json();
    console.log("[setup] Room mapping:", data);
  } catch (e) {
    console.warn("[setup] Room mapping failed:", (e as Error).message);
  }
}

// ---------------------------------------------------------------------------
// WebSocket connection with auto-reconnect
// ---------------------------------------------------------------------------

function connect(): void {
  const wsUrl = BRIDGE_URL.replace(/^http/, "ws");
  const capabilities = "message,image,file,reaction,edit,redaction,command";
  const url = `${wsUrl}/api/v1/ws?platform=${PLATFORM}&forward_sources=*&capabilities=${capabilities}`;
  console.log(`[ws] Connecting to ${url}`);

  const ws = new WebSocket(url);

  ws.onopen = () => {
    console.log("[ws] Connected");

    // If API key is configured, send auth as first frame
    if (API_KEY) {
      ws.send(JSON.stringify({ access_token: API_KEY }));
      console.log("[ws] Auth sent");
    }
  };

  ws.onmessage = (event) => {
    const payload: WebhookPayload = JSON.parse(String(event.data));

    if (payload.event === "message" && payload.message) {
      const msg = payload.message;
      const sender = msg.sender.display_name ?? msg.sender.external_id;
      const source = payload.source_platform ? ` [via ${payload.source_platform}]` : "";
      const contentType = msg.content.type;

      switch (contentType) {
        case "text":
          console.log(`[msg]${source} ${sender}: ${msg.content.body}`);
          break;
        case "image":
        case "file":
        case "video":
        case "audio":
          console.log(`[msg]${source} ${sender}: [${contentType}] ${msg.content.body ?? "(no caption)"}`);
          break;
        case "reaction":
          console.log(`[msg]${source} ${sender}: reacted ${msg.content.emoji} to ${msg.content.target_id}`);
          break;
        case "edit":
          console.log(`[msg]${source} ${sender}: edited ${msg.content.target_id}`);
          break;
        case "redaction":
          console.log(`[msg]${source} ${sender}: deleted ${msg.content.target_id}`);
          break;
        default:
          console.log(`[msg]${source} ${sender}: [${contentType}]`, msg.content);
      }
    } else if (payload.event === "command") {
      console.log(`[cmd] ${payload.sender}: ${payload.command} (room: ${payload.room_id})`);

      // Example: respond to commands
      handleCommand(payload.sender!, payload.command!, payload.room_id!);
    }
  };

  ws.onclose = (event) => {
    console.log(`[ws] Disconnected (code: ${event.code}, reason: ${event.reason})`);
    // Auto-reconnect after 5 seconds
    setTimeout(connect, 5000);
  };

  ws.onerror = (event) => {
    console.error("[ws] Error:", event);
  };
}

// ---------------------------------------------------------------------------
// Command handler — respond to !ws-demo <command> from Matrix DMs
// ---------------------------------------------------------------------------

async function handleCommand(sender: string, command: string, roomId: string): Promise<void> {
  console.log(`[cmd] Processing: "${command}" from ${sender}`);

  // Example: echo back via bridge API
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (API_KEY) headers["Authorization"] = `Bearer ${API_KEY}`;

  // For now just log — in a real integration you'd process the command
  // and potentially send a response back to the room
  if (command.startsWith("/echo ")) {
    const text = command.slice(6);
    try {
      const resp = await fetch(`${BRIDGE_URL}/api/v1/message`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          platform: PLATFORM,
          room_id: ROOM_ID,
          sender: { id: "bot", display_name: `${PLATFORM} Bot` },
          content: { type: "text", body: `Echo: ${text}` },
        }),
      });
      const data = await resp.json();
      console.log(`[cmd] Echo sent:`, data);
    } catch (e) {
      console.error("[cmd] Echo failed:", (e as Error).message);
    }
  }
}

// ---------------------------------------------------------------------------
// Send a test message (demonstrates inbound bridging)
// ---------------------------------------------------------------------------

async function sendTestMessage(): Promise<void> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (API_KEY) headers["Authorization"] = `Bearer ${API_KEY}`;

  try {
    const resp = await fetch(`${BRIDGE_URL}/api/v1/message`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        platform: PLATFORM,
        room_id: ROOM_ID,
        sender: {
          id: "demo-user",
          display_name: "WS Demo User",
        },
        content: {
          type: "text",
          body: `Hello from ${PLATFORM} WebSocket client!`,
        },
      }),
    });
    const data = await resp.json();
    console.log("[test] Message sent:", data);
  } catch (e) {
    console.error("[test] Send failed:", (e as Error).message);
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

console.log("=== Matrix Bridge WebSocket Client ===\n");
console.log(`Bridge:   ${BRIDGE_URL}`);
console.log(`Platform: ${PLATFORM}`);
console.log(`Room:     ${ROOM_ID}`);
console.log(`API Key:  ${API_KEY ? "(configured)" : "(none)"}\n`);

await setup();
connect();

// Send a test message after 3 seconds
setTimeout(sendTestMessage, 3000);

// Keep alive
console.log("\nRunning... Press Ctrl+C to stop.\n");
process.on("SIGINT", () => {
  console.log("\nShutting down.");
  process.exit(0);
});
