/**
 * Test 13: Multi-platform room bridging.
 *
 * A single Matrix room can be bridged to multiple external platforms
 * simultaneously. Messages from one platform should not leak to another
 * platform's webhook.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeCreateRoomMapping,
  bridgeRegisterWebhook,
  bridgeDeleteRoomMapping,
  bridgeDeleteWebhook,
  bridgeListRoomMappings,
  bridgeSendMessage,
  startWebhookReceiver,
  pushTransaction,
  waitFor,
} from "./helpers";

const WEBHOOK_PORT_A = 19882;
const WEBHOOK_PORT_B = 19883;
let receiverA: ReturnType<typeof startWebhookReceiver>;
let receiverB: ReturnType<typeof startWebhookReceiver>;

let mappingIdB: number;
let webhookIdA: number;
let webhookIdB: number;

const platformA = `test_multi_a_${Date.now()}`;
const platformB = `test_multi_b_${Date.now()}`;
const extRoomA = `ext_multi_a_${Date.now()}`;
const extRoomB = `ext_multi_b_${Date.now()}`;
let matrixRoom: string;
const sender = `@multi_user:${env.domain}`;

beforeAll(async () => {
  receiverA = startWebhookReceiver(WEBHOOK_PORT_A);
  receiverB = startWebhookReceiver(WEBHOOK_PORT_B);

  // 1. Send a message via platform A to auto-create a real portal room.
  await bridgeSendMessage({
    platform: platformA,
    roomId: extRoomA,
    senderId: "setup_user",
    senderName: "Setup",
    body: "init portal room",
  });

  // 2. Look up the auto-created matrix room ID.
  const mappingsA = await bridgeListRoomMappings(platformA);
  const portalA = mappingsA.find((m) => m.external_room_id === extRoomA);
  if (!portalA) throw new Error("portal room not auto-created for platform A");
  matrixRoom = portalA.matrix_room_id;

  // 3. Map the SAME matrix room to platform B.
  const mB = await bridgeCreateRoomMapping(matrixRoom, platformB, extRoomB);
  mappingIdB = mB.id;

  // 4. Register webhooks.
  const whA = await bridgeRegisterWebhook(platformA, receiverA.url, ["*"]);
  webhookIdA = whA.id;
  const whB = await bridgeRegisterWebhook(platformB, receiverB.url, ["*"]);
  webhookIdB = whB.id;
});

afterAll(async () => {
  receiverA?.stop();
  receiverB?.stop();
  await bridgeDeleteWebhook(webhookIdA).catch(() => {});
  await bridgeDeleteWebhook(webhookIdB).catch(() => {});
  await bridgeDeleteRoomMapping(mappingIdB).catch(() => {});
});

describe("Multi-Platform Room", () => {
  test("Matrix message dispatches to all platform webhooks", async () => {
    const body = `multi_dispatch_${Date.now()}`;

    await pushTransaction(`txn_multi_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: `$multi_msg_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiverA.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "platform A received", timeout: 10_000 },
    );
    await waitFor(
      () => receiverB.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "platform B received", timeout: 10_000 },
    );
  });

  test("external message from platform A does not echo to platform A webhook", async () => {
    const body = `from_a_${Date.now()}`;
    const countA = receiverA.messages.length;

    await bridgeSendMessage({
      platform: platformA,
      roomId: extRoomA,
      senderId: "ext_user_a",
      senderName: "Ext User A",
      body,
    });

    // Platform B should receive (it's a different platform).
    await waitFor(
      () => receiverB.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "platform B received from A", timeout: 10_000 },
    );

    // Platform A should NOT get its own message back (dedup).
    await Bun.sleep(1500);
    const newA = receiverA.messages
      .slice(countA)
      .filter((m: any) => m?.message?.content?.body === body);
    expect(newA).toHaveLength(0);
  });

  test("external message from platform B reaches platform A webhook", async () => {
    const body = `from_b_${Date.now()}`;

    await bridgeSendMessage({
      platform: platformB,
      roomId: extRoomB,
      senderId: "ext_user_b",
      senderName: "Ext User B",
      body,
    });

    await waitFor(
      () => receiverA.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "platform A received from B", timeout: 10_000 },
    );
  });
});
