/**
 * Minimal test harness — no external test framework needed.
 */

interface TestResult {
  name: string;
  passed: boolean;
  error?: string;
  durationMs: number;
}

const results: TestResult[] = [];
let currentSuite = "";

export function suite(name: string): void {
  currentSuite = name;
  console.log(`\n${"=".repeat(60)}`);
  console.log(`  ${name}`);
  console.log(`${"=".repeat(60)}`);
}

export async function test(name: string, fn: () => Promise<void>): Promise<void> {
  const fullName = currentSuite ? `${currentSuite} > ${name}` : name;
  const start = Date.now();
  try {
    await fn();
    const durationMs = Date.now() - start;
    results.push({ name: fullName, passed: true, durationMs });
    console.log(`  PASS  ${name} (${durationMs}ms)`);
  } catch (err) {
    const durationMs = Date.now() - start;
    const message = err instanceof Error ? err.message : (typeof err === 'object' ? JSON.stringify(err) : String(err));
    results.push({ name: fullName, passed: false, error: message, durationMs });
    console.log(`  FAIL  ${name} (${durationMs}ms)`);
    console.log(`        ${message}`);
  }
}

export function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(`Assertion failed: ${message}`);
}

export function assertEqual<T>(actual: T, expected: T, label: string): void {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

export function assertContains(haystack: string, needle: string, label: string): void {
  if (!haystack.includes(needle)) {
    throw new Error(`${label}: expected "${haystack}" to contain "${needle}"`);
  }
}

export function assertDefined<T>(value: T | null | undefined, label: string): asserts value is T {
  if (value === null || value === undefined) {
    throw new Error(`${label}: expected non-null/undefined value`);
  }
}

export function printSummary(): void {
  console.log(`\n${"=".repeat(60)}`);
  console.log("  TEST SUMMARY");
  console.log(`${"=".repeat(60)}`);

  const passed = results.filter((r) => r.passed).length;
  const failed = results.filter((r) => !r.passed).length;
  const totalMs = results.reduce((acc, r) => acc + r.durationMs, 0);

  for (const r of results) {
    const icon = r.passed ? "PASS" : "FAIL";
    console.log(`  ${icon}  ${r.name}`);
    if (r.error) console.log(`        ${r.error}`);
  }

  console.log(`\n  ${passed} passed, ${failed} failed (${totalMs}ms total)`);
  console.log(`${"=".repeat(60)}\n`);

  if (failed > 0) process.exit(1);
}

/**
 * Wait for a condition to become true, polling every `intervalMs`.
 * Throws after `timeoutMs` if the condition is never met.
 */
export async function waitFor(
  description: string,
  conditionFn: () => Promise<boolean> | boolean,
  timeoutMs = 15000,
  intervalMs = 500,
): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await conditionFn()) return;
    await sleep(intervalMs);
  }
  throw new Error(`Timed out waiting for: ${description} (${timeoutMs}ms)`);
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
