import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";

import { englishTranslate, createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen, isTaskActiveState } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { CreateSession } = await server.ssrLoadModule("/src/lib/create-session.svelte.ts");
const { ArchivePresetSession } = await server.ssrLoadModule("/src/lib/archive-presets.svelte.ts");
const { RecoverySession } = await server.ssrLoadModule("/src/lib/recovery-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const originalIpc = { ...ipc };
test.after(() => Object.assign(ipc, originalIpc));

function taskSpec(overrides = {}) {
  return {
    kind: "compress", inputs: ["/original/reports", "/original/photos"],
    dest: "/output/backup.zip", level: 4, password: null, encrypt_names: false,
    split_size: 123456789, split_mode: "native", excludes: ["*.bak", "cache/**"],
    content_policy: "custom", sqz_inner_format: null, sfx_target: null,
    replace_existing: true, replacement_guard: "obsolete-authorization",
    completion: "reveal_output", post_success: "keep_source", test_after_create: true,
    ...overrides,
  };
}

async function harness({ navigation = false, preparation = false, preview = false } = {}) {
  // Host routing and DOM focus remain integration tests; every Create command uses its real owner.
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const names = ["reviewTask", "preventCreateSubmissionNavigation", "preventConvertSubmissionNavigation",
    "preventTaskWorkspaceNavigation", "applyCreatePreflightEffect",
    ...(preparation ? ["setMode", "createPrimaryAction", "focusCreatePrimaryAction"] : []),
    ...(navigation ? ["setScreen", "dismissTaskDialog", "closeTaskCenter", "cancelTaskReview"] : [])];
  const declarations = selectFunctions(source, names);
  const calls = [];
  let requestId = 0;
  const context = {
    taskReviewScreen, isTaskActiveState,
    taskWindowMode: false, screen: "create", mode: "modern", createPrimaryFocusPending: null,
    blockingModalVisible: () => false,
    tr: englishTranslate, platformKind: () => "macos",
    setScreen: (screen) => calls.push(["screen", screen]), dismissTaskDialog: async () => { calls.push(["dismiss"]); },
    focusCreatePrimaryAction: () => calls.push(["focus"]), showNotice: (message) => calls.push(["notice", message]),
    focusBlockingTaskIfAny: () => false,
    archiveStemName: (value) => value.replace(/\.(?:tar\.zst|tzst|zip|7z|sqz|wim|swm|app|exe|run)$/i, ""),
    getDialogModule: async () => ({ save() {}, confirm: async () => { calls.push(["confirm"]); return true; } }),
    openNativeDialog: async () => null,
    saveNativeDialog: async (_purpose, _save, options) => { calls.push(["save", options]); return options.defaultPath; },
    ipc: {
      inspectCreateDestination: async (path) => {
        calls.push(["inspect", path]); return { conflict: true, guard: "fresh-authorization" };
      },
      cancelCreateDestinationInspection: async () => {},
      cancelCreatePlan: async (requestId) => { calls.push(["cancel-plan", requestId]); },
      uniqueCreateDestination: async (path) => path,
      planCreate: async (spec) => { calls.push(["plan", spec]); return { entries: 2, deduplicated_entries: 0,
        primary_output: spec.dest, total_bytes: 512,
        workspace_budget_bytes: 20, system_temp_budget_bytes: 0, final_output_budget_bytes: 10 }; },
      checkDiskSpace: async (path, bytes) => { calls.push(["space", path, bytes]); return { ok: true }; },
      tempDir: async () => "/temporary",
    },
    tick: async () => {}, document: { documentElement: {}, body: {}, querySelectorAll: () => [], querySelector: () => ({ focus() {} }) },
    convertRouteHandle: null, archiveOpenStatus: "idle", archivePasswordPrompt: null, previewPasswordPrompt: null,
    taskReviewRequestGeneration: 0, pendingTaskReviewId: null,
    taskCenterOpen: true, taskCenterSelectedTaskId: 8, taskCenterFocusTaskId: 8,
    taskDialogTaskId: 8, taskDialogDismissedId: null,
    archiveUpdateReview: { cancelSourceChoice() {} }, nestedExtractDraftGeneration: 0,
    nestedExtractPickerRequest: 0, batchPickerRequest: 0, pendingArchiveTaskReview: null,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {}, dismissArchiveAddPreparation() {},
    tools: { leave() {} },
    extraction: { leave() {}, invalidatePicker() {} },
    restoreTaskWorkspaceFocus: () => calls.push(["restore-focus"]),
    securitySettingsFocusPending: false, focusSecuritySettings: () => calls.push(["focus-security"]),
    currentArchive: null,
    submitJob: async (spec) => { calls.push(["submit", spec]); return 42; },
    taskCenterReturnFocus: null, HTMLElement: class {},
    trackAppearanceSave() {}, persistUiMode: (next) => { context.mode = next; return Promise.resolve(); },
  };
  const create = new CreateSession({
    read: () => ({ screen: context.screen, mode: context.mode, defaultDirectory: null }),
    presets: () => presets, notice: context.showNotice, platform: context.platformKind,
    getDialogModule: (...args) => context.getDialogModule(...args),
    openNativeDialog: (...args) => context.openNativeDialog(...args),
    saveNativeDialog: (...args) => context.saveNativeDialog(...args),
    nextRequestId: () => "create-preflight-" + ++requestId, archiveStemName: context.archiveStemName,
    submit: (...args) => context.submitJob(...args), blocked: () => context.focusBlockingTaskIfAny(),
    tError: (error) => error.key, submissionBlockedMessage: () => null,
    emit: (...args) => context.applyCreatePreflightEffect(...args),
    clearFocus: () => { context.createPrimaryFocusPending = null; },
    primaryFocusPending: () => context.createPrimaryFocusPending !== null,
    focusPrimary: () => context.focusCreatePrimaryAction(), focusInvalid() {}, scrollSection() {}, openRecovery() {},
    fileManager: () => "Finder", trashName: () => "Trash", featuredFormats: () => [], listen: async () => () => {},
  }, null, { scanned: 0, current: "", destinationBytes: preview ? 1024 : 0, destinationCurrent: "" });
  context.create = create;
  const presets = new ArchivePresetSession({
    create: { options: () => create.options.snapshotPreset(), apply: create.applyPreset,
      ready: () => create.capabilityReady, touched: () => create.options.state.touched,
      disabledReason: () => create.preflight.busy() ? "Busy" : "", saveIssue: create.saveIssue },
    extract: { options: () => ({}), apply: () => true, ready: () => true, touched: () => false, disabledReason: () => "" },
    notice: context.showNotice,
  });
  context.presets = presets;
  ipc.getArchivePresets = async () => ({ presets: [], bindings: {} });
  await presets.load();
  create.capabilityReady = true;
  const initialContext = create.draftContext();
  for (const [kind, value] of [["profile", "maximum"], ["customLevel", 9], ["contentPolicy", "cross_platform_clean"], ["excludeText", "old-rule"],
    ["password", "unrelated-secret"], ["passwordConfirmation", "unrelated-secret"],
    ["passwordVisible", true], ["encryptNames", true]]) create.options.edit({ kind, value }, initialContext, () => {});
  create.sources = [{ path: "/unrelated", kind: "folder" }];
  create.selected = ["/unrelated"];
  context.recoverySession = new RecoverySession({ read: () => ({ active: false, mode: context.mode, generation: 0,
    archive: context.currentArchive, tasks: [], selection: [], platform: "macos", level: 6, profile: "Balanced" }), notice: context.showNotice });
  context.recoverySession.selectSource("/unrelated/recovery.zip", "/unrelated/recovery.par2");
  if (preparation) {
    context.document.querySelector = () => ({ focus() { calls.push(["review-focus"]); } });
    context.document.getElementById = () => ({ focus() { calls.push(["primary-focus"]); } });
  }
  const outputText = compileTestScript(declarations.map((node) => node.getText(source)).join("\n"));
  context.calls = calls;
  const run = vm.runInNewContext(outputText + "\n({" + names.join(",") + ", create, context:globalThis, calls})", context);
  for (const method of ["inspectCreateDestination", "cancelCreateDestinationInspection", "uniqueCreateDestination",
    "planCreate", "cancelCreatePlan", "checkDiskSpace", "tempDir"]) ipc[method] = (...args) => context.ipc[method](...args);
  create.syncContext();
  return run;
}

for (const state of ["failed", "cancelled"]) {
test(`reviewing a ${state} creation restores its sources and options without restarting it`, async () => {
  const run = await harness();
  await run.reviewTask({ id: 8, state, spec: taskSpec() });
  const draft = run.context;
  assert.deepEqual(Array.from(run.create.sources, (source) => source.path), ["/original/reports", "/original/photos"]);
  assert.equal(run.create.options.state.format, "zip");
  assert.equal(run.create.options.state.profile, "custom");
  assert.equal(run.create.options.state.customLevel, 4);
  assert.equal(run.create.options.state.exactSplitSizeBytes, "123456789");
  assert.equal(run.create.options.state.splitMode, "native");
  assert.equal(run.create.options.state.contentPolicy, "custom");
  assert.equal(run.create.options.state.excludeText, "*.bak\ncache/**");
  assert.equal(run.create.options.state.completion, "reveal_output");
  assert.equal(run.create.options.state.postSuccess, "keep_source");
  assert.equal(run.create.options.state.testAfterCreate, true);
  assert.equal(run.create.options.state.suggestedDestination, "/output/backup.zip");
  assert.equal(run.create.options.state.destinationBase, "ask");
  assert.equal(run.create.options.state.overwritePolicy, "ask");
  assert.equal(run.context.presets.selection("create").id, null);
  assert.equal(run.create.options.state.touched, true);
  assert.equal(run.create.options.state.password, "");
  assert.equal(run.create.options.state.passwordConfirmation, "");
  assert.equal(run.create.options.state.passwordVisible, false);
  assert.equal(run.create.preflight.state.pending, null);
  assert.equal(run.create.preflight.state.destination, null);
  assert.equal(run.create.preflight.state.plan, null);
  assert.equal(run.create.preflight.state.destinationDisk, null);
  assert.equal(run.create.section, "general");
  assert.equal(run.calls.filter(([name]) => name === "plan").length, 0);
  assert.ok(run.calls.some(([name, screen]) => name === "screen" && screen === "create"));
});
}

for (const state of ["failed", "cancelled"]) {
test(`reviewing a ${state} encrypted ZIP requires a new password and keeps protection across edits and discarded plans`, async () => {
  const run = await harness();
  await run.reviewTask({ id: 9, state, spec: taskSpec(), outputPasswordRequired: true });
  assert.equal(run.create.options.state.encryptionEnabled, true);
  assert.equal(run.create.options.state.password, "");
  assert.equal(run.create.options.state.encryptNames, false);
  assert.equal(run.create.captureRun(), null);
  run.create.chooseFormat("tar.zst");
  assert.equal(run.create.options.state.format, "zip");
  run.create.edit({ kind: "password", value: "replacement" });
  run.create.edit({ kind: "passwordConfirmation", value: "different" });
  assert.equal(run.create.captureRun(), null);
  run.create.edit({ kind: "passwordConfirmation", value: "replacement" });
  const draft = run.create.captureRun();
  assert.equal(draft.password, "replacement");
  assert.equal(draft.restoreCredentialPrompt, true);
  const preset = run.create.options.snapshotPreset();
  assert.deepEqual(preset.credential, { kind: "prompt" });
  assert.equal(Object.hasOwn(preset, "password"), false, "saved options contain intent, never a run password");
  assert.equal(Object.hasOwn(preset, "split_mode"), false, "native layout belongs to the run contract");
  preset.excludes.push("preset-only");
  preset.destination.base = "source_parent";
  draft.excludes.push("run-only");
  draft.destination.base = "default_directory";
  assert.deepEqual(run.create.options.excludeRules, ["*.bak", "cache/**"]);
  assert.equal(run.create.options.state.destinationBase, "ask", "both snapshots are detached from the editable options");
  await run.create.start(taskSpec().inputs, "dialog");
  assert.equal(run.create.preflight.state.phase, "reviewing");
  assert.equal(run.create.preflight.state.pending.spec.password, "replacement");
  run.create.options.resetCredentials(draft);
  assert.equal(run.create.options.state.password, "");
  assert.equal(run.create.options.state.encryptionEnabled, true);
  assert.equal(run.create.captureRun(), null);
  run.create.edit({ kind: "password", value: "replacement" });
  assert.equal(run.create.options.state.password, "replacement", "old plan cleanup runs before the new password edit");
  assert.equal(run.create.preflight.state.pending, null);
  run.create.edit({ kind: "password", value: "" });
  assert.equal(run.create.captureRun(), null);
  run.create.edit({ kind: "password", value: "replacement" });
  run.create.edit({ kind: "passwordConfirmation", value: "replacement" });
  await run.create.start(taskSpec().inputs, "dialog");
  assert.ok(run.create.preflight.state.pending);
  run.create.edit({ kind: "encryptionEnabled", value: false });
  assert.equal(run.create.options.state.encryptionEnabled, false, "old plan intent cannot re-enable a newly disabled option");
  assert.equal(run.create.preflight.state.pending, null);
  assert.equal(run.create.captureRun().password, null);
  run.create.chooseFormat("tar.zst");
  assert.equal(run.create.options.state.format, "tar.zst");
});
}

test("restored name encryption is not silently removed by choosing ZIP or a self-extractor", async () => {
  const run = await harness();
  run.create.restoreTask(taskSpec({ dest: "/secure.7z", encrypt_names: true }), true);
  run.create.chooseFormat("zip");
  run.create.edit({ kind: "sfxEnabled", value: true });
  assert.equal(run.create.options.state.format, "7z");
  assert.equal(run.create.options.state.sfxEnabled, false);
  run.create.edit({ kind: "encryptNames", value: false });
  assert.equal(run.create.options.state.encryptionEnabled, true);
  assert.equal(run.create.captureRun(), null);
  run.create.chooseFormat("zip");
  assert.equal(run.create.options.state.format, "zip");
});

for (const state of ["failed", "cancelled"]) {
test(`restored ${state} creation chooses the destination again and checks sources, space and current overwrite permission`, async () => {
  const run = await harness();
  const spec = taskSpec();
  await run.reviewTask({ id: 8, state, spec });
  assert.match(run.create.view.outputPreview(), /Confirm location when starting.*\/output\/backup.zip/);
  assert.equal(run.create.view.archivePreviewName(), "backup.zip");
  await run.create.start(spec.inputs, "dialog");
  assert.deepEqual(run.calls.filter(([name]) => ["save", "inspect", "confirm", "plan", "space"].includes(name))
    .map(([name]) => name), ["save", "inspect", "confirm", "plan", "space", "space"]);
  assert.equal(run.calls.find(([name]) => name === "save")[1].defaultPath, spec.dest);
  const pending = run.create.preflight.state.pending;
  assert.equal(pending.spec.replacement_guard, "fresh-authorization");
  assert.equal(pending.spec.replace_existing, true);
  assert.equal(pending.spec.password, null);
  assert.equal(pending.spec.level, 4);
  assert.equal(pending.spec.split_size, 123456789);
  assert.equal(pending.spec.split_mode, "native");
  assert.equal(run.create.preflight.state.phase, "reviewing");
});
}

test("creation review respects format variants, current platform capability and an in-progress draft", async (t) => {
  for (const [dest, expected] of [["a.zip", "zip"], ["a.7Z", "7z"], ["a.sqz", "sqz"],
    ["a.tar.zst", "tar.zst"], ["a.TZST", "tar.zst"], ["a.wim", "wim"], ["a.swm", "wim"]]) {
    const run = await harness();
    assert.equal(run.create.restoreTask(taskSpec({ dest, sqz_inner_format: "7z" })), true);
    assert.equal(run.create.options.state.format, expected);
    if (expected === "sqz") assert.equal(run.create.options.state.sqzInnerFormat, "7z");
  }
  const sfx = await harness();
  assert.equal(sfx.create.restoreTask(taskSpec({ dest: "/output/Installer.app", sfx_target: "macos", split_size: null })), true);
  assert.equal(sfx.create.options.state.sfxEnabled, true);
  assert.equal(sfx.create.options.state.sfxTarget, "macos");
  assert.equal(sfx.create.options.state.format, "zip");
  assert.equal(sfx.create.options.state.splitPreset, "none");
  assert.equal(sfx.create.captureRun().suggestedDestination, "/output/Installer.app");
  for (const setup of [
    (run) => run.create.preflight.start(["/unrelated"], "dialog", run.create.captureRun()),
    (run) => { run.create.capabilityReady = false; },
    (run) => { run.create.capability.available = false; },
    (run) => { run.create.capability.target = "windows"; },
  ]) {
    const run = await harness();
    await setup(run);
    const destination = run.create.preflight.state.destination;
    const options = run.create.options.snapshotPreset();
    run.reviewTask({ id: 8, state: "failed", spec: taskSpec({ sfx_target: "macos" }) });
    assert.equal(run.create.sources[0].path, "/unrelated");
    assert.equal(run.create.preflight.state.destination, destination);
    assert.deepEqual(run.create.options.snapshotPreset(), options, "blocked restoration keeps every editable option");
    assert.equal(run.calls.some(([name]) => name === "screen" || name === "dismiss"), false);
  }
  const unsupported = await harness();
  assert.equal(unsupported.create.restoreTask(taskSpec({ dest: "/output/backup.tar" })), false);
  assert.equal(unsupported.create.sources[0].path, "/unrelated");

  const run = await harness({ navigation: true });
  const archive = { id: 1, path: "/current/photos.zip", source: "/current/photos.zip", name: "photos.zip",
    format: "zip", entry_count: 3, garbled_count: 0, non_utf8_name_count: 0, encoding_override: null };
  run.context.currentArchive = archive;
  run.context.screen = "convert";
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
    ensurePreflightListener: async () => {}, getDialogModule: run.context.getDialogModule,
    saveNativeDialog: async () => "/output/photos.7z",
    submitJob: async (spec) => { run.calls.push(["convert-submit", spec]); return 42; },
    focusBlockingTaskIfAny: () => false, isJobSubmitBlocked: () => false, jobSubmitBlockedMessage: () => "Busy",
    recordQueuedOperation() {}, archiveStemName: run.context.archiveStemName, platform: run.context.platformKind,
    prepareSubmitFocus() {}, shouldRestorePrimaryFocus: () => false, register() {},
  });
  t.after(() => {
    session.dispose();
    releaseInspection?.({ conflict: false, guard: null });
    Object.assign(ipc, original);
  });
  run.context.convertRouteHandle = session;
  session.syncArchive(archive);
  session.surface("modern").start.onSelect();
  const waitFor = async (predicate) => {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (predicate()) return;
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    assert.fail("Conversion did not reach the expected state");
  };
  await waitFor(() => session.surface("modern").review !== null);
  session.surface("modern").review.onConfirm();
  await waitFor(() => Boolean(releaseInspection));
  assert.equal(session.surface("modern").preflight.phase, "submitting");

  const plain = (value) => JSON.parse(JSON.stringify(value));
  const createDraft = () => {
    const captured = run.create.captureRun();
    assert.ok(captured);
    const { password, ...options } = captured;
    return plain({ sources: run.create.sources,
      selectedSources: run.create.selected, preset: run.context.presets.selection("create").id,
      touched: run.create.options.state.touched, advanced: run.create.advancedOpen,
      section: run.create.section, hasPassword: Boolean(password), options });
  };
  const draft = createDraft();
  const reviewTargets = [
    { id: 8, state: "failed", spec: taskSpec() },
    { id: 8, state: "cancelled", spec: taskSpec() },
    { id: 8, state: "failed", spec: { kind: "protect", path: "/original/recovery.zip" } },
    { id: 8, state: "failed", spec: { kind: "test", path: archive.path }, error: { key: "error.resource_limit" } },
  ];
  for (const task of reviewTargets) {
    await run.reviewTask(task);
    assert.deepEqual(createDraft(), draft, `${task.state} ${task.spec.kind} must retain the create draft`);
    assert.equal(run.context.screen, "convert");
    assert.equal(run.context.taskCenterOpen, true);
    assert.equal(run.context.taskCenterSelectedTaskId, 8);
    assert.equal(run.context.taskCenterFocusTaskId, 8);
    assert.equal(run.context.taskDialogTaskId, 8);
    assert.equal(run.context.taskDialogDismissedId, null);
    assert.equal(run.context.recoverySession.sourcePath(), "/unrelated/recovery.zip");
    assert.equal(run.context.recoverySession.sidecarPath(), "/unrelated/recovery.par2");
    assert.equal(run.context.securitySettingsFocusPending, false);
    assert.equal(session.surface("modern").preflight.phase, "submitting");
    assert.equal(run.calls.some(([name]) => ["focus", "focus-security", "restore-focus", "convert-submit", "cancel-convert"].includes(name)), false);
  }
  assert.match(run.calls.at(-1)[1], /finishes adding this conversion/);
  releaseInspection({ conflict: false, guard: null });
  await waitFor(() => session.surface("modern").preflight.phase === "ready");
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);

  session.surface("modern").start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  const conversion = plain(session.surface("modern"));
  await run.reviewTask({ id: 8, state: "failed", spec: taskSpec({ dest: "/output/backup.tar" }) });
  assert.deepEqual(plain(session.surface("modern")), conversion);
  assert.deepEqual(createDraft(), draft);
  assert.equal(run.context.screen, "convert");
  assert.equal(run.context.taskCenterOpen, true);

  await run.reviewTask({ id: 8, state: "failed", spec: taskSpec() });
  assert.equal(run.context.screen, "create");
  assert.equal(run.context.taskCenterOpen, false);
  assert.equal(run.context.taskCenterSelectedTaskId, null);
  assert.equal(run.context.taskDialogDismissedId, 8);
  assert.deepEqual(Array.from(run.create.sources, (source) => source.path), taskSpec().inputs);
  assert.equal(session.surface("modern").preflight.phase, "idle");
  assert.equal(session.surface("modern").review, null);
  assert.equal(run.calls.filter(([name]) => name === "focus").length, 1);
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);
  assert.equal(run.calls.some(([name]) => name === "plan"), false);

  await run.create.preflight.start(taskSpec().inputs, "dialog", run.create.captureRun());
  const createSubmission = deferred();
  let createSubmitting = false;
  run.context.submitJob = () => { createSubmitting = true; return createSubmission.promise; };
  const confirmingCreate = run.create.preflight.confirm();
  await waitFor(() => createSubmitting);
  assert.equal(run.create.preflight.state.phase, "submitting");
  run.context.taskCenterOpen = true;
  run.context.taskCenterSelectedTaskId = 8;
  run.context.taskCenterFocusTaskId = 8;
  run.context.taskDialogTaskId = 8;
  run.context.taskDialogDismissedId = null;
  const submittingDraft = createDraft();
  for (const task of reviewTargets.slice(2)) {
    const callsBeforeReview = run.calls.length;
    await run.reviewTask(task);
    assert.deepEqual(createDraft(), submittingDraft);
    assert.equal(run.context.screen, "create");
    assert.equal(run.create.preflight.state.phase, "submitting");
    assert.equal(run.context.recoverySession.sourcePath(), "/unrelated/recovery.zip");
    assert.equal(run.context.recoverySession.sidecarPath(), "/unrelated/recovery.par2");
    assert.equal(run.context.securitySettingsFocusPending, false);
    assert.equal(run.context.taskCenterOpen, true);
    assert.equal(run.context.taskCenterSelectedTaskId, 8);
    assert.equal(run.context.taskCenterFocusTaskId, 8);
    assert.equal(run.context.taskDialogTaskId, 8);
    assert.equal(run.context.taskDialogDismissedId, null);
    assert.equal(run.calls.slice(callsBeforeReview).some(([name]) => ["focus", "focus-security", "restore-focus", "convert-submit"].includes(name)), false);
    assert.match(run.calls.at(-1)[1], /finishes adding this create task/);
  }
  createSubmission.resolve(42);
  await confirmingCreate;
});

