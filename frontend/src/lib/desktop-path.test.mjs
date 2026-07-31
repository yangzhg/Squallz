import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("folder normalization trims absolute paths without accepting relative input", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

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
