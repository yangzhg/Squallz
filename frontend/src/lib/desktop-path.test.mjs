import assert from "node:assert/strict";
import test from "node:test";

import { createTestServer } from "../../tests/runtime.mjs";

test("folder normalization trims absolute paths without accepting relative input", async () => {
  const server = await createTestServer();

  try {
    const { normalizeDesktopFolder } = await server.ssrLoadModule(
      "/src/lib/desktop-path.ts",
    );

    assert.equal(
      normalizeDesktopFolder("  /Users/alex/Archives/  ", "macos"),
      "/Users/alex/Archives",
    );
    assert.equal(
      normalizeDesktopFolder("  C:\\Users\\Alex\\Archives\\  ", "windows"),
      "C:/Users/Alex/Archives",
    );
    assert.equal(
      normalizeDesktopFolder("  \\\\server\\share\\Archives\\  ", "windows"),
      "//server/share/Archives",
    );
    assert.equal(normalizeDesktopFolder("Archives", "macos"), null);
    assert.equal(normalizeDesktopFolder(".\\Archives", "windows"), null);
    assert.equal(normalizeDesktopFolder("   ", "linux"), null);
  } finally {
    await server.close();
  }
});