test("edited formats adapt the original output suggestion and cancelled selection never creates a plan", async () => {
  const run = await harness();
  run.create.restoreTask(taskSpec({ dest: "/output/my.backup.TZST", split_size: null }));
  assert.equal(run.create.suggestedOutputPath("tar.zst", "tar.zst"), "/output/my.backup.TZST");
  run.create.chooseFormat("7z");
  assert.equal(run.create.captureRun().suggestedDestination, "/output/my.backup.7z");
  assert.equal(run.create.suggestedOutputPath("wim", "swm"), "/output/my.backup.swm");
  run.context.saveNativeDialog = async () => null;
  await run.create.start(["/original/reports"], "dialog");
  assert.equal(run.calls.some(([name]) => name === "inspect" || name === "plan"), false);
  assert.equal(run.create.preflight.state.pending, null);
  assert.equal(run.create.preflight.state.phase, "cancelled");
  assert.equal(run.create.preflight.state.issueStage, "destination");
  assert.match(run.calls.findLast(([name]) => name === "notice")[1], /Destination selection cancelled/);
});

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

async function waitFor(predicate, label) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail(`Creation did not reach ${label}`);
}

async function creationPreparation({ stop = null, pauseNewPlan = false, automaticDestination = false } = {}) {
  const run = await harness({ navigation: true, preparation: true });
  run.context.taskCenterOpen = false;
  run.create.restoreTask(taskSpec({ split_size: null }));
  if (automaticDestination) run.create.edit({ kind: "destinationBase", value: "source_parent" });
  const blocked = deferred();
  const newerPlan = deferred();
  let hit = false;
  let newerPlanHit = false;
  const invoke = (label, value, detail) => {
    run.calls.push([label, detail]);
    if (label === "plan" && pauseNewPlan && detail.inputs[0] === "/new/source.txt") {
      newerPlanHit = true;
      return newerPlan.promise;
    }
    if (label === stop && !hit) {
      hit = true;
      blocked.value = value;
      return blocked.promise;
    }
    return Promise.resolve(value);
  };
  const plan = (spec) => ({ entries: 2, deduplicated_entries: 1, total_bytes: 512,
    primary_output: spec.dest, workspace_budget_bytes: 20, system_temp_budget_bytes: 30,
    final_output_budget_bytes: 10 });
  const dialog = {
    open() {}, save() {},
    confirm: (message) => invoke("confirm", true, message),
  };
  run.context.getDialogModule = () => invoke("dialog-module", dialog);
  run.context.saveNativeDialog = (_purpose, _save, options) => invoke("save", options.defaultPath, options);
  run.context.openNativeDialog = (_purpose, _open, options) => invoke("open", ["/picked/source.txt"], options);
  run.create.ensureEvents = () => invoke(
    run.create.preflight.state.phase === "measuring" ? "source-listener" : "destination-listener",
    undefined,
  );
  run.context.ipc = {
    inspectCreateDestination: (path, split, requestId) => invoke("inspection", {
      conflict: true, guard: "current-output",
    }, { path, split, requestId }),
    cancelCreateDestinationInspection: (requestId) => invoke("cancel-inspection", undefined, requestId),
    cancelCreatePlan: (requestId) => invoke("cancel-plan", undefined, requestId),
    uniqueCreateDestination: (path) => invoke("unique-name", path, path),
    planCreate: (spec, requestId) => invoke("plan", plan(spec), { ...spec, requestId }),
    tempDir: () => invoke("system-temp-directory", "/system-temporary"),
    checkDiskSpace: (path, bytes) => invoke(bytes === 20 ? "workspace-space"
      : bytes === 30 ? "system-temp-space" : "destination-space", { ok: true, available_bytes: 10000 }, { path, bytes }),
  };
  run.context.tick = () => run.context.screen === "create" && run.create.preflight.state.phase === "reviewing"
    ? invoke("review-tick", undefined) : run.context.createPrimaryFocusPending
      ? invoke("primary-focus-tick", undefined) : Promise.resolve();
  return { run, blocked, newerPlan, plan, reached: () => hit, newPlanReached: () => newerPlanHit };
}

