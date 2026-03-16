/**
 * Test 5: Error code mapping — BridgeError variants return correct HTTP status codes.
 */
import { describe, test, expect } from "bun:test";
import { env } from "./env";

describe("Error Code Mapping", () => {
  test("unmapped room auto-creates portal (returns 200)", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        platform: "nonexistent_platform",
        room_id: `no_such_room_${Date.now()}`,
        sender: { id: "user1", display_name: "User" },
        content: { type: "text", body: "test" },
      }),
    });
    // Portal auto-creation means unmapped rooms get created automatically.
    expect(resp.status).toBe(200);
  });

  test("400 for invalid request body", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "not json",
    });
    // Axum returns 400 for deserialization errors.
    expect(resp.status).toBe(400);
  });

  test("422 for missing required fields", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ platform: "test" }),
    });
    // Axum returns 422 for JSON with missing required fields.
    expect(resp.status).toBe(422);
  });

  test("400 for missing platform query on GET /api/v1/rooms", async () => {
    const resp = await fetch(`${env.bridgeUrl}/api/v1/rooms`);
    expect(resp.status).toBe(400);
    const data = (await resp.json()) as { error: string };
    expect(data.error).toContain("platform");
  });
});
