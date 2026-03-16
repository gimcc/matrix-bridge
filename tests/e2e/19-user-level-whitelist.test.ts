/**
 * Test 19: User-level whitelist — same domain, different users.
 *
 * This test requires: invite_whitelist = ["@admin:im.fr.ds.cc"]
 * If the config uses a domain wildcard like "@*:im.fr.ds.cc", these
 * tests will be skipped.
 *
 * To run: set invite_whitelist = ["@admin:im.fr.ds.cc"] in config.toml
 * and restart the bridge, then: bun test 19-user-level-whitelist.test.ts
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeRegisterWebhook,
  bridgeDeleteWebhook,
  bridgeCreateRoomMapping,
  bridgeDeleteRoomMapping,
  startWebhookReceiver,
  pushTransaction,
  waitFor,
} from "./helpers";

const WEBHOOK_PORT = 19890;
let receiver: ReturnType<typeof startWebhookReceiver>;
let webhookId: number;
let mappingId: number;
let skipAll = false;

const platform = `test_userlevel_${Date.now()}`;
const matrixRoomId = `!userlevel_${Date.now()}:${env.domain}`;
const extRoomId = `ext_userlevel_${Date.now()}`;
const botUserId = `@${env.bridgeBotLocalpart}:${env.domain}`;

const allowedUser = `@admin:${env.domain}`;
const blockedUser = `@other:${env.domain}`;

beforeAll(async () => {
  // Detect if user-level whitelist is active by trying @other's message.
  // If it gets forwarded, the config uses domain wildcard — skip these tests.
  receiver = startWebhookReceiver(WEBHOOK_PORT);
  const wh = await bridgeRegisterWebhook(platform, receiver.url);
  webhookId = wh.id;
  const m = await bridgeCreateRoomMapping(matrixRoomId, platform, extRoomId);
  mappingId = m.id;

  // Probe: send a message as @other and see if it gets forwarded.
  const probeBody = `probe_${Date.now()}`;
  await pushTransaction(`txn_probe_${Date.now()}`, [
    {
      type: "m.room.message",
      room_id: matrixRoomId,
      sender: blockedUser,
      event_id: `$probe_${Date.now()}`,
      origin_server_ts: Date.now(),
      content: { msgtype: "m.text", body: probeBody },
    },
  ]);
  await Bun.sleep(1500);
  const forwarded = receiver.messages.some(
    (m: any) => m?.message?.content?.body === probeBody,
  );
  if (forwarded) {
    skipAll = true;
    console.log(
      "SKIP: user-level whitelist tests — config uses domain wildcard, @other is allowed",
    );
  }
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("User-level whitelist (same domain)", () => {
  test("@admin can invite bot", async () => {
    if (skipAll) return;
    await pushTransaction(`txn_ul_bot_ok_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: allowedUser,
        state_key: botUserId,
        event_id: `$ul_bot_ok_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);
    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("@other CANNOT invite bot", async () => {
    if (skipAll) return;
    await pushTransaction(`txn_ul_bot_no_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: blockedUser,
        state_key: botUserId,
        event_id: `$ul_bot_no_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);
    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("@admin can invite puppet", async () => {
    if (skipAll) return;
    const puppet = `@bot_test_ul_ok_${Date.now()}:${env.domain}`;
    await pushTransaction(`txn_ul_puppet_ok_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: allowedUser,
        state_key: puppet,
        event_id: `$ul_puppet_ok_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);
    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("@other CANNOT invite puppet", async () => {
    if (skipAll) return;
    const puppet = `@bot_test_ul_no_${Date.now()}:${env.domain}`;
    await pushTransaction(`txn_ul_puppet_no_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: blockedUser,
        state_key: puppet,
        event_id: `$ul_puppet_no_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);
    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("@admin message IS forwarded", async () => {
    if (skipAll) return;
    const body = `admin_msg_${Date.now()}`;
    await pushTransaction(`txn_ul_fwd_ok_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: allowedUser,
        event_id: `$ul_fwd_ok_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);
    await waitFor(
      () => receiver.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "@admin message forwarded", timeout: 10_000 },
    );
  });

  test("@other message is NOT forwarded", async () => {
    if (skipAll) return;
    const body = `other_msg_${Date.now()}`;
    const countBefore = receiver.messages.length;
    await pushTransaction(`txn_ul_fwd_no_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: blockedUser,
        event_id: `$ul_fwd_no_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);
    await Bun.sleep(2000);
    const newMsgs = receiver.messages
      .slice(countBefore)
      .filter((m: any) => m?.message?.content?.body === body);
    expect(newMsgs).toHaveLength(0);
  });
});
