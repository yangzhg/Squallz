import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const taskModel = await server.ssrLoadModule("/src/lib/task-model.ts");
const { taskResultAvailableForSurface } = await server.ssrLoadModule("/src/lib/task-dialog.ts");
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");

function reportHarness({ navigation = false } = {}) {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const names = ["selectedChecksumTask", "checksumItems", "checksumResultText", "checksumResultNumber",
    "viewTaskResults", "checksumCopyFeedbackFor", "checksumCopyFeedbackToneFor",
    "submitChecksumJob", "submitChecksumCheckJob", "checksumWorkspaceSurface",
    "checksumAlgorithmLabel", "checksumAlgorithmHint", "checksumItemNumber",
    "preventCreateSubmissionNavigation", "preventTaskWorkspaceNavigation", "dismissRecoveryPicker",
    ...(navigation ? ["setScreen", "preventConvertSubmissionNavigation", "dismissTaskDialog",
      "closeTaskCenter", "cancelTaskReview", "adoptRecoveryTargetFromTask",
      "focusChecksumResultPanel", "focusDuplicateReportPanel"] : [])];
  const functions = names.map((name) => {
    const declaration = source.statements.find((node) =>
      ts.isFunctionDeclaration(node) && node.name?.text === name);
    assert.ok(declaration, name);
    return declaration.getText(source);
  }).join("\n");
  const calls = [];
  const panel = (name) => ({ scrollIntoView() {}, focus: () => calls.push(["focus", name]) });
  const context = {
    ...taskModel,
    jobRows: [], checksumReportTaskIds: {}, taskWindowMode: false,
    checksumCopyFeedbackKind: "checksum", checksumCopyFeedbackTaskId: 1,
    checksumCopyFeedbackMessage: "Copied", checksumCopyFeedbackTone: "success",
    checksumManifestPath: "/new/SHA256SUMS", checksumAlgorithm: "sha256",
    checksumAlgorithms: ["sha256", "sha512"], checksumExcludeText: "",
    toolsArchiveReturnSurface: () => ({ visible: false }),
    checksumCurrentArchiveDisabledReason: () => "", checksumTargetName: () => "New selection",
    checksumTargetLabel: () => "/new/files", checksumManifestLabel: () => "/new/SHA256SUMS",
    checksumItemText: (item, key) => item[key] ?? "", checksumItemStatus: () => "OK",
    taskStateLabel: (state) => state ?? "Pending", formatBytes: (value) => `${value} B`,
    taskOutcomeNeedsAttention: (task) => task.result?.ok === false,
    selectChecksumAlgorithm() {}, copyChecksumResults() {}, chooseChecksumFile() {},
    chooseChecksumFolder() {}, useCurrentArchiveForChecksum() {}, chooseChecksumManifest() {},
    setScreen() {}, dismissTaskDialog() {}, focusChecksumResultPanel() {},
    checksumResultLine: (_kind, item) => `${item.digest ?? item.actual}  ${item.path}`,
    checksumTarget: () => "/new/files", checksumExcludeRules: () => [],
    focusBlockingTaskIfAny: () => false, showNotice: (message) => calls.push(["notice", message]), recordOperation() {},
    tr: (_key, fallback) => fallback, pathBaseName: (path) => path.split("/").at(-1),
    submitJob: async (spec) => {
      calls.push(["submit", spec.kind]);
      context.jobRows.push({ id: 20, spec, result: null, state: "queued" });
      return 20;
    },
    screen: "checksum", createPreflightPhase: "idle", convertRouteHandle: null,
    archiveOpenStatus: "idle", archivePasswordPrompt: null, previewPasswordPrompt: null,
    taskReviewRequestGeneration: 0, pendingTaskReviewId: null,
    taskCenterOpen: false, taskCenterSelectedTaskId: null, taskCenterFocusTaskId: null,
    taskDialogTaskId: null, taskDialogDismissedId: null,
    archiveUpdateReview: { cancelSourceChoice() {} }, nestedExtractDraftGeneration: 0,
    nestedExtractPickerRequest: 0, batchPickerRequest: 0, pendingArchiveTaskReview: null,
    pendingCreateSubmission: null, dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {},
    restoreTaskWorkspaceFocus: () => calls.push(["restore-focus"]),
    currentArchive: null, sameFilePath: (left, right) => left === right,
    recoverySourceMode: "selected", recoverySourceOverride: "/previous/recovery.zip",
    recoveryPar2Override: "/previous/recovery.par2", duplicateReportTaskId: 9,
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0,
    duplicateReportFocusPending: false, createPrimaryFocusPending: true,
    extractReviewFocusPending: true, convertReviewFocusPending: true,
    securitySettingsFocusPending: true, archiveUpdateReviewFocusPending: true,
    nestedExtractReviewFocusPending: true, batchReviewFocusPending: true,
    checksumResultPanel: panel("checksum"), checksumCheckResultPanel: panel("checksum_check"),
    duplicateReportPanel: panel("duplicates"), blockingModalVisible: () => false,
    tick: async () => {}, document: { documentElement: {}, body: {}, querySelectorAll: () => [] },
    setTaskExpanded: (id, expanded) => {
      calls.push(["expand", id, expanded]);
      const task = context.jobRows.find((row) => row.id === id);
      if (task) task.expanded = expanded;
    },
  };
  const { outputText } = ts.transpileModule(functions, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  context.calls = calls;
  return vm.runInNewContext(`${outputText}\n({${names.join(",")}, context:globalThis, calls})`, context);
}

function checksumTask(id, kind = "checksum") {
  return { id, state: "done", expanded: true,
    spec: kind === "checksum"
      ? { kind, inputs: [`/run-${id}`], algorithm: "sha256", excludes: [] }
      : { kind, manifest: `/run-${id}/SHA256SUMS`, algorithm: "sha256" },
    result: { files_hashed: id, checked: id,
      items: [{ path: `/run-${id}/report.txt`, digest: `digest-${id}`, actual: `actual-${id}` }] },
  };
}

test("checksum report navigation and copying stay bound to the selected task", async (t) => {
  const harness = reportHarness();
  const { context } = harness;
  context.jobRows.push(checksumTask(1), checksumTask(2), checksumTask(3, "checksum_check"));
  harness.viewTaskResults(context.jobRows[0]);
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  assert.equal(harness.checksumResultNumber("checksum", "files_hashed"), 1);
  assert.equal(harness.checksumResultText("checksum"), "digest-1  /run-1/report.txt");
  context.jobRows.push(checksumTask(4));
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  harness.viewTaskResults(context.jobRows[2]);
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 3);
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  assert.equal(harness.checksumCopyFeedbackFor("checksum"), "Copied");
  harness.viewTaskResults(context.jobRows[1]);
  assert.equal(harness.checksumCopyFeedbackFor("checksum"), null);
  assert.equal(harness.checksumCopyFeedbackToneFor("checksum"), null);
  await harness.submitChecksumJob();
  assert.equal(harness.selectedChecksumTask("checksum").id, 20);
  assert.equal(harness.checksumResultText("checksum"), "");
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 3);

  const run = reportHarness({ navigation: true });
  const archive = { id: 1, path: "/current/photos.zip", source: "/current/photos.zip", name: "photos.zip",
    format: "zip", entry_count: 3, garbled_count: 0, non_utf8_name_count: 0, encoding_override: null };
  run.context.currentArchive = archive;
  run.context.screen = "convert";
  run.context.checksumReportTaskIds = { checksum: 1, checksum_check: 3 };
  const targets = [checksumTask(2), checksumTask(5, "checksum_check"),
    { id: 10, state: "done", expanded: false, spec: { kind: "duplicate_scan", inputs: ["/scan-10"] },
      result: { duplicate_groups: 1, groups: [] } },
    { id: 11, state: "done", expanded: false,
      spec: { kind: "repair_recovery", path: "/original/recovery.zip", recovery: "/original/recovery.par2" }, result: { ok: true } }];
  run.context.jobRows.push(checksumTask(1), checksumTask(3, "checksum_check"),
    { id: 9, state: "done", spec: { kind: "duplicate_scan", inputs: ["/previous/scan"] },
      result: { duplicate_groups: 2, groups: [] } }, ...targets);
  const original = { inspectCreateDestination: ipc.inspectCreateDestination,
    cancelCreateDestinationInspection: ipc.cancelCreateDestinationInspection, planConvert: ipc.planConvert,
    checkDiskSpace: ipc.checkDiskSpace, tempDir: ipc.tempDir };
  let checks = 0;
  let releaseInspection;
  ipc.inspectCreateDestination = async () => ++checks === 2
    ? new Promise((resolve) => { releaseInspection = resolve; })
    : { conflict: false, guard: null };
  ipc.cancelCreateDestinationInspection = async () => run.calls.push(["cancel-convert"]);
  ipc.planConvert = async (spec) => ({ input_count: 1, entries: 3, files: 3, directories: 0,
    symlinks: 0, total_bytes: 512, primary_output: spec.dest, output_budget_bytes: 700,
    archive_output_budget_bytes: 700, final_output_budget_bytes: 700, workspace_budget_bytes: 1400,
    system_temp_budget_bytes: 0, split_volume_count_budget: 1 });
  ipc.checkDiskSpace = async (path, required_bytes) => ({ path, required_bytes, available_bytes: 10000, ok: true });
  ipc.tempDir = async () => "/temporary";
  const session = convertSessionFor({}, {
    getArchive: () => archive, tr: run.context.tr, tError: () => "Read/write error", showNotice: run.context.showNotice,
    ensurePreflightListener: async () => {}, getDialogModule: async () => ({ save() {}, confirm: async () => true }),
    saveNativeDialog: async () => "/output/photos.7z",
    submitJob: async () => { run.calls.push(["convert-submit"]); return 42; },
    focusBlockingTaskIfAny: () => false, isJobSubmitBlocked: () => false, jobSubmitBlockedMessage: () => "Busy",
    recordQueuedOperation() {}, archiveStemName: (name) => name.replace(/\.[^.]+$/, ""), platform: () => "macos",
    prepareSubmitFocus() {}, shouldRestorePrimaryFocus: () => false, register() {},
  });
  const leave = session.leave.bind(session);
  session.leave = () => { run.calls.push(["leave-convert"]); leave(); };
  t.after(() => {
    session.dispose();
    releaseInspection?.({ conflict: false, guard: null });
    Object.assign(ipc, original);
  });
  run.context.convertRouteHandle = session;
  session.syncArchive(archive);
  const waitFor = async (predicate) => {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (predicate()) return;
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    assert.fail("Conversion did not reach the expected state");
  };
  session.surface("modern").start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  session.surface("modern").review.onConfirm();
  await waitFor(() => Boolean(releaseInspection));
  assert.equal(session.surface("modern").preflight.phase, "submitting");

  const plain = (value) => JSON.parse(JSON.stringify(value));
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  const selectDetails = (task) => Object.assign(run.context, {
    taskCenterOpen: true, taskCenterSelectedTaskId: task.id, taskCenterFocusTaskId: task.id,
    taskDialogTaskId: task.id, taskDialogDismissedId: null, pendingTaskReviewId: task.id,
  });
  const workspace = () => plain(Object.fromEntries([
    "screen", "checksumReportTaskIds", "duplicateReportTaskId", "recoverySourceMode",
    "recoverySourceOverride", "recoveryPar2Override", "taskCenterOpen", "taskCenterSelectedTaskId",
    "taskCenterFocusTaskId", "taskDialogTaskId", "taskDialogDismissedId", "pendingTaskReviewId",
    "taskReviewRequestGeneration", "duplicateReportFocusPending", "createPrimaryFocusPending",
    "extractReviewFocusPending", "convertReviewFocusPending", "securitySettingsFocusPending",
    "archiveUpdateReviewFocusPending", "nestedExtractReviewFocusPending", "batchReviewFocusPending",
  ].map((key) => [key, run.context[key]])));
  const conversion = plain(session.surface("modern"));
  for (const task of targets) {
    selectDetails(task);
    const before = workspace();
    const callCount = run.calls.length;
    run.viewTaskResults(task);
    await settle();
    assert.deepEqual(workspace(), before, `${task.spec.kind} must retain reports, sources and task details when navigation is blocked`);
    assert.deepEqual(plain(session.surface("modern")), conversion);
    assert.deepEqual(run.calls.slice(callCount).map(([name]) => name), ["notice"]);
    assert.match(run.calls.at(-1)[1], /finishes adding this conversion/);
  }
  run.context.screen = "create";
  run.context.createPreflightPhase = "submitting";
  for (const task of targets) {
    selectDetails(task);
    const before = workspace();
    const callCount = run.calls.length;
    run.viewTaskResults(task);
    await settle();
    assert.deepEqual(workspace(), before);
    assert.deepEqual(run.calls.slice(callCount).map(([name]) => name), ["notice"]);
    assert.match(run.calls.at(-1)[1], /finishes adding this create task/);
  }
  run.context.screen = "convert";
  run.context.createPreflightPhase = "idle";
  for (const [index, kind] of ["protect", "test"].entries()) {
    const task = { id: 12 + index, state: "done", expanded: false, spec: { kind, path: archive.path }, result: { ok: true } };
    run.context.jobRows.push(task);
    selectDetails(task);
    const before = workspace();
    run.viewTaskResults(task);
    assert.equal(task.expanded, true);
    assert.deepEqual(workspace(), before);
    assert.deepEqual(run.calls.at(-1), ["expand", task.id, true]);
  }
  const noResultScreen = { id: 14, state: "done", spec: { kind: "update", path: archive.path }, result: {} };
  selectDetails(noResultScreen);
  const beforeNoResult = workspace();
  const noResultCallCount = run.calls.length;
  run.viewTaskResults(noResultScreen);
  assert.deepEqual(workspace(), beforeNoResult);
  assert.equal(run.calls.length, noResultCallCount);
  run.context.taskWindowMode = true;
  const windowTask = { ...checksumTask(15), expanded: false };
  run.context.jobRows.push(windowTask);
  assert.equal(taskResultAvailableForSurface(windowTask, true), true);
  selectDetails(windowTask);
  const beforeWindow = workspace();
  const windowCallCount = run.calls.length;
  run.viewTaskResults(windowTask);
  assert.equal(windowTask.expanded, true);
  assert.deepEqual(workspace(), beforeWindow);
  assert.deepEqual(run.calls.slice(windowCallCount), [["expand", windowTask.id, true]]);
  run.context.taskWindowMode = false;
  assert.equal(run.calls.some(([name]) => ["submit", "convert-submit", "leave-convert", "cancel-convert", "focus", "restore-focus"].includes(name)), false);

  releaseInspection({ conflict: false, guard: null });
  await waitFor(() => session.surface("modern").preflight.phase === "ready");
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);
  for (const task of targets) {
    run.context.screen = "convert";
    selectDetails(task);
    session.surface("modern").start.onSelect();
    await waitFor(() => session.surface("modern").review !== null);
    const callCount = run.calls.length;
    run.viewTaskResults(task);
    await settle();
    assert.equal(run.context.screen, taskModel.taskResultScreen(task));
    assert.equal(run.context.taskCenterOpen, false);
    assert.equal(run.context.taskCenterSelectedTaskId, null);
    assert.equal(run.context.taskDialogTaskId, null);
    assert.equal(run.context.taskDialogDismissedId, task.id);
    assert.equal(run.context.pendingTaskReviewId, null);
    assert.equal(session.surface("modern").preflight.phase, "idle");
    assert.equal(session.surface("modern").review, null);
    assert.equal(run.calls.slice(callCount).filter(([name]) => name === "leave-convert").length, 1);
    if (task.spec.kind === "checksum" || task.spec.kind === "checksum_check") {
      assert.equal(run.selectedChecksumTask(task.spec.kind).id, task.id);
      assert.ok(run.calls.slice(callCount).some(([name, kind]) => name === "focus" && kind === task.spec.kind));
    } else if (task.spec.kind === "duplicate_scan") {
      assert.equal(run.context.duplicateReportTaskId, task.id);
      assert.equal(run.context.duplicateReportFocusPending, false);
      assert.ok(run.calls.slice(callCount).some(([name, kind]) => name === "focus" && kind === "duplicates"));
    } else {
      assert.equal(run.context.recoverySourceMode, "selected");
      assert.equal(run.context.recoverySourceOverride, task.spec.path);
      assert.equal(run.context.recoveryPar2Override, task.spec.recovery);
    }
  }
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);
  assert.equal(run.calls.some(([name]) => name === "submit" || name === "cancel-convert"), false);
});

