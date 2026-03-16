/**
 * Test 6: Webhook exclude_sources filtering.
 *
 * When a puppet user sends a message (cross-platform forward), the webhook
 * with that source excluded should NOT receive it.
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

const WEBHOOK_PORT_A = 19877;
const WEBHOOK_PORT_B = 19878;

let receiverAll: ReturnType<typeof startWebhookReceiver>;
let receiverFiltered: ReturnType<typeof startWebhookReceiver>;
let mappingId: number;
let whAllId: number;
let whFilteredId: number;

const platform = "test_exclude";
const extRoomId = `ext_excl_${Date.now()}`;
const matrixRoomId = `!excl_test_${Date.now()}:${env.domain}`;

beforeAll(async () => {
  receiverAll = startWebhookReceiver(WEBHOOK_PORT_A);
  receiverFiltered = startWebhookReceiver(WEBHOOK_PORT_B);

  const mapping = await bridgeCreateRoomMapping(matrixRoomId, platform, extRoomId);
  mappingId = mapping.id;

  // Webhook A: receives ALL sources.
  const whAll = await bridgeRegisterWebhook(platform, receiverAll.url);
  whAllId = whAll.id;

  // Webhook B: excludes messages originating from "telegram".
  const whFiltered = await bridgeRegisterWebhook(
    platform,
    receiverFiltered.url,
    ["telegram"],
  );
  whFilteredId = whFiltered.id;
});

afterAll(async () => {
  receiverAll?.stop();
  receiverFiltered?.stop();
  await bridgeDeleteWebhook(whAllId).catch(() => {});
  await bridgeDeleteWebhook(whFilteredId).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Webhook exclude_sources", () => {
  test("non-puppet message delivered to both webhooks", async () => {
    const body = `normal_msg_${Date.now()}`;

    await pushTransaction(`txn_excl_normal_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: `@real_user:${env.domain}`,
        event_id: `$excl_normal_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiverAll.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverAll gets message", timeout: 10_000 },
    );
    await waitFor(
      () => receiverFiltered.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverFiltered gets message", timeout: 5_000 },
    );
  });

  test("puppet message from excluded source NOT delivered to filtered webhook", async () => {
    const body = `puppet_msg_${Date.now()}`;
    const countFilteredBefore = receiverFiltered.messages.length;

    // Puppet user from telegram — matches the "bot_telegram_" prefix.
    await pushTransaction(`txn_excl_puppet_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: `@bot_telegram_12345:${env.domain}`,
        event_id: `$excl_puppet_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    // receiverAll should get it (puppet from telegram → forwarded to test_exclude platform).
    await waitFor(
      () => receiverAll.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverAll gets puppet message", timeout: 10_000 },
    );

    // receiverFiltered should NOT get it (telegram is excluded).
    await Bun.sleep(2000);
    expect(receiverFiltered.messages.length).toBe(countFilteredBefore);
  });
});
