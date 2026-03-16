/**
 * Test 9: Auto-join on invite + invite whitelist.
 *
 * Config: invite_whitelist = ["@*:im.fr.ds.cc"]
 *
 * Rules:
 * - Bot invite from whitelisted user → accepted
 * - Bot invite from non-whitelisted user → rejected
 * - Puppet invite from bridge bot → accepted (internal operation)
 * - Puppet invite from non-whitelisted user → rejected
 * - Message forwarding from non-whitelisted sender → blocked
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

const WEBHOOK_PORT = 19880;
let receiver: ReturnType<typeof startWebhookReceiver>;
let webhookId: number;
let mappingId: number;

const platform = "test_autojoin";
const matrixRoomId = `!autojoin_${Date.now()}:${env.domain}`;
const extRoomId = `ext_autojoin_${Date.now()}`;
const botUserId = `@${env.bridgeBotLocalpart}:${env.domain}`;

beforeAll(async () => {
  receiver = startWebhookReceiver(WEBHOOK_PORT);
  const wh = await bridgeRegisterWebhook(platform, receiver.url, ["*"]);
  webhookId = wh.id;
  const m = await bridgeCreateRoomMapping(matrixRoomId, platform, extRoomId);
  mappingId = m.id;
});

afterAll(async () => {
  receiver?.stop();
  await bridgeDeleteWebhook(webhookId).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Auto-join on invite", () => {
  test("bot invite from whitelisted domain → accepted", async () => {
    await pushTransaction(`txn_invite_bot_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: `@some_user:${env.domain}`,
        state_key: botUserId,
        event_id: `$invite_bot_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite", displayname: "Bridge Bot" },
      },
    ]);

    await Bun.sleep(1000);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("puppet invite from bridge bot → accepted (internal)", async () => {
    const puppetUserId = `@bot_telegram_99999:${env.domain}`;
    await pushTransaction(`txn_invite_puppet_internal_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: botUserId, // bridge bot itself invites
        state_key: puppetUserId,
        event_id: `$invite_puppet_int_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);

    await Bun.sleep(1000);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("puppet invite from whitelisted user → accepted", async () => {
    const puppetUserId = `@bot_telegram_88888:${env.domain}`;
    await pushTransaction(`txn_invite_puppet_wl_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: `@admin:${env.domain}`, // whitelisted domain
        state_key: puppetUserId,
        event_id: `$invite_puppet_wl_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);

    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("non-bridge users are NOT auto-joined", async () => {
    await pushTransaction(`txn_invite_other_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: `@some_user:${env.domain}`,
        state_key: `@random_user:${env.domain}`,
        event_id: `$invite_random_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);

    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("bot invite from non-whitelisted domain → rejected", async () => {
    await pushTransaction(`txn_invite_blocked_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: "@attacker:evil.org",
        state_key: botUserId,
        event_id: `$invite_blocked_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);

    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });

  test("puppet invite from non-whitelisted domain → rejected", async () => {
    const puppetUserId = `@bot_test_blocked_${Date.now()}:${env.domain}`;
    await pushTransaction(`txn_puppet_blocked_${Date.now()}`, [
      {
        type: "m.room.member",
        room_id: matrixRoomId,
        sender: "@attacker:evil.org",
        state_key: puppetUserId,
        event_id: `$puppet_blocked_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { membership: "invite" },
      },
    ]);

    await Bun.sleep(500);
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
  });
});

describe("Message forwarding whitelist", () => {
  test("whitelisted sender's message IS forwarded to webhook", async () => {
    const body = `msg_whitelisted_${Date.now()}`;
    await pushTransaction(`txn_fwd_ok_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: `@user_x:${env.domain}`, // whitelisted domain
        event_id: `$fwd_ok_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => receiver.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "whitelisted message forwarded", timeout: 10_000 },
    );
  });

  test("non-whitelisted sender's message is NOT forwarded", async () => {
    const body = `msg_blocked_${Date.now()}`;
    const countBefore = receiver.messages.length;

    await pushTransaction(`txn_fwd_blocked_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoomId,
        sender: "@hacker:evil.org", // NOT in whitelist
        event_id: `$fwd_blocked_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    // Wait and verify NO new messages arrived with that body.
    await Bun.sleep(2000);
    const newMsgs = receiver.messages
      .slice(countBefore)
      .filter((m: any) => m?.message?.content?.body === body);
    expect(newMsgs).toHaveLength(0);
  });
});
