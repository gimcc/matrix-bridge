/**
 * Test 2: Room mapping CRUD and upsert behavior.
 */
import { describe, test, expect, afterAll } from "bun:test";
import { env } from "./env";
import {
  bridgeCreateRoomMapping,
  bridgeDeleteRoomMapping,
} from "./helpers";

const cleanupIds: number[] = [];

afterAll(async () => {
  for (const id of cleanupIds) {
    await bridgeDeleteRoomMapping(id).catch(() => {});
  }
});

describe("Room Mapping API", () => {
  test("POST /api/v1/rooms creates a new mapping", async () => {
    const result = await bridgeCreateRoomMapping(
      `!test_create_${Date.now()}:${env.domain}`,
      "test_platform",
      `ext_room_${Date.now()}`,
    );
    expect(result.id).toBeGreaterThan(0);
    cleanupIds.push(result.id);
  });

  test("upsert: same (matrix_room_id, platform) updates external_room_id", async () => {
    const matrixRoom = `!test_upsert_m_${Date.now()}:${env.domain}`;
    const r1 = await bridgeCreateRoomMapping(matrixRoom, "test_upsert", "ext_a");
    cleanupIds.push(r1.id);

    const r2 = await bridgeCreateRoomMapping(matrixRoom, "test_upsert", "ext_b");
    // Should return the same ID (updated, not new row).
    expect(r2.id).toBe(r1.id);
  });

  test("upsert: same (platform, external_room_id) updates matrix_room_id", async () => {
    const extRoom = `ext_upsert_${Date.now()}`;
    const r1 = await bridgeCreateRoomMapping(
      `!room_old_${Date.now()}:${env.domain}`,
      "test_upsert2",
      extRoom,
    );
    cleanupIds.push(r1.id);

    const r2 = await bridgeCreateRoomMapping(
      `!room_new_${Date.now()}:${env.domain}`,
      "test_upsert2",
      extRoom,
    );
    expect(r2.id).toBe(r1.id);
  });

  test("DELETE /api/v1/rooms/:id returns 404 for non-existent", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/rooms/999999`, {
      method: "DELETE",
    });
    expect(resp.status).toBe(404);
  });
});
