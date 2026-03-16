/**
 * Test 3: Message bridging — External → Matrix (plaintext).
 *
 * Tests the /api/v1/message endpoint: external platform sends a message
 * that should appear in the mapped Matrix room as a puppet user.
 */
import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { env } from "./env";
import {
  registerUser,
  createRoom,
  inviteUser,
  getRoomMessages,
  bridgeCreateRoomMapping,
  bridgeSendMessage,
  bridgeDeleteRoomMapping,
  waitFor,
} from "./helpers";

let userToken: string;
let roomId: string;
let mappingId: number;
const platform = "test_msg";
const extRoomId = `ext_msg_${Date.now()}`;

beforeAll(async () => {
  // Create a test user and room.
  const user = await registerUser(`msgtest_${Date.now()}`, "password123");
  userToken = user.access_token;

  // Invite the bridge bot so it can puppet into the room.
  const botUserId = `@${env.bridgeBotLocalpart}:${env.domain}`;
  roomId = await createRoom(userToken, {
    name: "Message Bridge Test",
    invite: [botUserId],
  });

  // Create room mapping via bridge API.
  const mapping = await bridgeCreateRoomMapping(roomId, platform, extRoomId);
  mappingId = mapping.id;

  // Small delay for homeserver to process.
  await Bun.sleep(500);
});

afterAll(async () => {
  await bridgeDeleteRoomMapping(mappingId).catch(() => {});
});

describe("External → Matrix (plaintext)", () => {
  test("message appears in Matrix room from puppet user", async () => {
    const body = `Hello from external! ${Date.now()}`;

    const result = await bridgeSendMessage({
      platform,
      roomId: extRoomId,
      senderId: "ext_user_42",
      senderName: "External Alice",
      body,
    });

    expect(result.event_id).toBeTruthy();
    expect(result.event_id).toContain("$");

    // Verify the message appears in the room.
    await waitFor(
      async () => {
        const msgs = await getRoomMessages(userToken, roomId, 10);
        return msgs.some(
          (m) =>
            m.type === "m.room.message" &&
            (m.content as { body?: string })?.body === body,
        );
      },
      { label: "message appears in Matrix room", timeout: 10_000 },
    );
  });

  test("unmapped room triggers portal auto-creation (returns 200)", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        platform,
        room_id: `auto_portal_${Date.now()}`,
        sender: { id: "user1", display_name: "Auto User" },
        content: { type: "text", body: "triggers portal room" },
      }),
    });
    // Portal room auto-creation means unmapped rooms get created on the fly.
    expect(resp.status).toBe(200);
    const data = (await resp.json()) as { event_id: string };
    expect(data.event_id).toBeTruthy();
  });

  test("returns 400 for invalid puppet localpart", async () => {
    // Create a mapping for this test.
    const badExtRoom = `ext_bad_${Date.now()}`;
    const m = await bridgeCreateRoomMapping(roomId, "test_bad", badExtRoom);

    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        platform: "test_bad",
        room_id: badExtRoom,
        sender: { id: "USER WITH SPACES AND CAPS" },
        content: { type: "text", body: "should fail validation" },
      }),
    });
    expect(resp.status).toBe(400);

    await bridgeDeleteRoomMapping(m.id).catch(() => {});
  });
});
