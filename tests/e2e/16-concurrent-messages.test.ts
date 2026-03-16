/**
 * Test 16: Concurrent message handling.
 *
 * Verifies the bridge correctly handles multiple messages sent in rapid
 * succession, maintaining order and avoiding data corruption.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeRegisterWebhook,
  bridgeDeleteWebhook,
  bridgeSendMessage,
  bridgeListRoomMappings,
  startWebhookReceiver,
  pushTransaction,
  waitFor,
} from "./helpers";

const WEBHOOK_PORT = 19887;
let receiver: ReturnType<typeof startWebhookReceiver>;
let webhookId: number;
let realMatrixRoom: string;

const platform = "test_concurrent";
const extRoom = `ext_concurrent_${Date.now()}`;
const sender = `@concurrent_user:${env.domain}`;

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

describe("Concurrent Messages", () => {
  test("rapid external→Matrix messages all succeed", async () => {
    const count = 10;
    const prefix = `rapid_ext_${Date.now()}`;

    // Send messages concurrently.
    const results = await Promise.all(
      Array.from({ length: count }, (_, i) =>
        bridgeSendMessage({
          platform,
          roomId: extRoom,
          senderId: `rapid_sender_${i}`,
          senderName: `Rapid User ${i}`,
          body: `${prefix}_${i}`,
          messageId: `${prefix}_mid_${i}`,
        }),
      ),
    );

    // All should succeed with distinct event IDs.
    const eventIds = new Set(results.map((r) => r.event_id));
    expect(eventIds.size).toBe(count);
    for (const r of results) {
      expect(r.event_id).toContain("$");
    }
  });

  test("rapid Matrix→external messages all arrive at webhook", async () => {
    const count = 5;
    const prefix = `rapid_mtx_${Date.now()}`;
    const countBefore = receiver.messages.length;

    // Send multiple events in a single transaction.
    const events = Array.from({ length: count }, (_, i) => ({
      type: "m.room.message",
      room_id: realMatrixRoom,
      sender,
      event_id: `$${prefix}_${i}`,
      origin_server_ts: Date.now() + i,
      content: { msgtype: "m.text", body: `${prefix}_${i}` },
    }));

    await pushTransaction(`txn_rapid_${Date.now()}`, events);

    // Wait for all messages.
    await waitFor(
      () => {
        const received = receiver.messages
          .slice(countBefore)
          .filter((m: any) => (m as any)?.message?.content?.body?.startsWith(prefix));
        return received.length >= count;
      },
      { label: `all ${count} rapid messages received`, timeout: 15_000 },
    );
  });

  test("multiple transactions in quick succession", async () => {
    const prefix = `multi_txn_${Date.now()}`;
    const countBefore = receiver.messages.length;

    // Fire multiple transactions without waiting.
    const txnPromises = Array.from({ length: 5 }, (_, i) =>
      pushTransaction(`txn_multi_${prefix}_${i}`, [
        {
          type: "m.room.message",
          room_id: realMatrixRoom,
          sender,
          event_id: `$${prefix}_${i}`,
          origin_server_ts: Date.now() + i,
          content: { msgtype: "m.text", body: `${prefix}_${i}` },
        },
      ]),
    );

    await Promise.all(txnPromises);

    await waitFor(
      () => {
        const received = receiver.messages
          .slice(countBefore)
          .filter((m: any) => (m as any)?.message?.content?.body?.startsWith(prefix));
        return received.length >= 5;
      },
      { label: "all multi-txn messages received", timeout: 15_000 },
    );
  });

  test("transaction idempotency — same txnId replayed", async () => {
    const txnId = `txn_idempotent_${Date.now()}`;
    const body = `idempotent_${Date.now()}`;
    const countBefore = receiver.messages.length;

    const event = {
      type: "m.room.message",
      room_id: realMatrixRoom,
      sender,
      event_id: `$idempotent_${Date.now()}`,
      origin_server_ts: Date.now(),
      content: { msgtype: "m.text", body },
    };

    // Send the same transaction twice.
    await pushTransaction(txnId, [event]);
    await pushTransaction(txnId, [event]);

    await Bun.sleep(3000);

    // Should only be delivered once (txn idempotency).
    const received = receiver.messages
      .slice(countBefore)
      .filter((m: any) => (m as any)?.message?.content?.body === body);
    expect(received.length).toBeLessThanOrEqual(1);
  });
});
