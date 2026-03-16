/**
 * Test 4: Matrix → External (plaintext via webhook).
 *
 * Tests that when a real Matrix user sends a message in a bridged room,
 * the bridge delivers it to the registered webhook.
 *
 * Uses pushTransaction() to simulate the homeserver pushing events,
 * since we can't control Synapse's appservice push timing in tests.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeCreateRoomMapping,
  bridgeRegisterWebhook,
  bridgeDeleteRoomMapping,
  bridgeDeleteWebhook,
  startWebhookReceiver,
  pushTransaction,
  waitFor,
} from "./helpers";

const WEBHOOK_PORT = 19876;
let receiver: ReturnType<typeof startWebhookReceiver>;
let mappingId: number;
let webhookId: number;

const platform = "test_outbound";
const extRoomId = `ext_out_${Date.now()}`;
const matrixRoomId = `!outbound_test_${Date.now()}:${env.domain}`;

beforeAll(async () => {
  // Start webhook receiver.
  receiver = startWebhookReceiver(WEBHOOK_PORT);

  // Create room mapping.
  const mapping = await bridgeCreateRoomMapping(
    matrixRoomId,
    platform,
    extRoomId,
  );
  mappingId = mapping.id;

  // Register webhook.
  const wh = await bridgeRegisterWebhook(platform, receiver.url);
  webhookId = wh.id;
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Matrix → External (plaintext webhook)", () => {
  test("m.room.message is delivered to webhook", async () => {
    const msgBody = `Hello from Matrix! ${Date.now()}`;
    const eventId = `$test_event_${Date.now()}`;

    // Simulate homeserver pushing a transaction with a room message.
    await pushTransaction(`txn_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: `@real_user:${env.domain}`,
        event_id: eventId,
        origin_server_ts: Date.now(),
        content: {
          msgtype: "m.text",
          body: msgBody,
        },
      },
    ]);

    // Wait for webhook to receive the message.
    await waitFor(
      () =>
        receiver.messages.some((m: any) =>
          m?.message?.content?.body === msgBody,
        ),
      { label: "webhook receives message", timeout: 10_000 },
    );

    const received = receiver.messages.find(
      (m: any) => m?.message?.content?.body === msgBody,
    ) as any;

    expect(received).toBeTruthy();
    expect(received.event).toBe("message");
    expect(received.platform).toBe(platform);
    expect(received.message.sender.external_id).toContain("real_user");
  });

  test("duplicate transaction is deduplicated", async () => {
    const countBefore = receiver.messages.length;
    const txnId = `txn_dedup_${Date.now()}`;

    const events = [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: `@dedup_user:${env.domain}`,
        event_id: `$dedup_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body: "dedup test" },
      },
    ];

    await pushTransaction(txnId, events);
    await Bun.sleep(1000);
    await pushTransaction(txnId, events); // Same txn ID = should be skipped
    await Bun.sleep(1000);

    // Should only have received one new message.
    const newMessages = receiver.messages.length - countBefore;
    expect(newMessages).toBe(1);
  });

  test("bridge bot's own messages are ignored", async () => {
    const countBefore = receiver.messages.length;
    const botUserId = `@${env.bridgeBotLocalpart}:${env.domain}`;

    await pushTransaction(`txn_bot_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: botUserId,
        event_id: `$bot_msg_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body: "I am the bridge bot" },
      },
    ]);

    await Bun.sleep(1000);
    expect(receiver.messages.length).toBe(countBefore);
  });
});
