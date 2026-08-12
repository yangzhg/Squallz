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

test("convert controller stays behind the workspace loader boundary", async () => {
  const [host, route, session, app] = await Promise.all([
    readFile(path.join(frontendRoot, "src/components/ArchiveOperationWorkspaceHost.svelte"), "utf8"),
    readFile(path.join(frontendRoot, "src/components/ConvertWorkspaceRoute.svelte"), "utf8"),
    readFile(path.join(frontendRoot, "src/lib/convert-session.svelte.ts"), "utf8"),
    readFile(path.join(frontendRoot, "src/App.svelte"), "utf8"),
  ]);

  assert.match(host, /import\("\.\/ConvertWorkspaceRoute\.svelte"\)/);
  assert.doesNotMatch(host, /import\("\.\/ConvertWorkspace\.svelte"\)/);
  assert.match(route, /convertSessionFor\(owner, bridge\)/);
  assert.match(session, /new WeakMap<ConvertRouteOwner, ConvertSession>/);
  assert.doesNotMatch(app, /from "\.\/lib\/convert-session\.svelte"/);
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

test("convert lazy sessions isolate roots and preserve only non-sensitive drafts", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const [{ convertSessionFor }, { ipc }] = await Promise.all([
      server.ssrLoadModule("/src/lib/convert-session.svelte.ts"),
      server.ssrLoadModule("/src/lib/ipc.ts"),
    ]);
    const archive = {
      id: 7,
      path: "/source/photos.zip",
      source: "/source/photos.zip",
      name: "photos.zip",
      read_only: false,
      format: "zip",
      entry_count: 3,
      volumes: null,
      legacy_encoding_count: 0,
      garbled_count: 0,
      suggested_encoding: null,
      encoding_override: null,
    };
    const nextArchive = {
      ...archive,
      id: 8,
      path: "/source/next.7z",
      source: "/source/next.7z",
      name: "next.7z",
      format: "7z",
    };
    let currentArchive = archive;
    const notices = [];
    const operations = [];
    let submitStarted = false;
    let submitCalls = 0;
    let saveCalls = 0;
    let confirmResult = true;
    let finishSubmit;
    const submitPending = new Promise((resolve) => {
      finishSubmit = resolve;
    });
    let submitBehavior = () => submitPending;
    const bridge = {
      getArchive: () => currentArchive,
      tr: (_key, fallback) => fallback,
      tError: () => "translated error",
      showNotice: (message) => notices.push(message),
      ensurePreflightListener: async () => {},
      getDialogModule: async () => ({
        confirm: async () => confirmResult,
        save: async () => "/dest/photos.7z",
      }),
      saveNativeDialog: async () => {
        saveCalls += 1;
        return "/dest/photos.7z";
      },
      submitJob: async () => {
        submitCalls += 1;
        submitStarted = true;
        await submitBehavior();
        return 41;
      },
      focusBlockingTaskIfAny: () => false,
      isJobSubmitBlocked: () => false,
      jobSubmitBlockedMessage: () => "blocked",
      recordQueuedOperation: (title, detail) => operations.push([title, detail]),
      archiveStemName: (name) => name.replace(/\.[^.]+$/, ""),
      platform: () => "macos",
      prepareSubmitFocus: () => {},
      shouldRestorePrimaryFocus: () => false,
      register: () => {},
    };
    const original = {
      inspectCreateDestination: ipc.inspectCreateDestination,
      cancelCreateDestinationInspection: ipc.cancelCreateDestinationInspection,
      planConvert: ipc.planConvert,
      checkDiskSpace: ipc.checkDiskSpace,
      tempDir: ipc.tempDir,
    };
    ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
    ipc.planConvert = async () => ({
      input_count: 1,
      entries: 3,
      deduplicated_entries: 0,
      files: 3,
      directories: 0,
      symlinks: 0,
      total_bytes: 512,
      output_budget_bytes: 700,
      primary_output: "/dest/photos.7z.001",
      archive_output_budget_bytes: 700,
      final_output_budget_bytes: 700,
      split_volume_count_budget: 2,
      workspace_budget_bytes: 1_000,
      system_temp_budget_bytes: 0,
    });
    ipc.checkDiskSpace = async (pathValue, requiredBytes) => ({
      path: pathValue,
      required_bytes: requiredBytes,
      available_bytes: 10_000,
      ok: true,
    });

    const waitFor = async (predicate) => {
      for (let attempt = 0; attempt < 100; attempt += 1) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
      const preflight = activeSession?.surface("modern").preflight;
      assert.fail(`timed out waiting for convert session state: ${JSON.stringify({ preflight, notices })}`);
    };

    let activeSession = null;

    try {
      const owner = {};
      const session = convertSessionFor(owner, bridge);
      activeSession = session;
      assert.equal(convertSessionFor(owner, bridge), session);
      session.syncArchive(archive);
      let modern = session.surface("modern");
      modern.formats.find((choice) => choice.id === "7z").onSelect();
      modern.profiles.find((choice) => choice.id === "maximum").onSelect();
      modern.protection.onPasswordInput("secret");
      modern.protection.onPasswordConfirmationInput("secret");
      modern.protection.onSplitPresetChange("100-mib");

      const classic = session.surface("classic");
      assert.equal(classic.formats.find((choice) => choice.id === "7z").selected, true);
      assert.equal(classic.profiles.find((choice) => choice.id === "maximum").selected, true);
      assert.equal(classic.protection.password, "secret");
      assert.equal(classic.protection.splitPreset, "100-mib");

      classic.start.onSelect();
      await waitFor(() => session.surface("modern").review !== null);
      session.surface("modern").review.onConfirm();
      await waitFor(() => submitStarted);
      currentArchive = nextArchive;
      session.syncArchive(nextArchive);
      const switchedDuringSubmit = session.surface("modern");
      assert.equal(switchedDuringSubmit.preflight.phase, "submitting");
      assert.equal(switchedDuringSubmit.source.path, archive.path);
      assert.equal(switchedDuringSubmit.formats.find((choice) => choice.id === "7z").selected, true);
      assert.equal(switchedDuringSubmit.protection.splitPreset, "100-mib");
      assert.equal(switchedDuringSubmit.protection.password, "secret");
      assert.equal(operations.length, 0);
      assert.equal(session.canLeave(), false);
      assert.match(notices.at(-1), /finishes adding this conversion/);
      finishSubmit();
      await waitFor(() => session.surface("modern").source.path === nextArchive.path);
      assert.equal(session.surface("modern").preflight.phase, "idle");
      assert.equal(operations.length, 1);
      assert.match(operations[0][1], /photos\.zip/);
      assert.equal(submitCalls, 1);
      assert.equal(session.surface("modern").protection.password, "");
      assert.equal(session.surface("modern").formats.find((choice) => choice.id === "zip").selected, true);

      currentArchive = archive;
      session.syncArchive(archive);
      session.surface("modern").protection.onPasswordInput("failed-secret");
      session.surface("modern").protection.onPasswordConfirmationInput("failed-secret");
      session.surface("modern").start.onSelect();
      await waitFor(() => session.surface("modern").review !== null);
      let rejectSubmit;
      submitBehavior = () => new Promise((_resolve, reject) => {
        rejectSubmit = reject;
      });
      submitStarted = false;
      session.surface("modern").review.onConfirm();
      await waitFor(() => submitStarted && typeof rejectSubmit === "function");
      currentArchive = nextArchive;
      session.syncArchive(nextArchive);
      assert.equal(session.surface("modern").source.path, archive.path);
      rejectSubmit(new Error("submission unavailable"));
      await waitFor(() => session.surface("modern").source.path === nextArchive.path);
      const afterFailedSubmitSwitch = session.surface("modern");
      assert.equal(afterFailedSubmitSwitch.preflight.phase, "idle");
      assert.equal(afterFailedSubmitSwitch.review, null);
      assert.equal(afterFailedSubmitSwitch.protection.password, "");
      assert.equal(afterFailedSubmitSwitch.formats.find((choice) => choice.id === "zip").selected, true);
      assert.equal(operations.length, 1);
      assert.equal(submitCalls, 2);
      submitBehavior = async () => {};

      currentArchive = archive;
      session.syncArchive(archive);
      session.surface("modern").protection.onSplitPresetChange("100-mib");

      let destinationChecks = 0;
      confirmResult = false;
      ipc.inspectCreateDestination = async () => {
        destinationChecks += 1;
        return destinationChecks === 1
          ? { conflict: false, guard: null }
          : { conflict: true, guard: "changed-output" };
      };
      modern = session.surface("modern");
      modern.protection.onPasswordInput("discarded");
      modern.protection.onPasswordConfirmationInput("discarded");
      modern.start.onSelect();
      await waitFor(() => session.surface("modern").review !== null);
      session.surface("modern").review.onConfirm();
      await waitFor(() => session.surface("modern").preflight.phase === "reviewing");
      assert.equal(submitCalls, 2);
      assert.notEqual(session.surface("modern").review, null);
      assert.equal(session.surface("modern").protection.password, "discarded");

      ipc.inspectCreateDestination = async () => {
        throw { key: "error.cancelled", params: {}, detail: "" };
      };
      session.surface("modern").review.onConfirm();
      await waitFor(() => session.surface("modern").preflight.issueStage === "destination");
      assert.equal(session.surface("modern").preflight.phase, "reviewing");
      assert.notEqual(session.surface("modern").review, null);
      assert.equal(session.surface("modern").protection.password, "discarded");
      session.surface("modern").review.onCancel();
      assert.equal(session.surface("modern").review, null);
      assert.equal(session.surface("modern").protection.password, "");
      confirmResult = true;
      ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });

      modern = session.surface("modern");
      modern.protection.onPasswordInput("temporary");
      modern.protection.onPasswordConfirmationInput("temporary");
      session.leave();
      const returned = session.surface("classic");
      assert.equal(returned.protection.password, "");
      assert.equal(returned.protection.splitPreset, "100-mib");
      assert.equal(returned.formats.find((choice) => choice.id === "7z").selected, true);
      assert.equal(returned.profiles.find((choice) => choice.id === "maximum").selected, true);

      const other = convertSessionFor({}, bridge);
      other.syncArchive(archive);
      assert.notEqual(other, session);
      assert.equal(other.surface("modern").protection.password, "");

      let activeRequestId = null;
      let resolveInspection;
      const inspectionPending = new Promise((resolve) => {
        resolveInspection = resolve;
      });
      const cancelledRequests = [];
      ipc.inspectCreateDestination = async (_path, _split, requestId) => {
        activeRequestId = requestId;
        return inspectionPending;
      };
      ipc.cancelCreateDestinationInspection = async (requestId) => {
        cancelledRequests.push(requestId);
      };
      other.surface("modern").protection.onPasswordInput("cancelled-secret");
      other.surface("modern").protection.onPasswordConfirmationInput("cancelled-secret");
      other.surface("modern").start.onSelect();
      await waitFor(() => activeRequestId !== null);
      const cancelledRequestId = activeRequestId;
      assert.equal(other.applyPreflightEvent({
        request_id: "stale-request",
        phase: "destination",
        current: "/stale/output",
      }), false);
      assert.equal(other.applyPreflightEvent({
        request_id: activeRequestId,
        phase: "destination",
        current: "/dest/current-output",
      }), true);
      assert.equal(other.surface("modern").preflight.current, "/dest/current-output");
      other.surface("modern").preflight.onCancel();
      assert.deepEqual(cancelledRequests, [cancelledRequestId]);
      assert.equal(other.surface("modern").protection.password, "cancelled-secret");
      resolveInspection({ conflict: false, guard: null });
      await waitFor(() => other.surface("modern").preflight.phase === "cancelled");
      assert.equal(other.surface("modern").protection.password, "");
      assert.equal(other.applyPreflightEvent({
        request_id: activeRequestId,
        phase: "destination",
        current: "/late/output",
      }), false);

      activeRequestId = null;
      let resolveFailedCancelInspection;
      const failedCancelInspection = new Promise((resolve) => {
        resolveFailedCancelInspection = resolve;
      });
      ipc.inspectCreateDestination = async (_path, _split, requestId) => {
        activeRequestId = requestId;
        return failedCancelInspection;
      };
      ipc.cancelCreateDestinationInspection = async () => {
        throw new Error("cancel unavailable");
      };
      const cancelFailureSession = convertSessionFor({}, bridge);
      cancelFailureSession.syncArchive(archive);
      cancelFailureSession.surface("modern").protection.onPasswordInput("clear-on-cancel");
      cancelFailureSession.surface("modern").protection.onPasswordConfirmationInput("clear-on-cancel");
      const saveCallsBeforeCancelFailure = saveCalls;
      cancelFailureSession.surface("modern").start.onSelect();
      await waitFor(() => activeRequestId !== null);
      assert.equal(saveCalls, saveCallsBeforeCancelFailure + 1);
      cancelFailureSession.applyPreflightEvent({
        request_id: activeRequestId,
        phase: "destination",
        current: "/dest/still-checking",
      });
      cancelFailureSession.surface("modern").preflight.onCancel();
      await waitFor(() => cancelFailureSession.surface("modern").preflight.cancelPending === false);
      const afterCancelFailure = cancelFailureSession.surface("modern");
      assert.equal(afterCancelFailure.preflight.phase, "choosingDest");
      assert.equal(afterCancelFailure.preflight.requestKind, "destination");
      assert.equal(afterCancelFailure.preflight.current, "/dest/still-checking");
      assert.equal(afterCancelFailure.preflight.cancellable, true);
      assert.equal(afterCancelFailure.protection.password, "clear-on-cancel");
      assert.equal(cancelFailureSession.applyPreflightEvent({
        request_id: activeRequestId,
        phase: "destination",
        current: "/dest/continuing-after-cancel-failure",
      }), true);
      afterCancelFailure.start.onSelect();
      await new Promise((resolve) => setTimeout(resolve, 0));
      assert.equal(saveCalls, saveCallsBeforeCancelFailure + 1);
      resolveFailedCancelInspection({ conflict: false, guard: null });
      await waitFor(() => cancelFailureSession.surface("modern").review !== null);
      assert.equal(cancelFailureSession.surface("modern").protection.password, "clear-on-cancel");
      cancelFailureSession.surface("modern").review.onCancel();
      assert.equal(cancelFailureSession.surface("modern").protection.password, "");

      activeRequestId = null;
      const secondInspection = new Promise((resolve) => {
        resolveInspection = resolve;
      });
      ipc.inspectCreateDestination = async (_path, _split, requestId) => {
        activeRequestId = requestId;
        return secondInspection;
      };
      ipc.cancelCreateDestinationInspection = async (requestId) => {
        cancelledRequests.push(requestId);
      };
      other.surface("modern").start.onSelect();
      await waitFor(() => activeRequestId !== null);
      const leavingRequestId = activeRequestId;
      other.leave();
      assert.deepEqual(cancelledRequests, [cancelledRequestId, leavingRequestId]);
      resolveInspection({ conflict: false, guard: null });
      await new Promise((resolve) => setTimeout(resolve, 0));

      currentArchive = nextArchive;
      session.syncArchive(nextArchive);
      const changed = session.surface("modern");
      assert.equal(changed.formats.find((choice) => choice.id === "zip").selected, true);
      assert.equal(changed.protection.splitPreset, "none");
      session.dispose();
      assert.notEqual(convertSessionFor(owner, bridge), session);
    } finally {
      Object.assign(ipc, original);
    }
  } finally {
    await server.close();
  }
});

