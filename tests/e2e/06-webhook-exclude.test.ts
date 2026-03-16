/**
 * Test 6: Webhook forward_sources allowlist filtering.
 *
 * forward_sources controls which source platforms are forwarded:
 * - empty = deny all (nothing forwarded)
 * - ["*"] = forward all
 * - ["telegram"] = only forward telegram puppet messages
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

  // Webhook A: forward_sources=["*"] → receives ALL sources.
  const whAll = await bridgeRegisterWebhook(platform, receiverAll.url, ["*"]);
  whAllId = whAll.id;

  // Webhook B: forward_sources=["matrix"] → only forwards real Matrix user messages,
  // NOT puppet messages from telegram or other platforms.
  const whFiltered = await bridgeRegisterWebhook(
    platform,
    receiverFiltered.url,
    ["matrix"],
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

describe("Webhook forward_sources", () => {
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

    // Both webhooks allow "matrix" source (A via "*", B explicitly).
    await waitFor(
      () => receiverAll.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverAll gets message", timeout: 10_000 },
    );
    await waitFor(
      () => receiverFiltered.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverFiltered gets message", timeout: 5_000 },
    );
  });

  test("puppet message from telegram NOT delivered to matrix-only webhook", async () => {
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

    // receiverAll should get it (forward_sources=["*"]).
    await waitFor(
      () => receiverAll.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "receiverAll gets puppet message", timeout: 10_000 },
    );

    // receiverFiltered should NOT get it (forward_sources=["matrix"], telegram not listed).
    await Bun.sleep(2000);
    expect(receiverFiltered.messages.length).toBe(countFilteredBefore);
  });
});