function preparationState(run) {
  const state = run.create.preflight.state;
  return {
    owner: state.owner,
    picker: state.picker,
    phase: state.phase,
    requestId: state.requestId,
    requestKind: state.requestKind,
    scanned: state.scanned,
    processedBytes: state.processedBytes,
    cancelling: state.cancelPending,
    primaryFocusPending: run.context.createPrimaryFocusPending,
    current: state.current,
    issue: state.issue,
    issueStage: state.issueStage,
    plan: state.plan,
    dest: state.destination,
    workspace: state.workspaceDisk,
    systemTemp: state.systemTempDisk,
    disk: state.destinationDisk,
    pending: state.pending,
    noticeCount: run.calls.filter(([name]) => name === "notice").length,
    focusCount: run.calls.filter(([name]) => name === "review-focus" || name === "focus" || name === "primary-focus").length,
  };
}

test("creation rejects inconsistent destination guards before confirmation or planning", async () => {
  for (const inspection of [{ conflict: false, guard: "unexpected" }, { conflict: true, guard: null }]) {
    const { run } = await creationPreparation();
    run.context.ipc.inspectCreateDestination = async () => inspection;
    await run.create.start(taskSpec().inputs, "dialog");
    assert.equal(run.create.preflight.state.phase, "blocked");
    assert.equal(run.create.preflight.state.issueStage, "destination");
    assert.equal(run.calls.some(([name]) => name === "confirm" || name === "plan"), false);
    assert.equal(run.create.preflight.state.pending, null);
  }
});

