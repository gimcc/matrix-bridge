/**
 * Test 18: Multiple webhooks per platform.
 *
 * A platform can have multiple webhook endpoints registered.
 * All active webhooks should receive messages.
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

const WEBHOOK_PORT_1 = 19889;
const WEBHOOK_PORT_2 = 19890;
let receiver1: ReturnType<typeof startWebhookReceiver>;
let receiver2: ReturnType<typeof startWebhookReceiver>;
let mappingId: number;
let webhookId1: number;
let webhookId2: number;

const platform = "test_multi_wh";
const extRoom = `ext_mwh_${Date.now()}`;
const matrixRoom = `!multi_wh_${Date.now()}:${env.domain}`;
const sender = `@multi_wh_user:${env.domain}`;

beforeAll(async () => {
  receiver1 = startWebhookReceiver(WEBHOOK_PORT_1);
  receiver2 = startWebhookReceiver(WEBHOOK_PORT_2);

  const m = await bridgeCreateRoomMapping(matrixRoom, platform, extRoom);
  mappingId = m.id;

  const wh1 = await bridgeRegisterWebhook(platform, receiver1.url);
  webhookId1 = wh1.id;
  const wh2 = await bridgeRegisterWebhook(platform, receiver2.url);
  webhookId2 = wh2.id;
});

afterAll(async () => {
  receiver1?.stop();
  receiver2?.stop();
  await bridgeDeleteWebhook(webhookId1).catch(() => {});
  await bridgeDeleteWebhook(webhookId2).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Multiple Webhooks Per Platform", () => {
  test("both webhooks receive the same message", async () => {
    const body = `multi_wh_${Date.now()}`;

    await pushTransaction(`txn_multi_wh_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: `$multi_wh_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiver1.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "webhook 1 received", timeout: 10_000 },
    );
    await waitFor(
      () => receiver2.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "webhook 2 received", timeout: 10_000 },
    );
  });

  test("deleting one webhook stops delivery to it but not the other", async () => {
    // Delete webhook 2.
    await bridgeDeleteWebhook(webhookId2);

    const body = `after_delete_${Date.now()}`;
    const count1Before = receiver1.messages.length;
    const count2Before = receiver2.messages.length;

    await pushTransaction(`txn_after_del_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: `$after_del_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiver1.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "webhook 1 still receives", timeout: 10_000 },
    );

    await Bun.sleep(2000);
    // Webhook 2 should NOT have received the message.
    const new2 = receiver2.messages
      .slice(count2Before)
      .filter((m: any) => (m as any)?.message?.content?.body === body);
    expect(new2).toHaveLength(0);

    // Re-register webhook 2 for cleanup.
    const wh2 = await bridgeRegisterWebhook(platform, receiver2.url);
    webhookId2 = wh2.id;
  });
});
