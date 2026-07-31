import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("conversion target helpers preserve explicit formats and safe extensions", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const helpers = await server.ssrLoadModule("/src/lib/convert-format.ts");
    const outputOptions = await server.ssrLoadModule("/src/lib/archive-output-options.ts");
    assert.equal(helpers.suggestedConvertTargetFormat("ZIP"), "7z");
    assert.equal(helpers.suggestedConvertTargetFormat("rar"), "zip");
    assert.equal(helpers.sourceMatchesConvertTarget("7Z", "7z"), true);
    assert.equal(helpers.sourceMatchesConvertTarget("tzst", "tar.zst"), true);
    assert.equal(helpers.sourceMatchesConvertTarget("zip", "sqz"), false);
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive", "tar.zst"),
      "/tmp/archive.tar.zst",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive.TZST", "tar.zst"),
      "/tmp/archive.TZST",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive.TAR.ZST", "tar.zst"),
      "/tmp/archive.TAR.ZST",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive", "wim"),
      "/tmp/archive.wim",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive", "wim", "swm"),
      "/tmp/archive.swm",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive.wim", "wim", "swm"),
      "/tmp/archive.wim.swm",
    );
    assert.equal(
      helpers.ensureConvertOutputExtension("/tmp/archive.SWM", "wim", "swm"),
      "/tmp/archive.SWM",
    );
    assert.equal(outputOptions.resolveSplitSizeBytes("none", "100", "mib"), null);
    assert.equal(outputOptions.resolveSplitSizeBytes("25-mib", "100", "mib"), 25 * 1024 ** 2);
    assert.equal(outputOptions.resolveSplitSizeBytes("4-gib", "100", "mib"), 0xffff_ffff);
    assert.equal(outputOptions.resolveSplitSizeBytes("custom", "4", "gib"), 4 * 1024 ** 3);
    assert.equal(outputOptions.resolveSplitSizeBytes("custom", "1.5", "gib"), 1.5 * 1024 ** 3);
    assert.equal(outputOptions.resolveSplitSizeBytes("custom", "0.05", "mib"), null);
  } finally {
    await server.close();
  }
});
test("conversion preflight checks the exact core budgets before review", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const [{ runConvertPreflight }, { ipc }] = await Promise.all([
      server.ssrLoadModule("/src/lib/convert-preflight.ts"),
      server.ssrLoadModule("/src/lib/ipc.ts"),
    ]);
    const plan = {
      input_count: 1,
      entries: 4,
      files: 3,
      directories: 1,
      symlinks: 0,
      total_bytes: 512,
      output_budget_bytes: 700,
      primary_output: "/dest/archive.7z.001",
      archive_output_budget_bytes: 700,
      final_output_budget_bytes: 700,
      split_volume_count_budget: 3,
      workspace_budget_bytes: 1_000,
      system_temp_budget_bytes: 200,
    };
    const original = {
      planConvert: ipc.planConvert,
      checkDiskSpace: ipc.checkDiskSpace,
      tempDir: ipc.tempDir,
    };
    const calls = [];
    const phases = [];
    const snapshots = [];
    ipc.planConvert = async (_spec, requestId) => {
      calls.push(["plan", requestId]);
      return plan;
    };
    ipc.tempDir = async () => "/system-temp";
    ipc.checkDiskSpace = async (pathValue, requiredBytes) => {
      calls.push(["space", pathValue, requiredBytes]);
      return {
        path: pathValue,
        required_bytes: requiredBytes,
        available_bytes: 10_000,
        ok: true,
      };
    };

    try {
      const outcome = await runConvertPreflight({
        spec: {
          kind: "convert",
          src: "/source/archive.zip",
          dest: "/dest/archive.7z",
          level: 6,
          src_encoding: null,
          src_password: null,
          dest_password: null,
          encrypt_names: false,
          split_size: 256,
          split_mode: "generic",
        },
        requestId: "convert-plan-1",
        destinationDirectory: "/dest",
        isCurrent: () => true,
        cancelRequested: () => false,
        onPlanRequestComplete: () => calls.push(["plan-complete"]),
        onPhase: (phase) => phases.push(phase),
        onPlan: (value) => snapshots.push(["plan", value]),
        onTempDisk: (value) => snapshots.push(["workspace", value]),
        onSystemTempDisk: (value) => snapshots.push(["temporary", value]),
        onDestinationDisk: (value) => snapshots.push(["destination", value]),
      });

      assert.equal(outcome.status, "ready");
      assert.deepEqual(phases, ["measuring", "checkingTemp", "checkingDest"]);
      assert.deepEqual(calls, [
        ["plan", "convert-plan-1"],
        ["plan-complete"],
        ["space", "/dest", 1_000],
        ["space", "/system-temp", 200],
        ["space", "/dest", 700],
      ]);
      assert.deepEqual(snapshots.map(([kind]) => kind), [
        "plan",
        "workspace",
        "temporary",
        "destination",
      ]);

      let diskCheckCalled = false;
      ipc.checkDiskSpace = async () => {
        diskCheckCalled = true;
        throw new Error("unexpected disk check");
      };
      const cancelled = await runConvertPreflight({
        spec: {
          kind: "convert",
          src: "/source/archive.zip",
          dest: "/dest/archive.7z",
          level: 6,
          src_encoding: null,
          src_password: null,
          dest_password: null,
          encrypt_names: false,
          split_size: null,
          split_mode: "generic",
        },
        requestId: "convert-plan-2",
        destinationDirectory: "/dest",
        isCurrent: () => true,
        cancelRequested: () => true,
        onPlanRequestComplete: () => {},
        onPhase: () => {},
        onPlan: () => {},
        onTempDisk: () => {},
        onSystemTempDisk: () => {},
        onDestinationDisk: () => {},
      });
      assert.equal(cancelled.status, "cancelled");
      assert.equal(diskCheckCalled, false);
    } finally {
      Object.assign(ipc, original);
    }
  } finally {
    await server.close();
  }
});

test("conversion copy stays complete in both locales", async () => {
  const [english, chinese] = await Promise.all([
    readFile(path.join(frontendRoot, "../locales/en-US.json"), "utf8").then(JSON.parse),
    readFile(path.join(frontendRoot, "../locales/zh-CN.json"), "utf8").then(JSON.parse),
  ]);
  const englishKeys = Object.keys(english)
    .filter((key) => key.startsWith("gui.convert."))
    .sort();
  const chineseKeys = Object.keys(chinese)
    .filter((key) => key.startsWith("gui.convert."))
    .sort();

  assert.deepEqual(englishKeys, chineseKeys);
  for (const key of [
    "gui.convert.preflight_status",
    "gui.convert.review.description",
    "gui.convert.review.cancelled",
    "gui.convert.not_enough_destination_space",
    "gui.convert.destination_recheck_cancelled",
  ]) {
    assert.notEqual(english[key], key);
    assert.notEqual(chinese[key], key);
  }
});
