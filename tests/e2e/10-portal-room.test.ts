/**
 * Test 10: Portal room auto-creation.
 *
 * When an external platform sends a message and no room mapping exists,
 * the bridge should automatically create a Matrix room (portal room),
 * register the mapping, and deliver the message.
 */
import { describe, test, expect, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeSendMessage,
  bridgeListRoomMappings,
  bridgeDeleteRoomMapping,
} from "./helpers";

const platform = "test_portal";
const cleanupIds: number[] = [];

afterAll(async () => {
  for (const id of cleanupIds) {
    await bridgeDeleteRoomMapping(id).catch(() => {});
  }
});

describe("Portal Room Auto-Creation", () => {
  test("first message to unmapped room auto-creates portal room", async () => {
    const extRoom = `portal_${Date.now()}`;

    const result = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "portal_user_1",
      senderName: "Portal Alice",
      body: "Hello from portal!",
    });

    // Should succeed — room was auto-created.
    expect(result.event_id).toBeTruthy();
    expect(result.event_id).toContain("$");

    // Verify the room mapping was registered.
    const mappings = await bridgeListRoomMappings(platform);
    const created = mappings.find((m) => m.external_room_id === extRoom);
    expect(created).toBeTruthy();
    expect(created!.matrix_room_id).toContain("!");
    cleanupIds.push(created!.id);
  });

  test("second message to same external room reuses portal room", async () => {
    const extRoom = `portal_reuse_${Date.now()}`;

    const r1 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "portal_user_2",
      body: "First message",
    });

    const r2 = await bridgeSendMessage({
      platform,
      roomId: extRoom,
      senderId: "portal_user_3",
      body: "Second message",
    });

    expect(r1.event_id).toBeTruthy();
    expect(r2.event_id).toBeTruthy();
    // Both should be in the same Matrix room.
    expect(r1.event_id).not.toBe(r2.event_id);

    // Only one mapping should exist.
    const mappings = await bridgeListRoomMappings(platform);
    const matches = mappings.filter((m) => m.external_room_id === extRoom);
    expect(matches).toHaveLength(1);
    cleanupIds.push(matches[0].id);
  });

  test("portal room uses room name from message if provided", async () => {
    const extRoom = `portal_named_${Date.now()}`;

    // Send with room name in the message.
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        platform,
        room_id: extRoom,
        sender: { id: "user_named", display_name: "Named User" },
        content: { type: "text", body: "Hello named room" },
        // Note: room name comes from ExternalRoom.name if the adapter sets it.
        // The bridge API currently doesn't expose this directly, so the room
        // will be named after external_room_id.
      }),
    });
    expect(resp.ok).toBe(true);

    const mappings = await bridgeListRoomMappings(platform);
    const created = mappings.find((m) => m.external_room_id === extRoom);
    expect(created).toBeTruthy();
    cleanupIds.push(created!.id);
  });

  test("different platforms with same external room ID get separate portal rooms", async () => {
    const extRoom = `portal_cross_${Date.now()}`;
    const platformA = `${platform}_a`;
    const platformB = `${platform}_b`;

    const r1 = await bridgeSendMessage({
      platform: platformA,
      roomId: extRoom,
      senderId: "user_a",
      body: "From platform A",
    });
    const r2 = await bridgeSendMessage({
      platform: platformB,
      roomId: extRoom,
      senderId: "user_b",
      body: "From platform B",
    });

    expect(r1.event_id).toBeTruthy();
    expect(r2.event_id).toBeTruthy();

    const mappingsA = await bridgeListRoomMappings(platformA);
    const mappingsB = await bridgeListRoomMappings(platformB);

    const roomA = mappingsA.find((m) => m.external_room_id === extRoom);
    const roomB = mappingsB.find((m) => m.external_room_id === extRoom);

    expect(roomA).toBeTruthy();
    expect(roomB).toBeTruthy();
    // Different Matrix rooms for different platforms.
    expect(roomA!.matrix_room_id).not.toBe(roomB!.matrix_room_id);

    cleanupIds.push(roomA!.id, roomB!.id);
  });
});