test("removed reports and manifest submissions resolve to the appropriate task", async () => {
  const harness = reportHarness();
  harness.context.jobRows.push(checksumTask(1), checksumTask(2, "checksum_check"));
  harness.viewTaskResults(harness.context.jobRows[1]);
  await harness.submitChecksumCheckJob();
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 20);
  harness.context.jobRows.pop();
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 2);
  harness.context.jobRows.pop();
  assert.equal(harness.selectedChecksumTask("checksum_check"), null);
});

test("both workspace styles show the selected manifest report and full result paths", async () => {
  const harness = reportHarness();
  const task = checksumTask(7, "checksum_check");
  task.spec.algorithm = "sha512";
  task.result.ok = false;
  task.result.items = Array.from({ length: 25 }, (_, i) => ({
    path: `/run-7/folder-${i}/report.txt`, expected: "ab".repeat(64), actual: "cd".repeat(64), ok: false,
  }));
  harness.context.jobRows.push(task, checksumTask(8, "checksum_check"));
  harness.viewTaskResults(task);
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: ToolsWorkspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    harness.context.tr = tFallback;
    for (const variant of ["modern", "classic"]) {
      const surface = harness.checksumWorkspaceSurface(variant);
      assert.equal(surface.verification.context.algorithm, "SHA-512");
      assert.equal(surface.verification.context.sources[0], "/run-7/SHA256SUMS");
      assert.equal(surface.algorithm.selected, "sha256");
      assert.equal(surface.verification.rows.length, 20);
      assert.equal(surface.verification.totalRows, 25);
      assert.equal(harness.checksumResultText("checksum_check").split("\n").length, 25);
      const { body } = render(ToolsWorkspace, { props: { surface } });
      assert.ok(body.includes('/run-7/folder-0/report.txt'));
      assert.ok(body.includes('/run-7/folder-19/report.txt'));
      assert.ok(body.includes('ab'.repeat(64)) && body.includes('cd'.repeat(64)));
      assert.ok(body.includes(tFallback("gui.checksum.report_manifest")));
      assert.ok(body.includes(tFallback("gui.task.state.needs_attention")));
      assert.ok(body.includes(tFallback("gui.checksum.result_preview_rows")
        .replace("{shown}", "20").replace("{total}", "25")));
      assert.doesNotMatch(body, /run-8|gui\.checksum\./u);
    }
  }
});
