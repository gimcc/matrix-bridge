/**
 * Matrix Bridge Chat Demo (Bun + TypeScript)
 *
 * A minimal chat UI that bridges to a Matrix room via the Matrix Bridge
 * HTTP API. Messages typed in the browser are sent to Matrix as a puppet
 * user; messages from Matrix users are pushed to the browser in real time
 * via Server-Sent Events (SSE).
 *
 * Requirements: Bun 1.1+
 *     bun run server.ts
 *
 * Environment variables:
 *     BRIDGE_URL     - Base URL of the bridge API (default: http://localhost:29320)
 *     PLATFORM       - Platform identifier (default: web)
 *     ROOM_ID        - External room ID to bridge (default: general)
 *     MATRIX_ROOM_ID - Matrix room ID (optional; bridge auto-creates if omitted)
 *     PORT           - Port this demo listens on (default: 3030)
 *     HOST           - Public URL the bridge can reach (default: http://localhost:3030)
 */

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const BRIDGE_URL = process.env.BRIDGE_URL ?? "http://localhost:29320";
const PLATFORM = process.env.PLATFORM ?? "web";
const ROOM_ID = process.env.ROOM_ID ?? "general";
const MATRIX_ROOM_ID = process.env.MATRIX_ROOM_ID; // optional: bridge auto-creates if omitted
const PORT = Number(process.env.PORT ?? 3030);
const HOST = process.env.HOST ?? `http://localhost:${PORT}`;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ChatMessage {
  sender: string;
  body: string;
  platform: string;
  timestamp: number;
}

// ---------------------------------------------------------------------------
// SSE: push messages to connected browsers
// ---------------------------------------------------------------------------

// Each SSE client is a Bun direct-stream controller.
interface SSEClient {
  ctrl: ReadableStreamDirectController;
  alive: boolean;
}

const sseClients = new Set<SSEClient>();

function broadcast(msg: ChatMessage): void {
  console.log(`[broadcast] pushing to ${sseClients.size} SSE client(s)`);
  const payload = `data: ${JSON.stringify(msg)}\n\n`;
  for (const client of sseClients) {
    if (!client.alive) continue;
    try {
      client.ctrl.write(payload);
      client.ctrl.flush();
    } catch {
      console.log("[broadcast] write failed, removing client");
      client.alive = false;
      sseClients.delete(client);
      try { client.ctrl.close(); } catch { /* already closed */ }
    }
  }
}

// ---------------------------------------------------------------------------
// Bridge API helpers
// ---------------------------------------------------------------------------