test("creation ignores a disk outcome when navigation occurs between publication and the caller resuming", async () => {
  for (const ok of [true, false]) {
    const { run } = await creationPreparation();
    let retained;
    run.context.ipc.checkDiskSpace = (_path, bytes) => {
      const disk = { ok: bytes === 10 ? ok : true, available_bytes: 10000 };
      const result = Promise.resolve(disk);
      if (bytes === 10) {
        result.then(() => queueMicrotask(() => {
          assert.equal(run.create.preflight.state.destinationDisk?.ok, ok);
          assert.equal(run.create.preflight.state.phase, "checkingDest");
          run.setScreen("browse");
          retained = preparationState(run);
        }));
      }
      return result;
    };
    await run.create.start(taskSpec().inputs, "dialog");
    assert.ok(retained);
    assert.deepEqual(preparationState(run), retained);
  }
});

test("shared output authorization preserves SFX inspection and automatic unique output submission", async () => {
  const { run } = await creationPreparation();
  run.create.restoreTask(taskSpec({ dest: "/output/Installer.app", sfx_target: "macos", split_size: null }));
  const inspections = [];
  run.context.ipc.inspectCreateDestination = async (path, split, _requestId, target) => {
    inspections.push({ path, split, target });
    return { conflict: true, guard: "app-directory" };
  };
  await run.create.start(taskSpec().inputs, "dialog");
  await run.create.preflight.confirm();
  assert.deepEqual(inspections, [
    { path: "/output/Installer.app", split: false, target: "macos" },
    { path: "/output/Installer.app", split: false, target: "macos" },
  ]);
  assert.equal(run.calls.filter(([name]) => name === "confirm").length, 1);
  assert.equal(run.calls.find(([name]) => name === "submit")[1].replacement_guard, "app-directory");

  const automatic = await creationPreparation({ automaticDestination: true });
  await automatic.run.create.start(taskSpec().inputs, "dialog");
  assert.equal(automatic.run.create.preflight.state.pending.confirmLateConflict, false);
  await automatic.run.create.preflight.confirm();
  assert.equal(automatic.run.calls.some(([name]) => name === "inspection" || name === "confirm"), false);
  const submitted = automatic.run.calls.find(([name]) => name === "submit")[1];
  assert.equal(submitted.replace_existing, false);
  assert.equal(submitted.replacement_guard, null);
});

