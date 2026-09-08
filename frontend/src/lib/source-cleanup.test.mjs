import assert from "node:assert/strict";
import test from "node:test";

import { createTestServer } from "../../tests/runtime.mjs";

test("only incomplete compress cleanup results request a recovery refresh", async () => {
  const server = await createTestServer();

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
