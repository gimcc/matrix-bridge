/**
 * Test 12: Redaction (message deletion) bridging.
 *
 * When a Matrix user redacts a message, the bridge should forward a
 * Redaction content type to the platform webhook.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeCreateRoomMapping,
  bridgeRegisterWebhook,
  bridgeDeleteRoomMapping,
  bridgeDeleteWebhook,
  bridgeSendMessage,
  startWebhookReceiver,
  pushTransaction,
  waitFor,
} from "./helpers";

const WEBHOOK_PORT = 19881;
let receiver: ReturnType<typeof startWebhookReceiver>;
let mappingId: number;
let webhookId: number;

const platform = "test_redact";
const extRoom = `ext_redact_${Date.now()}`;
const matrixRoom = `!redact_${Date.now()}:${env.domain}`;
const sender = `@redactor:${env.domain}`;

beforeAll(async () => {
  receiver = startWebhookReceiver(WEBHOOK_PORT);
  const m = await bridgeCreateRoomMapping(matrixRoom, platform, extRoom);
  mappingId = m.id;
  const wh = await bridgeRegisterWebhook(platform, receiver.url, ["*"]);
  webhookId = wh.id;
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Redaction Bridging", () => {
  test("send message then redact → webhook receives both", async () => {
    const eventId = `$msg_to_redact_${Date.now()}`;
    const msgBody = `will be redacted ${Date.now()}`;

    // 1. Send original message.
    await pushTransaction(`txn_redact_msg_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: eventId,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body: msgBody },
      },
    ]);

    await waitFor(
      () => receiver.messages.some((m: any) => m?.message?.content?.body === msgBody),
      { label: "original message received", timeout: 10_000 },
    );

    // 2. Redact the message.
    await pushTransaction(`txn_redact_${Date.now()}`, [
      {
        type: "m.room.redaction",
        room_id: matrixRoom,
        sender,
        event_id: `$redaction_${Date.now()}`,
        redacts: eventId,
        origin_server_ts: Date.now(),
        content: {},
      },
    ]);

    // 3. Webhook should receive a redaction event.
    await waitFor(
      () =>
        receiver.messages.some(
          (m: any) => m?.message?.content?.type === "redaction",
        ),
      { label: "redaction received", timeout: 10_000 },
    );

    const redaction = receiver.messages.find(
      (m: any) => m?.message?.content?.type === "redaction",
    ) as any;
    expect(redaction.message.content.target_id).toBeTruthy();
  });

  test("redaction of unknown event is silently ignored", async () => {
    const countBefore = receiver.messages.length;

    await pushTransaction(`txn_redact_unknown_${Date.now()}`, [
      {
        type: "m.room.redaction",
        room_id: matrixRoom,
        sender,
        event_id: `$redact_unknown_${Date.now()}`,
        redacts: `$nonexistent_event_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: {},
      },
    ]);

    await Bun.sleep(1500);
    // No new messages — the redaction target was never bridged.
    expect(receiver.messages.length).toBe(countBefore);
  });
});
