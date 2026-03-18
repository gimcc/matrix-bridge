/**
 * Test helpers: user registration, room creation, HTTP utilities.
 */
import { env } from "./env";

// ---------- HTTP helpers ----------

async function jsonFetch(
  url: string,
  opts: RequestInit & { json?: unknown } = {},
): Promise<Response> {
  const headers: Record<string, string> = {
    ...(opts.headers as Record<string, string>),
  };
  if (opts.json !== undefined) {
    headers["content-type"] = "application/json";
    opts.body = JSON.stringify(opts.json);
  }
  return fetch(url, { ...opts, headers });
}

// ---------- Synapse admin API ----------

/** Register a user via Synapse's admin registration endpoint (shared secret not needed if admin exists). */
export async function registerUser(
  localpart: string,
  password: string,
  admin = false,
): Promise<{ user_id: string; access_token: string }> {
  // Use the client register endpoint with admin-created user.
  // First, login as admin to get a token.
  const adminToken = await loginUser(env.adminUser, env.adminPassword);

  // Create user via admin API.
  const userId = `@${localpart}:${env.domain}`;
  const resp = await jsonFetch(
    `${env.homeserverUrl}/_synapse/admin/v2/users/${encodeURIComponent(userId)}`,
    {
      method: "PUT",
      headers: { authorization: `Bearer ${adminToken}` },
      json: {
        password,
        admin,
        displayname: localpart,
      },
    },
  );

  if (!resp.ok && resp.status !== 400) {
    throw new Error(
      `registerUser ${localpart} failed: ${resp.status} ${await resp.text()}`,
    );
  }

  // Now login as the new user to get their token.
  const token = await loginUser(localpart, password);
  return { user_id: userId, access_token: token };
}

export async function loginUser(
  localpart: string,
  password: string,
): Promise<string> {
  const resp = await jsonFetch(`${env.homeserverUrl}/_matrix/client/v3/login`, {
    method: "POST",
    json: {
      type: "m.login.password",
      identifier: { type: "m.id.user", user: localpart },
      password,
    },
  });
  if (!resp.ok)
    throw new Error(`login failed: ${resp.status} ${await resp.text()}`);
  const data = (await resp.json()) as { access_token: string };
  return data.access_token;
}

// ---------- Matrix client helpers ----------

export async function createRoom(
  accessToken: string,
  opts: {
    name?: string;
    invite?: string[];
    encrypted?: boolean;
  } = {},
): Promise<string> {
  const initialState: Array<{ type: string; content: unknown }> = [];
  if (opts.encrypted) {
    initialState.push({
      type: "m.room.encryption",
      content: { algorithm: "m.megolm.v1.aes-sha2" },
    });
  }

  const resp = await jsonFetch(
    `${env.homeserverUrl}/_matrix/client/v3/createRoom`,
    {
      method: "POST",
      headers: { authorization: `Bearer ${accessToken}` },
      json: {
        name: opts.name ?? "E2E Test Room",
        preset: "private_chat",
        invite: opts.invite ?? [],
        initial_state: initialState,
      },
    },
  );
  if (!resp.ok)
    throw new Error(`createRoom failed: ${resp.status} ${await resp.text()}`);
  const data = (await resp.json()) as { room_id: string };
  return data.room_id;
}

export async function inviteUser(
  accessToken: string,
  roomId: string,
  userId: string,
): Promise<void> {
  const resp = await jsonFetch(
    `${env.homeserverUrl}/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/invite`,
    {
      method: "POST",
      headers: { authorization: `Bearer ${accessToken}` },
      json: { user_id: userId },
    },
  );
  // 403 = already in room, which is fine
  if (!resp.ok && resp.status !== 403) {
    throw new Error(
      `inviteUser failed: ${resp.status} ${await resp.text()}`,
    );
  }
}

export async function joinRoom(
  accessToken: string,
  roomId: string,
): Promise<void> {
  const resp = await jsonFetch(
    `${env.homeserverUrl}/_matrix/client/v3/join/${encodeURIComponent(roomId)}`,
    {
      method: "POST",
      headers: { authorization: `Bearer ${accessToken}` },
      json: {},
    },
  );
  if (!resp.ok)
    throw new Error(`joinRoom failed: ${resp.status} ${await resp.text()}`);
}

