import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("only incomplete compress cleanup results request a recovery refresh", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const {
      isNewSourceCleanupRecoveryGeneration,
      shouldRefreshSourceCleanupRecovery,
    } = await server.ssrLoadModule(
      "/src/lib/source-cleanup.ts",
    );
    const result = (status) => ({
      source_cleanup: { status, moved: 0, kept: 1, recovery_required: 0 },
    });

    assert.equal(shouldRefreshSourceCleanupRecovery("extract", result("failed")), false);
    assert.equal(shouldRefreshSourceCleanupRecovery("compress", result("completed")), false);
    assert.equal(shouldRefreshSourceCleanupRecovery("compress", result("not_requested")), false);
    for (const status of ["partial", "blocked", "cancelled", "failed", ""]) {
      assert.equal(
        shouldRefreshSourceCleanupRecovery("compress", result(status)),
        true,
        status || "empty status",
      );
    }
    assert.equal(shouldRefreshSourceCleanupRecovery("compress", null), true);

    assert.equal(isNewSourceCleanupRecoveryGeneration(0, 1), true);
    assert.equal(isNewSourceCleanupRecoveryGeneration(1, 1), false);
    assert.equal(isNewSourceCleanupRecoveryGeneration(2, 1), false);
    assert.equal(isNewSourceCleanupRecoveryGeneration(2, 3), true);
    assert.equal(isNewSourceCleanupRecoveryGeneration(2, Number.NaN), false);
  } finally {
    await server.close();
  }
});
