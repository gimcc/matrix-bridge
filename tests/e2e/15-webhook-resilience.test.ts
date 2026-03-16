/**
 * Test 15: Webhook delivery resilience.
 *
 * Verifies that a dead or slow webhook doesn't block message bridging
 * and that the bridge handles webhook failures gracefully.
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

const WEBHOOK_PORT_GOOD = 19885;
const WEBHOOK_PORT_FAIL = 19886;

let goodReceiver: ReturnType<typeof startWebhookReceiver>;
let failServer: { stop: () => void };
let mappingId: number;
let webhookIdGood: number;
let webhookIdFail: number;

const platform = "test_resilient";
const extRoom = `ext_resilient_${Date.now()}`;
const matrixRoom = `!resilient_${Date.now()}:${env.domain}`;
const sender = `@resilient_user:${env.domain}`;

/** Start a webhook that always returns 500. */
function startFailingWebhook(port: number): { stop: () => void; callCount: number[] } {
  const callCount = [0];
  const server = Bun.serve({
    port,
    fetch: async (req) => {
      if (req.method === "POST") {
        callCount[0]++;
        return new Response("Internal Server Error", { status: 500 });
      }
      return new Response("not found", { status: 404 });
    },
  });
  return { stop: () => server.stop(), callCount };
}

let failCallCount: number[];

beforeAll(async () => {
  goodReceiver = startWebhookReceiver(WEBHOOK_PORT_GOOD);
  const fail = startFailingWebhook(WEBHOOK_PORT_FAIL);
  failServer = fail;
  failCallCount = fail.callCount;

  const m = await bridgeCreateRoomMapping(matrixRoom, platform, extRoom);
  mappingId = m.id;

  const whGood = await bridgeRegisterWebhook(platform, goodReceiver.url, ["*"]);
  webhookIdGood = whGood.id;
  const whFail = await bridgeRegisterWebhook(platform, `http://${process.env.WEBHOOK_HOST ?? "localhost"}:${WEBHOOK_PORT_FAIL}`, ["*"]);
  webhookIdFail = whFail.id;
});

afterAll(async () => {
  goodReceiver?.stop();
  failServer?.stop();
  await bridgeDeleteWebhook(webhookIdGood).catch(() => {});
  await bridgeDeleteWebhook(webhookIdFail).catch(() => {});
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("Webhook Resilience", () => {
  test("failing webhook does not block healthy webhook", async () => {
    const body = `resilient_${Date.now()}`;

    await pushTransaction(`txn_resilient_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: `$resilient_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    // Good webhook should still receive the message despite the failing one.
    await waitFor(
      () => goodReceiver.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "good webhook received despite failing peer", timeout: 10_000 },
    );
  });

  test("failing webhook was called (bridge attempted delivery)", async () => {
    // The failing webhook should have been called at least once.
    expect(failCallCount[0]).toBeGreaterThan(0);
  });

  test("dead webhook (connection refused) does not block delivery", async () => {
    // Register a webhook on a port where nothing is listening.
    const deadWebhookId = (
      await bridgeRegisterWebhook(platform, `http://${process.env.WEBHOOK_HOST ?? "localhost"}:19999`, ["*"])
    ).id;

    const body = `dead_wh_${Date.now()}`;

    await pushTransaction(`txn_dead_wh_${Date.now()}`, [
      {
        type: "m.room.message",
        room_id: matrixRoom,
        sender,
        event_id: `$dead_wh_${Date.now()}`,
        origin_server_ts: Date.now(),
        content: { msgtype: "m.text", body },
      },
    ]);

    await waitFor(
      () => goodReceiver.messages.some((m: any) => m?.message?.content?.body === body),
      { label: "good webhook received despite dead peer", timeout: 10_000 },
    );

    await bridgeDeleteWebhook(deadWebhookId).catch(() => {});
  });
});
