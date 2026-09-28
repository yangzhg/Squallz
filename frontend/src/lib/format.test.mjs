import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

test("modification dates preserve historical and epoch values in local time", async () => {
  const server = await createTestServer();
  try {
    const { formatModified } = await server.ssrLoadModule("/src/lib/format.ts");
    for (const [year, month, day, hour, minute] of [
      [1900, 1, 1, 12, 30], [1969, 7, 20, 20, 17], [1970, 1, 1, 0, 0],
      [new Date().getFullYear(), 6, 15, 9, 5],
    ]) {
      const date = new Date(year, month - 1, day, hour, minute);
      const pad = (n) => String(n).padStart(2, "0");
      assert.equal(formatModified(date.getTime() / 1000),
        `${year}-${pad(month)}-${pad(day)} ${pad(hour)}:${pad(minute)}`);
    }
    assert.notEqual(formatModified(0), "-");
    assert.notEqual(formatModified(-1), "-");
    for (const unavailable of [null, NaN, Infinity, -Infinity, Number.MAX_VALUE, -Number.MAX_VALUE]) {
      assert.equal(formatModified(unavailable), "-");
    }
  } finally {
    await server.close();
  }
});
