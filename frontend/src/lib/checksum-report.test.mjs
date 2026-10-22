import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";

import { createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";
import { extractionFixture } from "../../tests/extraction.mjs";

const server = await createTestServer();
test.after(() => server.close());
const taskModel = await server.ssrLoadModule("/src/lib/task-model.ts");
const { taskResultAvailableForSurface } = await server.ssrLoadModule("/src/lib/task-dialog.ts");
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { ToolsSession } = await server.ssrLoadModule("/src/lib/tools-session.svelte.ts");
const { RecoverySession } = await server.ssrLoadModule("/src/lib/recovery-session.svelte.ts");
const { CreateSession } = await server.ssrLoadModule("/src/lib/create-session.svelte.ts");
const extraction = await extractionFixture(server);
test.afterEach(() => extraction.dispose());
const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
await loadLocale("en-US");

function reportHarness({ navigation = false } = {}) {
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const names = ["viewTaskResults", "localToolsSurface",
    "preventCreateSubmissionNavigation", "preventTaskWorkspaceNavigation",
    ...(navigation ? ["setScreen", "preventConvertSubmissionNavigation", "dismissTaskDialog",
      "openTaskCenter", "closeTaskCenter", "openTaskCenterDetails", "returnToTaskCenter",
      "cancelTaskReview",
      "focusChecksumResultPanel", "focusPendingChecksumResult", "focusDuplicateReportPanel"] : [])];
  const functions = selectFunctions(source, names).map((node) => node.getText(source)).join("\n");
  const calls = [];
  const document = { documentElement: {}, body: {}, activeElement: null, querySelectorAll: () => [] };
  document.activeElement = document.body;
  const panel = (name) => ({ isConnected: true, scrollIntoView() {}, focus() {
    document.activeElement = this;
    calls.push(["focus", name]);
  } });
  const context = {
    ...taskModel,
    jobRows: [], taskWindowMode: false,
    copyTextToClipboard: async () => "copied",
    toolsArchiveReturnSurface: () => ({ visible: false }),
    setScreen() {}, dismissTaskDialog() {}, focusChecksumResultPanel() {},
    registerDuplicateReportPanel() {},
    focusBlockingTaskIfAny: () => false, showNotice: (message) => calls.push(["notice", message]), recordOperation() {},
    tr: tFallback, pathBaseName: (path) => path.split("/").at(-1),
    submitJob: async (spec) => {
      calls.push(["submit", spec.kind]);
      context.jobRows.push({ id: 20, spec, result: null, state: "queued" });
      return 20;
    },
    screen: "checksum", mode: "modern", convertRouteHandle: null,
    archiveOpenStatus: "idle", archivePasswordPrompt: null, previewPasswordPrompt: null,
    taskReviewRequestGeneration: 0, pendingTaskReviewId: null,
    taskCenterOpen: false, taskCenterSelectedTaskId: null, taskCenterFocusTaskId: null,
    activePopover: null, appActionEnabled: () => true, rememberTaskWorkspaceFocus() {},
    taskDialogTaskId: null, taskDialogDismissedId: null,
    taskDialogTask: () => context.jobRows.find((task) => task.id === context.taskDialogTaskId) ?? null,
    archiveUpdateReview: { cancelSourceChoice() {} }, nestedExtractDraftGeneration: 0,
    nestedExtractPickerRequest: 0, batchPickerRequest: 0, pendingArchiveTaskReview: null,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {},
    restoreTaskWorkspaceFocus: () => calls.push(["restore-focus"]),
    currentArchive: null, sameFilePath: (left, right) => left === right,
    dismissArchiveAddPreparation() {},
    checksumResultFocusPending: null, modeSelectionBlocked: false,
    duplicateReportFocusPending: false, createPrimaryFocusPending: true,
    extractReviewFocusPending: true, convertReviewFocusPending: true,
    securitySettingsFocusPending: true, archiveUpdateReviewFocusPending: true,
    nestedExtractReviewFocusPending: true, batchReviewFocusPending: true,
    checksumResultPanel: panel("checksum"), checksumCheckResultPanel: panel("checksum_check"),
    duplicateReportPanel: panel("duplicates"), blockingModalVisible: () => false,
    tick: async () => {}, document,
    setTaskExpanded: (id, expanded) => {
      calls.push(["expand", id, expanded]);
      const task = context.jobRows.find((row) => row.id === id);
      if (task) task.expanded = expanded;
    },
  };
  extraction.install(context);
  context.create = new CreateSession({
    read: () => ({ screen: context.screen, mode: context.mode, defaultDirectory: null }),
    platform: () => "macos", presets: () => context.presets,
    notice: context.showNotice, clearFocus() {},
  });
  context.recoverySession = new RecoverySession({
    read: () => ({ archive: context.currentArchive, tasks: context.jobRows, platform: "macos" }),
    notice: context.showNotice,
  });
  context.recoverySession.selectSource("/previous/recovery.zip", "/previous/recovery.par2");
  const tools = new ToolsSession({
    read: () => ({ tasks: context.jobRows, archive: context.currentArchive, screen: context.screen,
      copyScope: [context.screen, context.mode, context.taskCenterOpen, context.taskCenterSelectedTaskId,
        context.taskDialogTaskId, context.taskDialogDismissedId].join(":"),
      visibleTaskId: context.taskWindowMode ? context.taskDialogTask()?.id ?? null : context.taskCenterOpen
        ? context.taskCenterSelectedTaskId : context.taskDialogDismissedId === context.taskDialogTaskId ? null : context.taskDialogTaskId,
    }),
    choose: async () => null, submitJob: context.submitJob,
    submissionBlocked: () => false, isSubmitBlocked: () => false, notice: context.showNotice,
    copy: (...args) => context.copyTextToClipboard(...args),
  }, { checksumPath: "/new/files", manifestPath: "/new/SHA256SUMS", checksumExcludes: "" });
  test.after(() => { tools.dispose(); context.recoverySession.dispose(); context.create.dispose(); });
  context.tools = tools;
  const outputText = compileTestScript(functions);
  context.calls = calls;
  return vm.runInNewContext(`${outputText}\n({${names.join(",")}, context:globalThis, calls, tools})`, context);
}

function checksumTask(id, kind = "checksum") {
  return { id, state: "done", expanded: true,
    spec: kind === "checksum"
      ? { kind, inputs: [`/run-${id}`], algorithm: "sha256", excludes: [] }
      : { kind, manifest: `/run-${id}/SHA256SUMS`, algorithm: "sha256" },
    result: jobResult(kind, { files_hashed: id, checked: id,
      items: [{ path: `/run-${id}/report.txt`, digest: `digest-${id}`, actual: `actual-${id}` }] }),
  };
}

test("checksum report navigation and copying stay bound to the selected task", async (t) => {
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  const harness = reportHarness();
  const { context } = harness;
  context.jobRows.push(checksumTask(1), checksumTask(2), checksumTask(3, "checksum_check"));
  harness.viewTaskResults(context.jobRows[0]);
  assert.equal(harness.tools.selectedTask("checksum").id, 1);
  assert.equal(harness.tools.result("checksum")?.files_hashed, 1);
  assert.equal(harness.tools.checksumText("checksum"), "digest-1  /run-1/report.txt");
  context.jobRows.push(checksumTask(4));
  assert.equal(harness.tools.selectedTask("checksum").id, 1);
  harness.viewTaskResults(context.jobRows[2]);
  assert.equal(harness.tools.selectedTask("checksum_check").id, 3);
  assert.equal(harness.tools.selectedTask("checksum").id, 1);
  await harness.tools.copyResults("checksum");
  assert.equal((harness.tools.copyFeedback("checksum")?.message ?? null), "Checksum results copied");
  harness.viewTaskResults(context.jobRows[1]);
  assert.equal((harness.tools.copyFeedback("checksum")?.message ?? null), null);
  assert.equal((harness.tools.copyFeedback("checksum")?.tone ?? null), null);
  await harness.tools.submit("checksum");
  assert.equal(harness.tools.selectedTask("checksum").id, 20);
  assert.equal(harness.tools.checksumText("checksum"), "");
  assert.equal(harness.tools.selectedTask("checksum_check").id, 3);

  const copyHarness = () => {
    const run = reportHarness({ navigation: true });
    const tasks = [checksumTask(1), checksumTask(2), checksumTask(3, "checksum_check")];
    const writes = [];
    run.context.jobRows.push(...tasks);
    run.tools.adoptTask(tasks[0]);
    run.tools.adoptTask(tasks[2]);
    run.tools.leave();
    run.context.copyTextToClipboard = (text, isCurrent) => {
      let resolve;
      const promise = new Promise((done) => { resolve = done; });
      writes.push({ text, isCurrent, resolve });
      return promise;
    };
    return { run, tasks, writes };
  };
  {
    const { run, tasks, writes } = copyHarness();
    const old = run.tools.copyResults("checksum");
    assert.equal(writes[0].text, "digest-1  /run-1/report.txt");
    assert.equal(writes[0].isCurrent(), true);
    assert.equal(run.tools.copyPending("checksum", 1), true);
    assert.equal(run.calls.some(([name]) => name === "notice"), false, "waiting must not announce success");
    run.viewTaskResults(tasks[1]);
    assert.equal(writes[0].isCurrent(), false);
    const newer = run.tools.copyResults("checksum");
    writes[1].resolve("copied");
    await newer;
    const timer = run.tools.feedback;
    const noticeCount = run.calls.filter(([name]) => name === "notice").length;
    assert.equal((run.tools.copyFeedback("checksum")?.message ?? null), "Checksum results copied");
    writes[0].resolve("failed");
    await old;
    assert.equal((run.tools.copyFeedback("checksum")?.message ?? null), "Checksum results copied");
    assert.equal((run.tools.copyFeedback("checksum")?.tone ?? null), "success");
    assert.equal(run.tools.feedback, timer, "an old result must not replace the new feedback timer");
    assert.equal(run.tools.feedback?.tone, "success");
    assert.equal(run.calls.filter(([name]) => name === "notice").length, noticeCount);
  }
  {
    const { run, tasks, writes } = copyHarness();
    run.openTaskCenter();
    run.openTaskCenterDetails(tasks[0]);
    const old = run.tools.copyTask(tasks[0]);
    assert.equal(run.tools.copyPending("task", 1), true);
    run.openTaskCenterDetails(tasks[1]);
    const newer = run.tools.copyTask(tasks[1]);
    writes[1].resolve("copied");
    await newer;
    const notices = run.calls.filter(([name]) => name === "notice").length;
    writes[0].resolve("failed");
    await old;
    assert.equal(writes[0].isCurrent(), false);
    assert.equal((run.tools.copyFeedback("task", tasks[1].id)?.message ?? null), "Checksum results copied");
    assert.equal((run.tools.copyFeedback("task", tasks[1].id)?.tone ?? null), "success");
    assert.equal(run.calls.filter(([name]) => name === "notice").length, notices);
  }
  {
    const { run, writes } = copyHarness();
    const old = run.tools.copyResults("checksum");
    const newer = run.tools.copyResults("checksum_check");
    assert.equal(writes[0].isCurrent(), false, "a different result kind must supersede the previous copy");
    assert.equal(run.tools.copyPending("checksum_check", 3), true);
    writes[0].resolve("superseded");
    await old;
    assert.equal(run.tools.copyPending("checksum_check", 3), true, "old finally must not release a newer request");
    assert.equal(run.tools.copyPending("checksum_check", 3), true);
    writes[1].resolve("copied");
    await newer;
    assert.equal((run.tools.copyFeedback("checksum_check")?.message ?? null), "Checksum results copied");
    assert.equal((run.tools.copyFeedback("checksum")?.message ?? null), null);
  }
  {
    const { run, writes } = copyHarness();
    const old = run.tools.copyResults("checksum");
    const newer = run.tools.copyResults("checksum");
    assert.equal(writes[0].isCurrent(), false, "a new copy of the same task must still supersede the old request");
    writes[1].resolve("copied");
    await newer;
    writes[0].resolve("failed");
    await old;
    assert.equal((run.tools.copyFeedback("checksum")?.message ?? null), "Checksum results copied");
  }
  for (const leave of ["screen", "task-center", "task-back", "dialog-dismiss"]) {
    const { run, tasks, writes } = copyHarness();
    if (leave === "task-back") {
      run.openTaskCenter();
      run.openTaskCenterDetails(tasks[0]);
    } else if (leave === "dialog-dismiss") run.context.taskDialogTaskId = tasks[0].id;
    const fromTask = leave === "task-back" || leave === "dialog-dismiss";
    const pending = fromTask
      ? run.tools.copyTask(tasks[0]) : run.tools.copyResults("checksum");
    if (leave === "screen") { run.setScreen("browse"); run.setScreen("checksum"); }
    else if (leave === "task-center") { run.openTaskCenter(); run.closeTaskCenter(); }
    else if (leave === "task-back") { run.returnToTaskCenter(tasks[0]); run.openTaskCenterDetails(tasks[0]); }
    else {
      await run.dismissTaskDialog(tasks[0]);
      run.context.taskDialogTaskId = tasks[0].id;
      run.context.taskDialogDismissedId = null;
    }
    assert.equal(writes[0].isCurrent(), false, `${leave} must remain cancelled after returning to the same context`);
    const notices = run.calls.filter(([name]) => name === "notice").length;
    writes[0].resolve("copied");
    await pending;
    assert.equal((run.tools.feedback?.message ?? null), null);
    assert.equal(run.calls.filter(([name]) => name === "notice").length, notices);
  }
  for (const result of ["copied", "failed", "superseded"]) {
    const { run, writes } = copyHarness();
    const pending = run.tools.copyResults("checksum");
    writes[0].resolve(result);
    await pending;
    assert.equal(run.tools.copyPending("checksum", 1), false);
    assert.equal((run.tools.copyFeedback("checksum")?.tone ?? null), result === "copied" ? "success" : result === "failed" ? "danger" : null);
    assert.equal(run.calls.filter(([name]) => name === "notice").length, result === "superseded" ? 0 : 1);
  }
  for (const taskWindowMode of [false, true]) {
    const { run, tasks, writes } = copyHarness();
    Object.assign(run.context, { taskWindowMode, taskDialogTaskId: tasks[0].id });
    const pending = run.tools.copyTask(tasks[0]);
    assert.equal(run.tools.copyPending("task", tasks[0].id), true);
    writes[0].resolve("copied");
    await pending;
    assert.equal((run.tools.copyFeedback("task", tasks[0].id)?.message ?? null), "Checksum results copied",
      "normal task dialog and independent window copying must remain available");
  }
  {
    const { run, writes } = copyHarness();
    const pending = run.tools.copyResults("checksum");
    run.context.mode = "classic";
    assert.equal(writes[0].isCurrent(), false, "a copy must not retain the prior interface after switching modes");
    writes[0].resolve("failed");
    await pending;
    assert.equal(run.calls.some(([name]) => name === "notice"), false);
  }
  {
    const { run, writes } = copyHarness();
    await run.tools.copyText(" ", "checksum", 1);
    assert.equal(writes.length, 0);
    assert.equal((run.tools.copyFeedback("checksum")?.message ?? null), "No checksum results to copy");
    assert.equal((run.tools.copyFeedback("checksum")?.tone ?? null), "danger");
    assert.equal(run.tools.copyPending("checksum", 1), false);
  }

  const openLazyReport = (kind) => {
    const run = reportHarness({ navigation: true });
    const task = checksumTask(6, kind);
    const panels = { checksum: run.context.checksumResultPanel, checksum_check: run.context.checksumCheckResultPanel };
    Object.assign(run.context, {
      screen: "browse", checksumResultPanel: null, checksumCheckResultPanel: null,
      taskCenterOpen: true, taskCenterSelectedTaskId: task.id, taskDialogTaskId: task.id,
    });
    run.context.jobRows.push(task);
    run.viewTaskResults(task);
    return { run, panels, mount: run.localToolsSurface("checksum", "modern").onPanelMount };
  };
  for (const kind of ["checksum", "checksum_check"]) {
    const { run, panels, mount } = openLazyReport(kind);
    await settle();
    assert.equal(run.context.screen, "checksum");
    assert.equal(run.context.taskCenterOpen, false);
    assert.equal(run.context.taskDialogTaskId, null);
    assert.equal(run.context.document.activeElement, run.context.document.body);
    if (kind === "checksum_check") {
      mount("checksum", panels.checksum);
      await settle();
      assert.equal(run.context.document.activeElement, run.context.document.body, "the calculation panel must not consume verification focus");
    }
    mount(kind, panels[kind]);
    await settle();
    assert.equal(run.context.document.activeElement, panels[kind], "the selected report must receive focus when its panel becomes available");
    const input = {};
    run.context.document.activeElement = input;
    mount(kind, null);
    mount(kind, panels[kind]);
    await settle();
    assert.equal(run.context.document.activeElement, input, "a consumed focus request must not run again after remounting");
    assert.deepEqual(run.calls.filter(([name]) => name === "focus"), [["focus", kind]]);
  }
  for (const change of ["leave-return", "user-focus", "task-center"]) {
    const { run, panels, mount } = openLazyReport("checksum_check");
    await settle();
    const input = {};
    if (change === "leave-return") {
      run.setScreen("recovery");
      run.setScreen("checksum");
    } else if (change === "task-center") {
      run.openTaskCenter();
      assert.equal(run.context.taskCenterOpen, true);
      run.closeTaskCenter();
    } else run.context.document.activeElement = input;
    mount("checksum_check", panels.checksum_check);
    await settle();
    assert.equal(run.context.document.activeElement, change === "user-focus" ? input : run.context.document.body,
      "a late report must not steal focus after newer navigation, user interaction or task views");
    assert.equal(run.calls.some(([name]) => name === "focus"), false);
  }
  {
    const { run, panels, mount } = openLazyReport("checksum_check");
    const newer = checksumTask(7);
    run.context.jobRows.push(newer);
    run.viewTaskResults(newer);
    await settle();
    mount("checksum_check", panels.checksum_check);
    await settle();
    assert.equal(run.context.document.activeElement, run.context.document.body);
    mount("checksum", panels.checksum);
    await settle();
    assert.equal(run.context.document.activeElement, panels.checksum);
    assert.equal(run.tools.selectedTask("checksum").id, newer.id);
    assert.deepEqual(run.calls.filter(([name]) => name === "focus"), [["focus", "checksum"]]);
  }

  const run = reportHarness({ navigation: true });
  const archive = { id: 1, path: "/current/photos.zip", source: "/current/photos.zip", name: "photos.zip",
    format: "zip", entry_count: 3, garbled_count: 0, non_utf8_name_count: 0, encoding_override: null };
  run.context.currentArchive = archive;
  run.context.screen = "convert";
  const targets = [checksumTask(2), checksumTask(5, "checksum_check"),
    { id: 10, state: "done", expanded: false, spec: { kind: "duplicate_scan", inputs: ["/scan-10"] },
      result: jobResult("duplicate_scan", { duplicate_groups: 1, groups: [] }) },
    { id: 11, state: "done", expanded: false,
      spec: { kind: "repair_recovery", path: "/original/recovery.zip", recovery: "/original/recovery.par2" }, result: jobResult("repair_recovery", { ok: true }) }];
  run.context.jobRows.push(checksumTask(1), checksumTask(3, "checksum_check"),
    { id: 9, state: "done", spec: { kind: "duplicate_scan", inputs: ["/previous/scan"] },
      result: jobResult("duplicate_scan", { duplicate_groups: 2, groups: [] }) }, ...targets);
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
  const selectDetails = (task) => Object.assign(run.context, {
    taskCenterOpen: true, taskCenterSelectedTaskId: task.id, taskCenterFocusTaskId: task.id,
    taskDialogTaskId: task.id, taskDialogDismissedId: null, pendingTaskReviewId: task.id,
  });
  const workspace = () => plain(Object.fromEntries([
    "screen", "taskCenterOpen", "taskCenterSelectedTaskId",
    "taskCenterFocusTaskId", "taskDialogTaskId", "taskDialogDismissedId", "pendingTaskReviewId",
    "taskReviewRequestGeneration", "duplicateReportFocusPending", "createPrimaryFocusPending",
    "extractReviewFocusPending", "convertReviewFocusPending", "securitySettingsFocusPending",
    "archiveUpdateReviewFocusPending", "nestedExtractReviewFocusPending", "batchReviewFocusPending",
  ].map((key) => [key, run.context[key]]).concat([["toolReports", run.tools.selectedReports], ["recoverySource", run.context.recoverySession.sourcePath()], ["recoverySidecar", run.context.recoverySession.sidecarPath()]])));
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
  run.context.create.preflight.canLeave = () => false;
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
  run.context.create.preflight.canLeave = () => true;
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
  const noResultScreen = { id: 14, state: "done", spec: { kind: "update", path: archive.path }, result: jobResult("update", {}) };
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
      assert.equal(run.tools.selectedTask(task.spec.kind).id, task.id);
      assert.ok(run.calls.slice(callCount).some(([name, kind]) => name === "focus" && kind === task.spec.kind));
    } else if (task.spec.kind === "duplicate_scan") {
      assert.equal(run.tools.selectedReports.duplicate_scan, task.id);
      assert.equal(run.context.duplicateReportFocusPending, false);
      assert.ok(run.calls.slice(callCount).some(([name, kind]) => name === "focus" && kind === "duplicates"));
    } else {
      assert.equal(run.context.recoverySession.sourcePath(), task.spec.path);
      assert.equal(run.context.recoverySession.sidecarPath(), task.spec.recovery);
    }
  }
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);
  assert.equal(run.calls.some(([name]) => name === "submit" || name === "cancel-convert"), false);
});

test("removed reports and manifest submissions resolve to the appropriate task", async () => {
  const harness = reportHarness();
  harness.context.jobRows.push(checksumTask(1), checksumTask(2, "checksum_check"));
  harness.viewTaskResults(harness.context.jobRows[1]);
  await harness.tools.submit("checksum_check");
  assert.equal(harness.tools.selectedTask("checksum_check").id, 20);
  harness.context.jobRows.pop();
  assert.equal(harness.tools.selectedTask("checksum_check").id, 2);
  harness.context.jobRows.pop();
  assert.equal(harness.tools.selectedTask("checksum_check"), null);
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
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    for (const variant of ["modern", "classic"]) {
      const surface = harness.localToolsSurface("checksum", variant);
      assert.equal(surface.session.checksumReport("checksum_check").context.algorithm, "SHA-512");
      assert.equal(surface.session.checksumReport("checksum_check").context.sources[0], "/run-7/SHA256SUMS");
      assert.equal(surface.session.draft.algorithm, "sha256");
      assert.equal(surface.session.checksumReport("checksum_check").rows.length, 20);
      assert.equal(surface.session.checksumReport("checksum_check").totalRows, 25);
      assert.equal(harness.tools.checksumText("checksum_check").split("\n").length, 25);
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