export async function sendMessage(
  accessToken: string,
  roomId: string,
  body: string,
  txnId?: string,
): Promise<string> {
  const txn = txnId ?? crypto.randomUUID();
  const resp = await jsonFetch(
    `${env.homeserverUrl}/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/send/m.room.message/${encodeURIComponent(txn)}`,
    {
      method: "PUT",
      headers: { authorization: `Bearer ${accessToken}` },
      json: { msgtype: "m.text", body },
    },
  );
  if (!resp.ok)
    throw new Error(`sendMessage failed: ${resp.status} ${await resp.text()}`);
  const data = (await resp.json()) as { event_id: string };
  return data.event_id;
}

export async function getRoomMessages(
  accessToken: string,
  roomId: string,
  limit = 20,
): Promise<Array<{ type: string; content: unknown; sender: string; event_id: string }>> {
  const resp = await jsonFetch(
    `${env.homeserverUrl}/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/messages?dir=b&limit=${limit}`,
    {
      headers: { authorization: `Bearer ${accessToken}` },
    },
  );
  if (!resp.ok)
    throw new Error(`getMessages failed: ${resp.status} ${await resp.text()}`);
  const data = (await resp.json()) as { chunk: Array<{ type: string; content: unknown; sender: string; event_id: string }> };
  return data.chunk;
}

// ---------- Bridge API helpers ----------

export async function bridgeCreateRoomMapping(
  matrixRoomId: string,
  platform: string,
  externalRoomId: string,
): Promise<{ id: number }> {
  const resp = await jsonFetch(`${env.bridgeUrl}/api/v1/rooms`, {
    method: "POST",
    json: {
      matrix_room_id: matrixRoomId,
      platform,
      external_room_id: externalRoomId,
    },
  });
  if (!resp.ok)
    throw new Error(
      `bridgeCreateRoomMapping failed: ${resp.status} ${await resp.text()}`,
    );
  return (await resp.json()) as { id: number };
}

export async function bridgeSendMessage(opts: {
  platform: string;
  roomId: string;
  senderId: string;
  senderName?: string;
  body: string;
  messageId?: string;
}): Promise<{ event_id: string; message_id: string }> {
  const resp = await jsonFetch(`${env.bridgeUrl}/api/v1/message`, {
    method: "POST",
    json: {
      platform: opts.platform,
      room_id: opts.roomId,
      sender: {
        id: opts.senderId,
        display_name: opts.senderName ?? opts.senderId,
      },
      content: { type: "text", body: opts.body },
      external_message_id: opts.messageId,
    },
  });
  const data = await resp.json();
  if (!resp.ok) throw new Error(`bridgeSendMessage failed: ${JSON.stringify(data)}`);
  return data as { event_id: string; message_id: string };
}

export async function bridgeRegisterWebhook(
  platform: string,
  url: string,
  forwardSources: string[] = [],
): Promise<{ id: number }> {
  const resp = await jsonFetch(`${env.bridgeUrl}/api/v1/webhooks`, {
    method: "POST",
    json: { platform, url, forward_sources: forwardSources },
  });
  if (!resp.ok)
    throw new Error(
      `bridgeRegisterWebhook failed: ${resp.status} ${await resp.text()}`,
    );
  return (await resp.json()) as { id: number };
}

export async function bridgeDeleteWebhook(id: number): Promise<void> {
  await jsonFetch(`${env.bridgeUrl}/api/v1/webhooks/${id}`, {
    method: "DELETE",
  });
}

export async function bridgeDeleteRoomMapping(id: number): Promise<void> {
  await jsonFetch(`${env.bridgeUrl}/api/v1/rooms/${id}`, {
    method: "DELETE",
  });
}

export async function bridgeHealth(): Promise<boolean> {
  try {
    const resp = await fetch(`${env.bridgeUrl}/health`);
    return resp.ok;
  } catch {
    return false;
  }
}

