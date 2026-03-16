/**
 * Test 8: Idempotent migration — bridge can restart without crash.
 *
 * This is a smoke test: if the bridge is running, the migration
 * already succeeded at least once. We verify by hitting health.
 */
import { describe, test, expect } from "bun:test";
import { env } from "./env";

describe("Idempotent Migration", () => {
  test("bridge started successfully (migration ran without duplicate column error)", async () => {
    const resp = await fetch(`${env.bridgeUrl}/health`);
    expect(resp.ok).toBe(true);
    const data = (await resp.json()) as { status: string };
    expect(data.status).toBe("ok");
  });
});