async function bridgePost(path: string, body: unknown): Promise<unknown> {
  const resp = await fetch(`${BRIDGE_URL}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`${path} failed: ${resp.status} ${text}`);
  }
  return resp.json();
}

async function setup(): Promise<void> {
  console.log("Setting up bridge integration...");
  console.log(`  Bridge URL : ${BRIDGE_URL}`);
  console.log(`  Platform   : ${PLATFORM}`);
  console.log(`  Room ID    : ${ROOM_ID}`);
  console.log(`  Matrix Room: ${MATRIX_ROOM_ID ?? "(auto-create)"}`);
  console.log(`  Webhook URL: ${HOST}/webhook`);

  try {
    await bridgePost("/api/v1/webhooks", {
      platform: PLATFORM,
      url: `${HOST}/webhook`,
    });
    console.log("  Webhook registered.");
  } catch (e: unknown) {
    console.warn("  Webhook registration failed:", (e as Error).message);
  }

  try {
    const roomPayload: Record<string, string> = {
      platform: PLATFORM,
      external_room_id: ROOM_ID,
    };
    if (MATRIX_ROOM_ID) {
      roomPayload.matrix_room_id = MATRIX_ROOM_ID;
    }
    const roomResult = await bridgePost("/api/v1/rooms", roomPayload);
    const assignedRoom = (roomResult as { matrix_room_id?: string }).matrix_room_id;
    console.log(`  Room mapping created. Matrix room: ${assignedRoom ?? MATRIX_ROOM_ID}`);
  } catch (e: unknown) {
    console.warn("  Room mapping failed:", (e as Error).message);
  }

  console.log(`\nChat UI: http://localhost:${PORT}`);
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

function handleSSE(req: Request): Response {
  const stream = new ReadableStream({
    type: "direct",
    pull(controller: ReadableStreamDirectController) {
      const client: SSEClient = { ctrl: controller, alive: true };
      sseClients.add(client);
      controller.write(": connected\n\n");
      controller.flush();
      console.log(`[sse] client connected (${sseClients.size} total)`);

      function cleanup() {
        if (!client.alive) return;
        client.alive = false;
        clearInterval(heartbeat);
        sseClients.delete(client);
        console.log(`[sse] client disconnected (${sseClients.size} total)`);
        try { controller.close(); } catch { /* already closed */ }
      }

      // Send a heartbeat comment every 10s to keep the connection alive
      // through proxies and prevent browser idle timeouts.
      const heartbeat = setInterval(() => {
        try {
          controller.write(": heartbeat\n\n");
          controller.flush();
          console.log(`[sse] heartbeat sent (${sseClients.size} client(s))`);
        } catch {
          cleanup();
        }
      }, 10_000);

      // Detect client disconnect via the request's AbortSignal.
      req.signal.addEventListener("abort", () => cleanup());

      // Keep the stream open until the client disconnects.
      return new Promise<void>((resolve) => {
        const check = setInterval(() => {
          if (!client.alive) {
            clearInterval(check);
            resolve();
          }
        }, 1000);
      });
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    },
  });
}

async function handleWebhook(req: Request): Promise<Response> {
  const payload = (await req.json()) as Record<string, unknown>;
  const message = (payload.message ?? {}) as Record<string, unknown>;
  const sender = (message.sender ?? {}) as Record<string, unknown>;
  const content = (message.content ?? {}) as Record<string, unknown>;
  const sourcePlatform = (payload.source_platform as string) ?? "";

  const displayName = (sender.display_name as string) ?? (sender.external_id as string) ?? "Unknown";
  const body = (content.body as string) ?? "";
  const platform = sourcePlatform || (sender.platform as string) || "matrix";

  const msg: ChatMessage = {
    sender: displayName,
    body,
    platform,
    timestamp: Date.now(),
  };

  const contentType = (content.type as string) ?? "text";
  console.log(`[webhook] [${platform}] ${displayName}: [${contentType}] ${body}`);
  broadcast(msg);
  return Response.json({ status: "ok" });
}

async function handleSend(req: Request): Promise<Response> {
  const { sender_id, sender_name, body } = (await req.json()) as {
    sender_id: string;
    sender_name: string;
    body: string;
  };

  console.log(`[send] ${sender_name} (${sender_id}): ${body}`);

  const result = await bridgePost("/api/v1/message", {
    platform: PLATFORM,
    room_id: ROOM_ID,
    sender: { id: sender_id, display_name: sender_name },
    content: { type: "text", body },
  });

  return Response.json(result);
}

// ---------------------------------------------------------------------------
// Inline HTML
// ---------------------------------------------------------------------------

const HTML = /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Matrix Bridge Chat</title>
<style>
  :root {
    --bg: #0d1117; --surface: #161b22; --border: #30363d;
    --text: #e6edf3; --muted: #8b949e; --accent: #58a6ff;
    --sent: #1f6feb; --received: #30363d;
  }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
    background: var(--bg); color: var(--text);
    height: 100dvh; display: flex; flex-direction: column;
  }
  header {
    padding: 12px 16px; background: var(--surface); border-bottom: 1px solid var(--border);
    display: flex; align-items: center; gap: 12px;
  }
  header h1 { font-size: 16px; font-weight: 600; }
  header .dot { width: 8px; height: 8px; border-radius: 50%; background: #3fb950; }
  header .room { color: var(--muted); font-size: 13px; margin-left: auto; }

  #messages {
    flex: 1; overflow-y: auto; padding: 16px; display: flex;
    flex-direction: column; gap: 8px;
  }
  .msg {
    max-width: 70%; padding: 8px 12px; border-radius: 12px;
    font-size: 14px; line-height: 1.5; word-wrap: break-word;
  }
  .msg.sent { align-self: flex-end; background: var(--sent); border-bottom-right-radius: 4px; }
  .msg.received { align-self: flex-start; background: var(--received); border-bottom-left-radius: 4px; }
  .msg .meta { font-size: 11px; color: var(--muted); margin-bottom: 2px; }
  .msg.sent .meta { color: rgba(255,255,255,.6); }

  #compose {
    padding: 12px 16px; background: var(--surface); border-top: 1px solid var(--border);
    display: flex; gap: 8px;
  }
  #name-input {
    width: 100px; padding: 8px 12px; border-radius: 8px; border: 1px solid var(--border);
    background: var(--bg); color: var(--text); font-size: 14px; flex-shrink: 0;
  }
  #msg-input {
    flex: 1; padding: 8px 12px; border-radius: 8px; border: 1px solid var(--border);
    background: var(--bg); color: var(--text); font-size: 14px; outline: none;
  }
  #msg-input:focus { border-color: var(--accent); }
  #send-btn {
    padding: 8px 20px; border-radius: 8px; border: none; cursor: pointer;
    background: var(--accent); color: #fff; font-size: 14px; font-weight: 600;
  }
  #send-btn:hover { opacity: .85; }
  #send-btn:disabled { opacity: .4; cursor: default; }