test("create preparation abandons every late boundary and preserves a newer preparation after leaving and returning", async () => {
  const stages = ["dialog-module", "save", "destination-listener", "inspection", "confirm",
    "source-listener", "plan", "workspace-space", "system-temp-directory", "system-temp-space",
    "destination-space", "review-tick", "unique-name"];
  for (const stage of stages) {
    for (const rejects of stage === "review-tick" ? [false] : [false, true]) {
      const { run, blocked, newerPlan, plan, reached, newPlanReached } = await creationPreparation({
        stop: stage, pauseNewPlan: true, automaticDestination: stage === "unique-name",
      });
      const first = run.create.start(taskSpec().inputs, "dialog");
      await waitFor(reached, stage);
      const sourceRequestId = run.create.preflight.state.requestId;
      run.setScreen("browse");
      if (stage === "plan") {
        assert.ok(sourceRequestId);
        assert.deepEqual(run.calls.filter(([name]) => name === "cancel-plan"), [["cancel-plan", sourceRequestId]]);
      }
      if (stage === "inspection") {
        assert.ok(sourceRequestId);
        assert.deepEqual(run.calls.filter(([name]) => name === "cancel-inspection"), [["cancel-inspection", sourceRequestId]]);
      }
      assert.equal(run.context.screen, "browse", stage);
      assert.equal(run.create.preflight.state.owner, null, stage);
      assert.equal(run.create.preflight.state.pending, null, stage);
      run.setScreen("create");
      assert.equal(run.create.restoreTask(taskSpec({ inputs: ["/new/source.txt"],
        dest: "/new-output/new.zip", level: 6, split_size: null })), true);
      const second = run.create.start(["/new/source.txt"], "dialog");
      await waitFor(newPlanReached, `newer source plan after ${stage}`);
      const current = preparationState(run);
      assert.ok(current.owner, stage);
      assert.equal(current.phase, "measuring", stage);
      const callCount = run.calls.length;
      if (rejects && stage !== "review-tick") blocked.reject(new Error("Old preparation failed"));
      else blocked.resolve(blocked.value);
      await first;
      assert.deepEqual(preparationState(run), current, `${stage} late ${rejects ? "error" : "result"}`);
      assert.equal(run.calls.length, callCount, `${stage} must not open another dialog or check`);
      const newSpec = run.calls.findLast(([name, detail]) => name === "plan" && detail.inputs[0] === "/new/source.txt")[1];
      newerPlan.resolve(plan(newSpec));
      await second;
      assert.equal(run.create.preflight.state.phase, "reviewing", stage);
      assert.equal(run.create.preflight.state.owner, null, stage);
      assert.deepEqual(Array.from(run.create.preflight.state.pending.spec.inputs), ["/new/source.txt"], stage);
      assert.equal(run.create.preflight.state.pending.spec.level, 6, stage);
      assert.equal(run.create.preflight.state.pending.spec.dest, "/new-output/new.zip", stage);
    }
  }
  for (const change of ["mode", "source"]) {
    const { run, blocked, reached } = await creationPreparation({ stop: "plan" });
    const preparing = run.create.start(taskSpec().inputs, "dialog");
    await waitFor(reached, "source plan before context invalidation");
    const sourceRequestId = run.create.preflight.state.requestId;
    if (change === "mode") run.setMode("classic");
    else run.create.appendSources(["/new/source.txt"], "file");
    run.create.syncContext();
    assert.deepEqual(run.calls.filter(([name]) => name === "cancel-plan"), [["cancel-plan", sourceRequestId]], change);
    assert.equal(run.create.preflight.state.owner, null, change);
    assert.equal(run.create.preflight.state.phase, "idle", change);
    assert.equal(run.create.preflight.busy(), false, change);
    const retained = preparationState(run);
    blocked.resolve(blocked.value);
    await preparing;
    assert.deepEqual(preparationState(run), retained, `${change} invalidation cannot become a review`);
  }
});

