/**
 * Test 1: Bridge health check and basic connectivity.
 */
import { describe, test, expect } from "bun:test";
import { bridgeHealth } from "./helpers";

describe("Bridge Health", () => {
  test("GET /health returns ok", async () => {
    const ok = await bridgeHealth();
    expect(ok).toBe(true);
  });
});
