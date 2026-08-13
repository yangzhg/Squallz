import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("recovery results gate repair and expose one semantic tone", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const {
      latestMatchingRecoveryTask,
      readRecoveryProtectionResult,
      recoveryRouteForOpen,
      recoveryRepairGate,
      recoveryResultConfirmsRepairCapacity,
      recoveryResultHasNoDamage,
      recoveryResultTone,
    } = await server.ssrLoadModule("/src/lib/recovery-result.ts");

    const staleRoute = {
      sourceMode: "selected",
      sourceOverride: "/tmp/older.zip",
      par2Override: "/tmp/older.zip.par2",
    };
    assert.deepEqual(recoveryRouteForOpen("current", true, staleRoute), {
      sourceMode: "current",
      sourceOverride: null,
      par2Override: null,
    });
    assert.deepEqual(recoveryRouteForOpen("preserve", true, staleRoute), staleRoute);
    assert.deepEqual(
      recoveryRouteForOpen("preserve", true, {
        sourceMode: "none",
        sourceOverride: "/tmp/stale.zip",
        par2Override: null,
      }),
      { sourceMode: "current", sourceOverride: null, par2Override: null },
    );

    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: ArchiveStructureWarning } = await server.ssrLoadModule(
      "/src/components/ArchiveStructureWarning.svelte",
    );
    const { body: structureWarning } = render(ArchiveStructureWarning, {
      props: {
        message: "The ZIP index is damaged.",
        actionLabel: "Open ZIP repair",
        onRepair: () => {},
      },
    });
    assert.match(structureWarning, /data-archive-structure-warning/);
    assert.match(structureWarning, /role="status"/);
    assert.match(structureWarning, /The ZIP index is damaged\./);
    assert.match(structureWarning, /data-archive-structure-repair/);
    assert.match(structureWarning, />Open ZIP repair<\/button>/);

    assert.equal(recoveryRepairGate(null), "verify_first");
    assert.equal(recoveryResultTone(null), "neutral");

    assert.deepEqual(
      readRecoveryProtectionResult(
        {
          recovery: "/Users/alex/Backups/archive.zip.par2",
          outputs: [
            "/Users/alex/Backups/archive.zip.vol00+01.par2",
            "/Users/alex/Backups/archive.zip.par2",
            "/Users/alex/Backups/archive.zip.vol00+01.par2",
          ],
        },
        "/Users/alex/Backups/fallback.par2",
      ),
      {
        primaryOutput: "/Users/alex/Backups/archive.zip.par2",
        outputs: [
          "/Users/alex/Backups/archive.zip.par2",
          "/Users/alex/Backups/archive.zip.vol00+01.par2",
        ],
      },
    );
    assert.deepEqual(
      readRecoveryProtectionResult(null, "/Users/alex/Backups/archive.zip.par2"),
      {
        primaryOutput: "/Users/alex/Backups/archive.zip.par2",
        outputs: ["/Users/alex/Backups/archive.zip.par2"],
      },
    );

    const repairable = {
      operation: "verify",
      ok: false,
      metrics: {
        all_correct: false,
        no_damage: false,
        repair_possible: true,
        blocks_needed: 3,
        recovery_blocks_available: 8,
      },
    };
    assert.equal(recoveryRepairGate(repairable), null);
    assert.equal(recoveryResultConfirmsRepairCapacity(repairable), true);
    assert.equal(recoveryResultTone(repairable), "warning");

    const overCapacity = {
      ...repairable,
      metrics: {
        ...repairable.metrics,
        repair_possible: false,
        blocks_needed: 12,
        recovery_blocks_available: 4,
      },
    };
    assert.equal(recoveryRepairGate(overCapacity), "over_capacity");
    assert.equal(recoveryResultConfirmsRepairCapacity(overCapacity), false);
    assert.equal(recoveryResultTone(overCapacity), "danger");

    for (const metrics of [{ no_damage: true }, { all_correct: true }]) {
      const result = { operation: "verify", ok: true, metrics };
      assert.equal(recoveryResultHasNoDamage(result), true);
      assert.equal(recoveryRepairGate(result), "no_damage");
      assert.equal(recoveryResultConfirmsRepairCapacity(result), false);
      assert.equal(recoveryResultTone(result), "success");
    }

    const successfulWithoutCounts = { operation: "verify", ok: true };
    assert.equal(recoveryResultHasNoDamage(successfulWithoutCounts), true);
    assert.equal(recoveryRepairGate(successfulWithoutCounts), "no_damage");
    assert.equal(recoveryResultConfirmsRepairCapacity(successfulWithoutCounts), false);
    assert.equal(recoveryResultTone(successfulWithoutCounts), "success");

    const failedWithoutCounts = { operation: "verify", ok: false };
    assert.equal(recoveryResultHasNoDamage(failedWithoutCounts), false);
    assert.equal(recoveryRepairGate(failedWithoutCounts), null);
    assert.equal(recoveryResultConfirmsRepairCapacity(failedWithoutCounts), false);
    assert.equal(recoveryResultTone(failedWithoutCounts), "warning");

    const oldResult = { id: 1, result: repairable };
    const latestRunning = { id: 2, result: null };
    assert.equal(
      latestMatchingRecoveryTask([oldResult, latestRunning], () => true),
      latestRunning,
    );
  } finally {
    await server.close();
  }
});