test("disposed convert sessions ignore late submission failures", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const [{ convertSessionFor }, { ipc }] = await Promise.all([
      server.ssrLoadModule("/src/lib/convert-session.svelte.ts"),
      server.ssrLoadModule("/src/lib/ipc.ts"),
    ]);
    const archive = {
      id: 17,
      path: "/source/stale.zip",
      source: "/source/stale.zip",
      name: "stale.zip",
      read_only: false,
      format: "zip",
      entry_count: 1,
      volumes: null,
      legacy_encoding_count: 0,
      garbled_count: 0,
      suggested_encoding: null,
      encoding_override: null,
    };
    const notices = [];
    const operations = [];
    let submitStarted = false;
    let rejectSubmit;
    const submitPending = new Promise((_resolve, reject) => {
      rejectSubmit = reject;
    });
    const bridge = {
      getArchive: () => archive,
      tr: (_key, fallback) => fallback,
      tError: () => "translated error",
      showNotice: (message) => notices.push(message),
      ensurePreflightListener: async () => {},
      getDialogModule: async () => ({
        confirm: async () => true,
        save: async () => "/dest/stale.7z",
      }),
      saveNativeDialog: async () => "/dest/stale.7z",
      submitJob: async () => {
        submitStarted = true;
        return submitPending;
      },
      focusBlockingTaskIfAny: () => false,
      isJobSubmitBlocked: () => false,
      jobSubmitBlockedMessage: () => "blocked",
      recordQueuedOperation: (title, detail) => operations.push([title, detail]),
      archiveStemName: (name) => name.replace(/\.[^.]+$/, ""),
      platform: () => "macos",
      prepareSubmitFocus: () => {},
      shouldRestorePrimaryFocus: () => false,
      register: () => {},
    };
    const original = {
      inspectCreateDestination: ipc.inspectCreateDestination,
      cancelCreateDestinationInspection: ipc.cancelCreateDestinationInspection,
      planConvert: ipc.planConvert,
      checkDiskSpace: ipc.checkDiskSpace,
    };
    ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
    ipc.planConvert = async () => ({
      input_count: 1,
      entries: 1,
      deduplicated_entries: 0,
      files: 1,
      directories: 0,
      symlinks: 0,
      total_bytes: 64,
      output_budget_bytes: 96,
      primary_output: "/dest/stale.7z",
      archive_output_budget_bytes: 96,
      final_output_budget_bytes: 96,
      split_volume_count_budget: 1,
      workspace_budget_bytes: 128,
      system_temp_budget_bytes: 0,
    });
    ipc.checkDiskSpace = async (pathValue, requiredBytes) => ({
      path: pathValue,
      required_bytes: requiredBytes,
      available_bytes: 10_000,
      ok: true,
    });

    const waitFor = async (predicate) => {
      for (let attempt = 0; attempt < 100; attempt += 1) {
        if (predicate()) return;
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
      assert.fail(`timed out waiting for disposed convert session: ${JSON.stringify({ notices })}`);
    };

    try {
      const owner = {};
      const session = convertSessionFor(owner, bridge);
      session.syncArchive(archive);
      session.surface("modern").start.onSelect();
      await waitFor(() => session.surface("modern").review !== null);
      session.surface("modern").review.onConfirm();
      await waitFor(() => submitStarted);
      session.dispose();
      const noticeCountAtDispose = notices.length;
      rejectSubmit(new Error("late submission failure"));
      await new Promise((resolve) => setTimeout(resolve, 0));

      const disposed = session.surface("modern");
      assert.equal(disposed.preflight.phase, "idle");
      assert.equal(disposed.preflight.issue, "");
      assert.equal(disposed.review, null);
      assert.equal(notices.length, noticeCountAtDispose);
      assert.equal(operations.length, 0);
      assert.notEqual(convertSessionFor(owner, bridge), session);

      let recheckCalls = 0;
      let recheckStarted = false;
      let rejectRecheck;
      ipc.inspectCreateDestination = async () => {
        recheckCalls += 1;
        if (recheckCalls === 1) return { conflict: false, guard: null };
        recheckStarted = true;
        return new Promise((_resolve, reject) => {
          rejectRecheck = reject;
        });
      };
      ipc.cancelCreateDestinationInspection = async () => {};
      submitStarted = false;
      const recheckOwner = {};
      const recheckSession = convertSessionFor(recheckOwner, bridge);
      recheckSession.syncArchive(archive);
      recheckSession.surface("modern").start.onSelect();
      await waitFor(() => recheckSession.surface("modern").review !== null);
      recheckSession.surface("modern").review.onConfirm();
      await waitFor(() => recheckStarted && typeof rejectRecheck === "function");
      recheckSession.dispose();
      const noticeCountAtRecheckDispose = notices.length;
      rejectRecheck(new Error("late destination recheck failure"));
      await new Promise((resolve) => setTimeout(resolve, 0));

      const disposedDuringRecheck = recheckSession.surface("modern");
      assert.equal(disposedDuringRecheck.preflight.phase, "idle");
      assert.equal(disposedDuringRecheck.preflight.issue, "");
      assert.equal(disposedDuringRecheck.review, null);
      assert.equal(notices.length, noticeCountAtRecheckDispose);
      assert.equal(operations.length, 0);
      assert.equal(submitStarted, false);
      assert.notEqual(convertSessionFor(recheckOwner, bridge), recheckSession);
    } finally {
      Object.assign(ipc, original);
    }
  } finally {
    await server.close();
  }
});