// ---------- Webhook receiver ----------

/** Start a simple HTTP server that collects webhook payloads. */
export function startWebhookReceiver(port: number): {
  messages: Array<unknown>;
  stop: () => void;
  url: string;
} {
  const messages: Array<unknown> = [];

  const server = Bun.serve({
    port,
    fetch: async (req) => {
      if (req.method === "POST") {
        const body = await req.json();
        messages.push(body);
        return new Response("ok", { status: 200 });
      }
      return new Response("not found", { status: 404 });
    },
  });

  return {
    messages,
    stop: () => server.stop(),
    url: `http://${process.env.WEBHOOK_HOST ?? "localhost"}:${port}`,
  };
}

// ---------- Polling / waiting ----------

/** Poll until a condition is met or timeout. */
export async function waitFor(
  fn: () => Promise<boolean> | boolean,
  opts: { timeout?: number; interval?: number; label?: string } = {},
): Promise<void> {
  const timeout = opts.timeout ?? 15_000;
  const interval = opts.interval ?? 500;
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await fn()) return;
    await Bun.sleep(interval);
  }
  throw new Error(`waitFor timed out after ${timeout}ms: ${opts.label ?? "condition not met"}`);
}

// ---------- Transaction simulation ----------

/**
 * Simulate a homeserver pushing an appservice transaction.
 * Useful for testing the Matrix → External direction without a real Synapse.
 */
export async function pushTransaction(
  txnId: string,
  events: unknown[],
): Promise<void> {
  const resp = await jsonFetch(
    `${env.bridgeUrl}/_matrix/app/v1/transactions/${encodeURIComponent(txnId)}`,
    {
      method: "PUT",
      headers: { authorization: `Bearer ${env.hsToken}` },
      json: { events },
    },
  );
  if (!resp.ok)
    throw new Error(
      `pushTransaction failed: ${resp.status} ${await resp.text()}`,
    );
}

// ---------- Bridge raw API ----------

/** Send a raw bridge message request and return the full response (no throw). */
export async function bridgeSendMessageRaw(body: unknown): Promise<Response> {
  return fetch(`${env.bridgeUrl}/api/v1/message`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** List room mappings for a platform. */
export async function bridgeListRoomMappings(
  platform: string,
): Promise<Array<{ id: number; matrix_room_id: string; platform_id: string; external_room_id: string }>> {
  const resp = await fetch(
    `${env.bridgeUrl}/api/v1/rooms?platform=${encodeURIComponent(platform)}`,
  );
  if (!resp.ok)
    throw new Error(`listRoomMappings failed: ${resp.status} ${await resp.text()}`);
  const data = (await resp.json()) as { rooms: Array<{ id: number; matrix_room_id: string; platform_id: string; external_room_id: string }> };
  return data.rooms;
}

/** Upload media via bridge API. Returns mxc:// URI. */
export async function bridgeUploadMedia(
  data: Uint8Array,
  filename: string,
  contentType: string,
): Promise<{ content_uri: string; filename: string; size: number }> {
  const form = new FormData();
  form.append("file", new Blob([data], { type: contentType }), filename);
  const resp = await fetch(`${env.bridgeUrl}/api/v1/upload`, {
    method: "POST",
    body: form,
  });
  if (!resp.ok)
    throw new Error(`bridgeUploadMedia failed: ${resp.status} ${await resp.text()}`);
  return (await resp.json()) as { content_uri: string; filename: string; size: number };
}

/** Send a bridge message with arbitrary content payload (returns raw Response). */
export async function bridgeSendContent(opts: {
  platform: string;
  roomId: string;
  senderId: string;
  senderName?: string;
  content: unknown;
  messageId?: string;
}): Promise<Response> {
  return fetch(`${env.bridgeUrl}/api/v1/message`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      platform: opts.platform,
      room_id: opts.roomId,
      sender: {
        id: opts.senderId,
        display_name: opts.senderName ?? opts.senderId,
      },
      content: opts.content,
      external_message_id: opts.messageId,
    }),
  });
}
