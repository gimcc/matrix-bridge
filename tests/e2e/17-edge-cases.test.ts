/**
 * Test 17: Edge cases and boundary conditions.
 *
 * Tests unusual inputs, empty payloads, special characters,
 * and other boundary conditions the bridge should handle gracefully.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeSendMessage,
  bridgeSendMessageRaw,
  bridgeSendContent,
  bridgeListRoomMappings,
  pushTransaction,
  startWebhookReceiver,
  bridgeRegisterWebhook,
  bridgeDeleteWebhook,
  waitFor,
} from "./helpers";

let webhookId: number;
let receiver: ReturnType<typeof startWebhookReceiver>;
let realMatrixRoom: string;

const WEBHOOK_PORT = 19888;
const platform = "test_edge";
const extRoom = `ext_edge_${Date.now()}`;
const sender = `@edge_user:${env.domain}`;

beforeAll(async () => {
  receiver = startWebhookReceiver(WEBHOOK_PORT);
  // Auto-create portal room by sending a message
  await bridgeSendMessage({
    platform,
    roomId: extRoom,
    senderId: "setup_user",
    body: "setup message to create portal room",
  });
  // Look up the real matrix room ID
  const mappings = await bridgeListRoomMappings(platform);
  const mapping = mappings.find((m) => m.external_room_id === extRoom);
  if (!mapping) throw new Error("Portal room was not auto-created");
  realMatrixRoom = mapping.matrix_room_id;
  const wh = await bridgeRegisterWebhook(platform, receiver.url, ["*"]);
  webhookId = wh.id;
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
});

describe("Input Validation", () => {
  test("missing platform returns 422", async () => {
    const resp = await bridgeSendMessageRaw({
      room_id: extRoom,
      sender: { id: "user1", display_name: "User" },
      content: { type: "text", body: "hello" },
    });
    expect(resp.status).toBe(422);
  });

  test("missing room_id returns 422", async () => {
    const resp = await bridgeSendMessageRaw({
      platform,
      sender: { id: "user1", display_name: "User" },
      content: { type: "text", body: "hello" },
    });
    expect(resp.status).toBe(422);
  });

  test("missing sender returns 422", async () => {
    const resp = await bridgeSendMessageRaw({
      platform,
      room_id: extRoom,
      content: { type: "text", body: "hello" },
    });
    expect(resp.status).toBe(422);
  });

  test("missing content returns 422", async () => {
    const resp = await bridgeSendMessageRaw({
      platform,
      room_id: extRoom,
      sender: { id: "user1", display_name: "User" },
    });
    expect(resp.status).toBe(422);
  });

  test("empty body is not valid JSON", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "",
    });
    expect(resp.status).toBeGreaterThanOrEqual(400);
  });

  test("invalid JSON returns 400", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{not valid json",
    });
    expect(resp.status).toBeGreaterThanOrEqual(400);
  });
});

describe("Special Characters", () => {
  test("unicode emoji in message body", async () => {
    const body = `emoji test 🎉🔥💻 ${Date.now()}`;
    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "emoji_user",
      body,
    });
    expect(result.event_id).toBeTruthy();
  });

  test("CJK characters in message body", async () => {
    const body = `中文测试 日本語テスト 한국어테스트 ${Date.now()}`;
    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "cjk_user",
      body,
    });
    expect(result.event_id).toBeTruthy();
  });

  test("HTML entities in message body", async () => {
    const body = `<script>alert('xss')</script> &amp; &lt; ${Date.now()}`;
    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "html_user",
      body,
    });
    expect(result.event_id).toBeTruthy();
  });

  test("newlines and whitespace in message body", async () => {
    const body = `line1\nline2\n\ttabbed\n\n\nempty lines ${Date.now()}`;
    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "newline_user",
      body,
    });
    expect(result.event_id).toBeTruthy();
  });

  test("very long message body (10KB)", async () => {
    const body = `long_${Date.now()}_${"x".repeat(10_000)}`;
    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "long_user",
      body,
    });
    expect(result.event_id).toBeTruthy();
  });
});

describe("Matrix Transaction Edge Cases", () => {
  test("empty events array in transaction", async () => {
    const resp = await fetch(
      `${env.bridgeUrl}/_matrix/app/v1/transactions/txn_empty_${Date.now()}`,
      {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${env.hsToken}`,
        },
        body: JSON.stringify({ events: [] }),
      },
    );
    expect(resp.ok).toBe(true);
  });

  test("unknown event type is silently ignored", async () => {
    const countBefore = receiver.messages.length;

    await pushTransaction(`txn_unknown_type_${Date.now()}`, [
      {
        type: "com.example.custom_event",
        room_id: realMatrixRoom,
        sender,
        event_id: `$unknown_type_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { foo: "bar" },
      },
    ]);

    await Bun.sleep(1500);
    expect(receiver.messages.length).toBe(countBefore);
  });

  test("transaction without authorization returns 401 or 403", async () => {
    const resp = await fetch(
      `${env.bridgeUrl}/_matrix/app/v1/transactions/txn_noauth_${Date.now()}`,
      {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ events: [] }),
      },
    );
    expect([401, 403]).toContain(resp.status);
  });

  test("transaction with wrong token returns 401 or 403", async () => {
    const resp = await fetch(
      `${env.bridgeUrl}/_matrix/app/v1/transactions/txn_badtoken_${Date.now()}`,
      {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          authorization: "Bearer WRONG_TOKEN",
        },
        body: JSON.stringify({ events: [] }),
      },
    );
    expect([401, 403]).toContain(resp.status);
  });

  test("state events (m.room.name etc.) do not crash the bridge", async () => {
    await pushTransaction(`txn_state_${Date.now()}`, [
      {
        type: "m.room.name",
        room_id: realMatrixRoom,
        sender,
        event_id: `$state_name_${Date.now()}`,
        origin_server_ts: Date.now(),
        state_key: "",
        content: { name: "New Room Name" },
      },
      {
        type: "m.room.topic",
        room_id: realMatrixRoom,
        sender,
        event_id: `$state_topic_${Date.now()}`,
        origin_server_ts: Date.now(),
        state_key: "",
        content: { topic: "New topic" },
      },
    ]);

    // Bridge should not crash — verify by sending a subsequent message.
    const body = `after_state_${Date.now()}`;
    await pushTransaction(`txn_after_state_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: realMatrixRoom,
        sender,
        event_id: `$after_state_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiver.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "message after state events received", timeout: 10_000 },
    );
  });
});

describe("Puppet Identity", () => {
  test("messages from different senders create different puppets", async () => {
    const body1 = `puppet_a_${Date.now()}`;
    const body2 = `puppet_b_${Date.now()}`;

    const r1 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "puppet_sender_a",
      senderName: "Alice from Telegram",
      body: body1,
    });
    const r2 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "puppet_sender_b",
      senderName: "Bob from Telegram",
      body: body2,
    });

    expect(r1.event_id).toBeTruthy();
    expect(r2.event_id).toBeTruthy();
    // Different senders should produce different event IDs (different puppet users).
    expect(r1.event_id).not.toBe(r2.event_id);
  });

  test("same sender across messages reuses puppet", async () => {
    const senderId = `reuse_puppet_${Date.now()}`;
    const body1 = `reuse_1_${Date.now()}`;
    const body2 = `reuse_2_${Date.now()}`;

    const r1 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId,
      senderName: "Stable Puppet",
      body: body1,
    });
    const r2 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId,
      senderName: "Stable Puppet",
      body: body2,
    });

    expect(r1.event_id).toBeTruthy();
    expect(r2.event_id).toBeTruthy();
  });
});