test("disposing a source preflight cancels its request and ignores late results and errors", async () => {
  for (const rejects of [false, true]) {
    const { run, blocked, reached } = await creationPreparation({ stop: "plan" });
    const preparing = run.create.start(taskSpec().inputs, "dialog");
    await waitFor(reached, "source plan before disposal");
    const requestId = run.create.preflight.state.requestId;
    assert.ok(requestId);
    run.create.dispose();
    assert.deepEqual(run.calls.filter(([name]) => name === "cancel-plan"), [["cancel-plan", requestId]]);
    const disposed = preparationState(run);
    const callCount = run.calls.length;
    if (rejects) blocked.reject(new Error("Disposed source scan cancelled"));
    else blocked.resolve(blocked.value);
    await preparing;
    assert.deepEqual(preparationState(run), disposed);
    assert.equal(run.calls.length, callCount);
    assert.equal(run.calls.some(([name]) => name === "workspace-space" || name === "submit"), false);
  }
});

test("Create event subscriptions and capability replies cannot outlive the workspace", async () => {
  const run = await harness();
  const subscribed = deferred();
  let subscriptions = 0;
  let cleanups = 0;
  let receive;
  run.create.port.listen = (callback) => {
    subscriptions += 1;
    receive = callback;
    return subscribed.promise;
  };
  const first = run.create.ensureEvents();
  const second = run.create.ensureEvents();
  assert.equal(first, second, "concurrent callers share the native subscription");
  const capability = deferred();
  ipc.getSfxCreateCapability = () => capability.promise;
  run.create.capabilityReady = false;
  const initialCapability = run.create.capability;
  const loading = run.create.loadCapability();
  run.create.dispose();
  subscribed.resolve(() => { cleanups += 1; });
  capability.resolve({ ...initialCapability, available: false, status: "invalid" });
  await Promise.all([first, second, loading]);
  assert.equal(cleanups, 1, "a subscription completed after disposal is released immediately");
  assert.equal(run.create.capability, initialCapability);
  assert.equal(run.create.capabilityReady, false);
  const disposed = preparationState(run);
  receive({ request_id: "stale", scanned: 99, current: "/late" });
  assert.deepEqual(preparationState(run), disposed);
  await run.create.ensureEvents();
  assert.equal(subscriptions, 1, "a closed workspace never reconnects");
});

test("Create forwards conversion events and accepts only its current native request", async () => {
  const run = await harness();
  run.create.restoreTask(taskSpec({ split_size: null }), false);
  let receive;
  let forwarded = false;
  let cleanups = 0;
  run.create.port.listen = async (callback) => { receive = callback; return () => { cleanups += 1; }; };
  run.create.port.forwardEvent = () => forwarded;
  const plan = deferred();
  run.context.ipc.planCreate = () => plan.promise;
  const preparing = run.create.review();
  await waitFor(() => run.create.preflight.state.requestKind === "source", "current native source request");
  const requestId = run.create.preflight.state.requestId;
  receive({ request_id: "older", scanned: 3 });
  assert.equal(run.create.preflight.state.scanned, 0);
  forwarded = true;
  receive({ request_id: requestId, scanned: 5 });
  assert.equal(run.create.preflight.state.scanned, 0, "events consumed by Convert stay out of Create");
  forwarded = false;
  receive({ request_id: requestId, scanned: 7, current: "/source/current" });
  assert.equal(run.create.preflight.state.scanned, 7);
  assert.equal(run.create.preflight.state.current, "/source/current");
  run.create.dispose();
  plan.reject(new Error("disposed"));
  await preparing;
  assert.equal(cleanups, 1);
});

test("create source choosing retains new sources, ignores stale errors and clears only its own picker", async () => {
  for (const stage of ["dialog-module", "open"]) {
    for (const rejects of [false, true]) {
      const { run, blocked, reached } = await creationPreparation({ stop: stage });
      const first = run.create.chooseSources("files");
      await waitFor(reached, stage);
      run.setScreen("browse");
      assert.equal(run.create.preflight.state.picker, null);
      run.setScreen("create");
      run.create.restoreTask(taskSpec({ inputs: ["/new/source.txt"], dest: "/new-output/new.zip", split_size: null }));
      const newer = deferred();
      let newerOpened = false;
      run.context.openNativeDialog = () => { newerOpened = true; return newer.promise; };
      const second = run.create.chooseSources("folder");
      await waitFor(() => newerOpened, "new source chooser");
      const picker = run.create.preflight.state.picker;
      const notices = run.calls.filter(([name]) => name === "notice").length;
      const calls = run.calls.length;
      if (rejects) blocked.reject(new Error("Old source chooser failed"));
      else blocked.resolve(blocked.value);
      await first;
      assert.equal(run.create.preflight.state.picker, picker);
      assert.deepEqual(Array.from(run.create.sources, (source) => source.path), ["/new/source.txt"]);
      assert.equal(run.calls.filter(([name]) => name === "notice").length, notices);
      assert.equal(run.calls.length, calls);
      newer.resolve(["/new/folder"]);
      await second;
      assert.equal(run.create.preflight.state.picker, null);
      assert.deepEqual(Array.from(run.create.sources, (source) => source.path), ["/new/source.txt", "/new/folder"]);
      assert.equal(run.create.sources[1].kind, "folder");
    }
  }

  const { run } = await creationPreparation();
  const sourcesBeforeCancel = run.create.sources;
  run.context.openNativeDialog = async () => null;
  await run.create.chooseSources("files");
  assert.equal(run.create.sources, sourcesBeforeCancel);
  assert.equal(run.create.preflight.state.picker, null);
  assert.match(run.calls.at(-1)[1], /Source selection cancelled/);

  for (const change of ["mode", "source", "preset"]) {
    const { run: pending, blocked, reached } = await creationPreparation({ stop: "open" });
    const choosing = pending.create.chooseSources("files");
    await waitFor(reached, "source chooser before a draft change");
    const picker = pending.create.preflight.state.picker;
    const sourceRoots = pending.create.sources;
    pending.create.appendSources(taskSpec().inputs, "unknown");
    assert.equal(pending.create.sources, sourceRoots, "unchanged roots keep their identity");
    assert.equal(pending.create.preflight.state.picker, picker, "a duplicate source is not a new intent");
    assert.equal(pending.create.restoreTask(taskSpec({ dest: "/unsupported.tar" })), false);
    assert.equal(pending.create.preflight.state.picker, picker, "a blocked review keeps its owner");

    ipc.getArchivePresets = async () => ({ presets: [{ id: "invalid", kind: "create", label: "Invalid",
      options: { format: "rar", output: { kind: "archive" } } },
    { id: "replacement", kind: "create", label: "Replacement", options: {
      format: "zip", level: 2, credential: { kind: "prompt" }, encrypt_names: false,
      volumes: { kind: "split", size_bytes: "123456789" }, content_policy: "custom", excludes: ["*.tmp"],
      output: { kind: "archive" }, format_options: { kind: "none" },
      destination: { base: "source_parent", existing_output: "ask" }, completion: "open_in_squallz", post_success: "trash_source", test_after_create: false,
    } }], bindings: { app_default_create: "replacement" } });
    await pending.context.presets.load();
    const optionsBeforeInvalidPreset = pending.create.options.snapshotPreset();
    pending.context.presets.apply("create", "invalid", false);
    assert.equal(pending.create.preflight.state.picker, picker, "an unavailable preset keeps its owner");
    assert.deepEqual(pending.create.options.snapshotPreset(), optionsBeforeInvalidPreset);
    pending.context.presets.applyDefault("create");
    assert.deepEqual(pending.create.options.snapshotPreset(), optionsBeforeInvalidPreset, "a default cannot replace an already touched draft");
    assert.equal(pending.create.preflight.state.picker, picker);
    if (change === "mode") {
      pending.setMode("classic");
      pending.create.syncContext();
    } else if (change === "source") {
      pending.create.appendSources(["/new/source.txt"], "file");
      pending.create.syncContext();
    } else {
      pending.context.presets.apply("create", "replacement", false);
      assert.equal(pending.create.options.state.customLevel, 2);
      assert.equal(pending.context.presets.selection("create").id, "replacement");
      assert.equal(pending.create.options.state.splitMode, "generic", "presets deliberately do not restore a run's native layout");
      assert.equal(pending.create.options.state.exactSplitSizeBytes, "123456789");
      assert.equal(pending.create.options.snapshotPreset().volumes.size_bytes, "123456789");
      assert.equal(pending.create.options.state.overwritePolicy, "rename", "applying a preset derives conflict handling from its base");
      assert.equal(pending.create.options.state.suggestedDestination, null);
      assert.equal(pending.create.options.state.encryptionEnabled, true);
      assert.equal(pending.create.options.state.password, "");
      assert.equal(pending.create.options.state.completion, "reveal_output");
      assert.equal(pending.create.options.state.postSuccess, "keep_source");
      const applied = pending.create.options.snapshotPreset();
      pending.context.presets.apply("create", null, false);
      assert.equal(pending.context.presets.selection("create").id, null);
      assert.deepEqual(pending.create.options.snapshotPreset(), applied, "clearing a selection keeps its editable options");
    }
    assert.equal(pending.create.preflight.state.picker, null, `${change} ends the old chooser`);
    const retainedRoots = pending.create.sources;
    const retained = preparationState(pending);
    blocked.resolve(blocked.value);
    await choosing;
    assert.equal(pending.create.sources, retainedRoots, `${change} ignores the old picked source`);
    assert.deepEqual(preparationState(pending), retained, `${change} keeps the new draft and feedback`);
  }
});