</style>
</head>
<body>
  <header>
    <span class="dot"></span>
    <h1>Matrix Bridge Chat</h1>
    <span class="room" id="room-label"></span>
  </header>

  <div id="messages"></div>

  <form id="compose">
    <input id="name-input" placeholder="Your name" value="Guest" autocomplete="off">
    <input id="msg-input" placeholder="Type a message..." autocomplete="off" autofocus>
    <button id="send-btn" type="submit">Send</button>
  </form>

<script>
const messagesEl = document.getElementById("messages");
const nameInput = document.getElementById("name-input");
const msgInput = document.getElementById("msg-input");
const sendBtn = document.getElementById("send-btn");
const roomLabel = document.getElementById("room-label");

roomLabel.textContent = location.hostname + ":" + location.port;

function myName() { return nameInput.value.trim() || "Guest"; }
function myId() { return myName().toLowerCase().replace(/[^a-z0-9._-]/g, "_"); }

function appendMessage(sender, body, platform, isSent) {
  const div = document.createElement("div");
  div.className = "msg " + (isSent ? "sent" : "received");
  const meta = document.createElement("div");
  meta.className = "meta";
  meta.textContent = isSent ? sender : sender + " (" + platform + ")";
  const text = document.createElement("div");
  text.textContent = body;
  div.appendChild(meta);
  div.appendChild(text);
  messagesEl.appendChild(div);
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

// SSE: real-time messages from Matrix (with reconnect backoff)
let sse;
function connectSSE() {
  sse = new EventSource("/events");
  sse.onmessage = (e) => {
    const msg = JSON.parse(e.data);
    if (msg.sender === myName() && msg.platform === "web") return;
    appendMessage(msg.sender, msg.body, msg.platform, false);
  };
  sse.onerror = () => {
    sse.close();
    setTimeout(connectSSE, 3000);
  };
}
connectSSE();

// Send message
document.getElementById("compose").addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = msgInput.value.trim();
  if (!body) return;

  sendBtn.disabled = true;
  msgInput.value = "";

  appendMessage(myName(), body, "web", true);

  try {
    await fetch("/send", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sender_id: myId(), sender_name: myName(), body }),
    });
  } catch (err) {
    appendMessage("System", "Failed to send: " + err.message, "error", false);
  }

  sendBtn.disabled = false;
  msgInput.focus();
});
</script>
</body>
</html>`;

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

Bun.serve({
  port: PORT,
  idleTimeout: 255, // max allowed; prevents Bun from killing SSE connections
  async fetch(req) {
    const url = new URL(req.url);

    if (url.pathname === "/" && req.method === "GET") {
      return new Response(HTML, {
        headers: { "Content-Type": "text/html; charset=utf-8" },
      });
    }
    if (url.pathname === "/events" && req.method === "GET") return handleSSE(req);
    if (url.pathname === "/webhook" && req.method === "POST") return handleWebhook(req);
    if (url.pathname === "/send" && req.method === "POST") return handleSend(req);

    return new Response("Not Found", { status: 404 });
  },
});

await setup();
