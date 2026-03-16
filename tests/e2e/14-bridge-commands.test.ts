/**
 * Test 14: Bridge commands via Matrix messages.
 *
 * Users can send !bridge commands (link, unlink, status) in Matrix rooms.
 * These commands are processed by the bridge and not forwarded to webhooks.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeRegisterWebhook,
  bridgeDeleteWebhook,
  bridgeListRoomMappings,
  bridgeSendMessage,
  startWebhookReceiver,
  pushTransaction,
} from "./helpers";

const WEBHOOK_PORT = 19884;
let receiver: ReturnType<typeof startWebhookReceiver>;
let webhookId: number;
let realMatrixRoom: string;

const platform = "test_cmd";
const extRoom = `ext_cmd_${Date.now()}`;
const sender = `@commander:${env.domain}`;
const botUser = `@${env.bridgeBotLocalpart}:${env.domain}`;

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
  const wh = await bridgeRegisterWebhook(platform, receiver.url);
  webhookId = wh.id;
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
});

describe("Bridge Commands", () => {
  test("!bridge commands are not forwarded to webhooks", async () => {
    const countBefore = receiver.messages.length;

    await pushTransaction(`txn_cmd_nofwd_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: realMatrixRoom,
        sender,
        event_id: `$cmd_nofwd_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: {
          msgtype: "m.text",
          body: `!bridge status`,
        },
      },
    ]);

    // Commands should NOT reach the webhook.
    await Bun.sleep(2000);
    expect(receiver.messages.length).toBe(countBefore);
  });

  test("bot's own messages are not forwarded", async () => {
    const countBefore = receiver.messages.length;

    await pushTransaction(`txn_bot_msg_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: realMatrixRoom,
        sender: botUser,
        event_id: `$bot_msg_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body: "I am the bridge bot" },
      },
    ]);

    await Bun.sleep(2000);
    expect(receiver.messages.length).toBe(countBefore);
  });
});