test("same-page creation completes all checks, cancels cleanly and retains an in-flight submission when navigation is requested", async (t) => {
  const originalSetTimeout = globalThis.setTimeout;
  t.after(() => { globalThis.setTimeout = originalSetTimeout; });
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: CreateWorkspace } = await server.ssrLoadModule("/src/components/CreateWorkspace.svelte");
  const workspaceBody = (run, variant) => render(CreateWorkspace, {
    props: { variant, session: run.create },
  }).body;
  const { run } = await creationPreparation();
  const firstDraft = run.create.captureRun();
  await run.create.start(taskSpec().inputs, "dialog");
  assert.deepEqual(run.calls.filter(([name]) => ["save", "inspection", "confirm", "plan", "workspace-space",
    "system-temp-directory", "system-temp-space", "destination-space"].includes(name)).map(([name]) => name),
  ["save", "inspection", "confirm", "plan", "workspace-space", "system-temp-directory", "system-temp-space", "destination-space"]);
  assert.equal(run.create.preflight.state.phase, "reviewing");
  assert.equal(run.create.preflight.state.plan.entries, 2);
  assert.equal(run.create.preflight.state.scanned, 3);
  assert.equal(run.create.preflight.state.pending.spec.level, firstDraft.level);
  assert.equal(run.create.preflight.state.pending.spec.replacement_guard, "current-output");
  assert.equal(run.create.preflight.state.owner, null);
  assert.equal(run.calls.filter(([name]) => name === "review-focus").length, 1);
  for (const variant of ["modern", "classic"]) {
    const body = workspaceBody(run, variant);
    assert.match(body, /create-plan-review/);
    assert.doesNotMatch(body, /create-preflight-action|Reading current output again|Rechecking the current output/);
    if (variant === "modern") assert.doesNotMatch(body, /create-preflight-status/);
  }
  run.create.cancelReview();
  assert.equal(run.create.preflight.state.pending, null);
  assert.equal(run.create.preflight.state.plan, null);
  assert.equal(run.create.preflight.state.phase, "idle");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);

  run.context.saveNativeDialog = async () => null;
  await run.create.start(taskSpec().inputs, "dialog");
  assert.equal(run.create.preflight.state.phase, "cancelled");
  assert.equal(run.create.preflight.state.owner, null);
  assert.equal(run.create.preflight.state.pending, null);
  run.context.saveNativeDialog = async (_purpose, _save, options) => options.defaultPath;
  await run.create.start(taskSpec().inputs, "dialog");
  const pending = run.create.preflight.state.pending;
  const submission = deferred();
  let submitted;
  run.context.submitJob = (spec) => { submitted = spec; return submission.promise; };
  const confirming = run.create.confirm();
  await waitFor(() => Boolean(submitted), "queue submission");
  run.setScreen("browse");
  assert.equal(run.context.screen, "create");
  assert.equal(run.create.preflight.state.pending, pending);
  assert.equal(run.create.preflight.state.phase, "submitting");
  assert.match(run.calls.at(-1)[1], /finishes adding this create task/);
  submission.resolve(42);
  await confirming;
  assert.equal(run.create.preflight.state.phase, "ready");
  assert.equal(run.create.preflight.state.pending, null);
  assert.equal(run.create.sources.length, 0);
  assert.deepEqual(JSON.parse(JSON.stringify(submitted)), taskSpec({ split_size: null, split_mode: "generic",
    replacement_guard: "current-output" }));

  const rechecking = await creationPreparation();
  await rechecking.run.create.start(taskSpec().inputs, "dialog");
  const reinspection = deferred();
  let reinspectionStarted = false;
  rechecking.run.context.ipc.inspectCreateDestination = () => {
    reinspectionStarted = true;
    return reinspection.promise;
  };
  const reconfirming = rechecking.run.create.confirm();
  await waitFor(() => reinspectionStarted, "confirmed output reinspection");
  const requestId = rechecking.run.create.preflight.state.requestId;
  assert.ok(requestId);
  assert.equal(rechecking.run.create.preflight.state.phase, "submitting");
  assert.equal(rechecking.run.create.preflight.state.requestKind, "destination");
  assert.equal(rechecking.run.create.preflight.applyEvent({ request_id: requestId, phase: "destination",
    processed_bytes: 2048, current: "/output/backup.zip" }), true);
  const cancellationButton = (body) => body.match(/<button\b[^>]*class="[^"]*\bcreate-preflight-action\b[^"]*"[^>]*>[\s\S]*?<\/button>/)?.[0];
  for (const variant of ["modern", "classic"]) {
    const body = workspaceBody(rechecking.run, variant);
    assert.match(body, /create-plan-review/);
    assert.match(body, /create-preflight-status phase-submitting/);
    assert.match(body, /Rechecking the current output/);
    assert.match(body, /Reading current output again · 2\.0 KB/);
    assert.match(body, /Current · \/output\/backup\.zip/);
    const button = cancellationButton(body);
    assert.ok(button, `${variant} output reinspection keeps its cancellation entry`);
    assert.match(button, /Cancel output check/);
    assert.doesNotMatch(button, /\bdisabled\b/);
    assert.match(button, /aria-busy="false"/);
  }
  await rechecking.run.create.view.surface("modern").preflight.onAction();
  assert.equal(rechecking.run.create.preflight.state.cancelPending, true);
  assert.equal(rechecking.run.calls.findLast(([name]) => name === "cancel-inspection")[1], requestId);
  for (const variant of ["modern", "classic"]) {
    const button = cancellationButton(workspaceBody(rechecking.run, variant));
    assert.ok(button, `${variant} pending cancellation remains visible`);
    assert.match(button, /\bdisabled\b/);
    assert.match(button, /aria-busy="true"/);
    assert.match(button, /Stopping the output check/);
  }
  reinspection.resolve({ conflict: true, guard: "current-output" });
  await reconfirming;
  assert.equal(rechecking.run.create.preflight.state.phase, "reviewing");
  assert.ok(rechecking.run.create.preflight.state.pending);
  assert.equal(rechecking.run.calls.some(([name]) => name === "submit"), false);

  const cancelled = await creationPreparation({ stop: "inspection" });
  const checking = cancelled.run.create.start(taskSpec().inputs, "dialog");
  await waitFor(cancelled.reached, "cancellable output inspection");
  await cancelled.run.create.preflight.cancelDestination();
  assert.equal(cancelled.run.create.preflight.state.cancelPending, true);
  cancelled.blocked.resolve(cancelled.blocked.value);
  await checking;
  assert.equal(cancelled.run.create.preflight.state.phase, "cancelled");
  assert.equal(cancelled.run.create.preflight.state.owner, null);
  assert.equal(cancelled.run.create.preflight.state.pending, null);
  assert.equal(cancelled.run.create.preflight.state.requestId, null);
  assert.equal(cancelled.run.create.preflight.state.cancelPending, false);
  assert.equal(cancelled.run.calls.some(([name]) => name === "plan" || name === "submit"), false);
  assert.match(cancelled.run.calls.findLast(([name]) => name === "notice")[1], /Output check cancelled/);
  assert.equal(cancelled.run.calls.filter(([name]) => name === "primary-focus").length, 1);

  const delayedFocus = await creationPreparation({ stop: "primary-focus-tick", pauseNewPlan: true });
  const inspection = deferred();
  const inspectCurrent = delayedFocus.run.context.ipc.inspectCreateDestination;
  let firstInspection = true;
  delayedFocus.run.context.ipc.inspectCreateDestination = (...args) => {
    if (!firstInspection) return inspectCurrent(...args);
    firstInspection = false;
    return inspection.promise;
  };
  const oldPreparation = delayedFocus.run.create.start(taskSpec().inputs, "dialog");
  await waitFor(() => delayedFocus.run.create.preflight.state.requestKind === "destination", "inspection before focus cancellation");
  await delayedFocus.run.create.preflight.cancelDestination();
  inspection.resolve({ conflict: true, guard: "current-output" });
  await waitFor(delayedFocus.reached, "cancelled inspection focus tick");
  delayedFocus.run.setScreen("browse");
  delayedFocus.run.setScreen("create");
  delayedFocus.run.create.restoreTask(taskSpec({ inputs: ["/new/source.txt"],
    dest: "/new-output/new.zip", split_size: null }));
  const newPreparation = delayedFocus.run.create.start(["/new/source.txt"], "dialog");
  await waitFor(delayedFocus.newPlanReached, "new preparation before the old focus tick");
  const current = preparationState(delayedFocus.run);
  delayedFocus.blocked.resolve();
  await oldPreparation;
  assert.deepEqual(preparationState(delayedFocus.run), current);
  assert.equal(delayedFocus.run.calls.some(([name]) => name === "primary-focus"), false);
  const newSpec = delayedFocus.run.calls.findLast(([name]) => name === "plan")[1];
  delayedFocus.newerPlan.resolve(delayedFocus.plan(newSpec));
  await newPreparation;
  assert.equal(delayedFocus.run.create.preflight.state.phase, "reviewing");
  assert.deepEqual(Array.from(delayedFocus.run.create.preflight.state.pending.spec.inputs), ["/new/source.txt"]);

  for (const leaveAndReturn of [false, true]) {
    const preview = await harness({ navigation: true, preparation: true, preview: true });
    const timer = deferred();
    globalThis.setTimeout = (callback, delay) => {
      assert.equal(delay, 180);
      preview.calls.push(["preview-cancel-timer", delay]);
      timer.promise.then(callback);
      return 1;
    };
    assert.equal(preview.create.preflight.state.phase, "choosingDest");
    assert.equal(preview.create.preflight.state.requestKind, "destination");
    assert.equal(preview.create.preflight.state.processedBytes, 1024);
    const sources = preview.create.sources;
    const mode = preview.context.mode;
    const cancelling = preview.create.preflight.cancelDestination();
    assert.equal(preview.calls.filter(([name]) => name === "preview-cancel-timer").length, 1);
    assert.equal(preview.create.preflight.state.cancelPending, true);
    assert.equal(preview.calls.some(([name]) => name === "primary-focus" || name === "notice"), false);
    if (leaveAndReturn) {
      preview.setScreen("browse");
      preview.setScreen("create");
      assert.equal(preview.context.screen, "create");
      assert.equal(preview.context.mode, mode);
      assert.equal(preview.create.sources, sources);
      assert.equal(preview.create.preflight.state.owner, null);
      const retained = preparationState(preview);
      const calls = preview.calls.length;
      timer.resolve();
      await cancelling;
      assert.deepEqual(preparationState(preview), retained, "returning to the same page, mode and sources cannot revive a preview cancellation");
      assert.equal(preview.calls.length, calls);
      assert.equal(preview.create.preflight.state.phase, "idle");
      assert.equal(preview.create.preflight.busy(), false);
    } else {
      timer.resolve();
      await cancelling;
      assert.equal(preview.create.preflight.state.phase, "cancelled");
      assert.equal(preview.create.preflight.state.owner, null);
      assert.equal(preview.create.preflight.state.requestId, null);
      assert.equal(preview.create.preflight.state.cancelPending, false);
      assert.equal(preview.create.sources, sources);
      assert.equal(preview.calls.filter(([name]) => name === "primary-focus").length, 1);
      assert.match(preview.calls.findLast(([name]) => name === "notice")[1], /Output check cancelled/);
    }
  }
});
