import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("created archive opening falls back to reveal only when opening fails", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { openCreatedOutputWithFallback } = await server.ssrLoadModule(
      "/src/lib/create-completion.ts",
    );
    let reveals = 0;

    assert.equal(
      await openCreatedOutputWithFallback(
        async () => true,
        async () => {
          reveals += 1;
          return true;
        },
      ),
      "opened",
    );
    assert.equal(reveals, 0);

    assert.equal(
      await openCreatedOutputWithFallback(
        async () => false,
        async () => {
          reveals += 1;
          return true;
        },
      ),
      "revealed",
    );
    assert.equal(reveals, 1);

    assert.equal(
      await openCreatedOutputWithFallback(
        async () => {
          throw new Error("open failed");
        },
        async () => {
          reveals += 1;
          return false;
        },
      ),
      "unavailable",
    );
    assert.equal(reveals, 2);
  } finally {
    await server.close();
  }
});

test("created archive results retain integrity-test evidence", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { readCreateResult } = await server.ssrLoadModule("/src/lib/create-result.ts");
    const result = readCreateResult(
      {
        primary_output: "/tmp/archive.zip",
        outputs: ["/tmp/archive.zip"],
        tested_after_create: true,
        entries_tested_after_create: 17,
      },
      "/tmp/fallback.zip",
      false,
    );
    assert.equal(result.testedAfterCreate, true);
    assert.equal(result.entriesTestedAfterCreate, 17);

    const missingResult = readCreateResult(null, "/tmp/fallback.zip", false);
    assert.equal(missingResult.testedAfterCreate, false);
    assert.equal(missingResult.entriesTestedAfterCreate, 0);
  } finally {
    await server.close();
  }
});
