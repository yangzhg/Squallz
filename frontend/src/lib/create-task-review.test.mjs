import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const model = await server.ssrLoadModule("/src/lib/ui-model.ts");
const paths = await server.ssrLoadModule("/src/lib/desktop-path.ts");
const sources = await server.ssrLoadModule("/src/lib/create-sources.ts");
const { taskReviewScreen, isTaskActiveState } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");

function taskSpec(overrides = {}) {
  return {
    kind: "compress", inputs: ["/original/reports", "/original/photos"],
    dest: "/output/backup.zip", level: 4, password: null, encrypt_names: false,
    split_size: 123456789, split_mode: "native", excludes: ["*.bak", "cache/**"],
    content_policy: "custom", sqz_inner_format: null, sfx_target: null,
    replace_existing: true, replacement_guard: "obsolete-authorization",
    completion: "reveal", post_success: "keep_source", test_after_create: true,
    ...overrides,
  };
}

function harness({ navigation = false } = {}) {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts",
    component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["reviewTask", "restoreCreateTaskDraft", "createTaskFormat", "applyPresetVolumeMode",
    "invalidateCreatePreflightResult", "clearCreatePasswordFields", "markCreatePresetDraftTouched",
    "createSuggestedOutputPath", "createSaveDefaultPathForDraft", "createArchiveNameForOutput",
    "captureCreateRunDraft", "archiveOutputExtension", "submitCreateInputs", "beginCreatePreflight",
    "askCreateDestination", "normalizeCreateDestinationForDraft", "createSaveFiltersForDraft",
    "resolveCreateDestination", "createOutputPreview", "createArchivePreviewName",
    "createPasswordValidationMessage", "validateCreateOptions", "updateCreatePassword",
    "updateCreatePasswordConfirmation", "updateCreateEncryptionEnabled", "chooseCreateFormat",
    "activeCreateFormatData", "updateCreateEncryptNames", "updateCreateSfxEnabled",
    "resetCreateCredentialsAfterPlan", "preventCreateSubmissionNavigation", "preventConvertSubmissionNavigation", "preventTaskWorkspaceNavigation", "dismissRecoveryPicker",
    ...(navigation ? ["setScreen", "dismissTaskDialog", "closeTaskCenter", "cancelTaskReview", "adoptRecoveryTargetFromTask"] : [])];
  const declarations = source.statements.filter((node) =>
    ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = {
    ...model, ...paths, ...sources, taskReviewScreen, isTaskActiveState,
    taskWindowMode: false, screen: "create", blockingModalVisible: () => false,
    createSources: [{ path: "/unrelated", kind: "folder" }],
    selectedCreateSourcePaths: ["/unrelated"], createSourcePickerBusy: null,
    createPassword: "unrelated-secret", createPasswordConfirmation: "unrelated-secret",
    createPasswordVisible: true, createEncryptNames: true, createEncryptionEnabled: true,
    selectedCreatePresetId: "unrelated-preset", createPresetDraftName: "Unrelated",
    createPresetMutationState: "saved", createPresetDraftTouched: false,
    activeCreateFormat: "7z", activeCreateProfile: "maximum", customCreateLevel: 9,
    customCreateLevelError: "old-error", createSplitPreset: "none", createSplitMode: "generic",
    createPresetSplitSizeBytes: null, createCustomSplitAmount: "100", createCustomSplitUnit: "mib",
    createContentPolicy: "cross_platform_clean", createExcludeText: "old-rule",
    createSfxEnabled: false, createPresetSfxTarget: "current_platform", createPresetSqzInnerFormat: "sqz",
    createDestinationBase: "source_parent", createOverwritePolicy: "rename",
    createCompletion: "none", createPostSuccess: "trash_source", createTestAfterCreate: false,
    createOptionsValidationAttempted: true, createAdvancedOpen: false, classicCreateSection: "password",
    createSuggestedDestination: null, createPreflightPhase: "blocked", createPreflightIssue: "old-error",
    pendingCreateSubmission: null, lastCreatePlan: {}, lastCreateDest: "/unrelated.zip",
    lastDiskSpace: {}, lastTempDiskSpace: {}, lastSystemTempDiskSpace: {},
    sfxCreateCapabilityReady: true,
    sfxCreateCapability: { target: "macos", available: true, extension: "app" },
    appliedDefaultCreateDir: null, fat32CompatibleSplitSizeBytes: 4294967295,
    bytesPerMiB: 1024 ** 2, bytesPerGiB: 1024 ** 3,
    tr: (_key, fallback) => fallback, platformKind: () => "macos",
    setScreen: (screen) => calls.push(["screen", screen]), dismissTaskDialog: async () => { calls.push(["dismiss"]); },
    focusCreatePrimaryAction: () => calls.push(["focus"]), showNotice: (message) => calls.push(["notice", message]),
    createPreflightBusy: () => false, createSourcesLocked: () => false, createSourcesLockedReason: () => "Busy",
    createConfigurationPending: () => false, createConfigurationPendingMessage: () => "Loading",
    normalizeUnsupportedCreatePostSuccess() {}, normalizeUnsupportedCreateCompletion() {},
    resetCreateCredentialsAfterPlan() {}, createSplitValidationMessage: () => "",
    createPasswordDataAvailable: () => model.createFormats[context.activeCreateFormat].can_encrypt_data,
    createNameEncryptionAvailable: () => model.createFormats[context.activeCreateFormat].can_encrypt_names,
    persistCreateFormat() {}, recordOperation() {}, nativeSplitKind: (format) => format === "zip" ? "zip" : null,
    createCompressionLevel: () => context.customCreateLevel,
    selectedCreateArchivePreset: () => null, createSplitSizeBytes: () => Number(context.createPresetSplitSizeBytes) || null,
    createExcludeRules: () => context.createExcludeText.split("\n"),
    resolvedPresetSfxTarget: (target) => target === "current_platform" ? "macos" : target,
    effectiveCreateTestAfterCreate: () => context.createTestAfterCreate || context.createPostSuccess === "trash_source",
    normalizedDefaultCreateDir: (value) => value, uniqueNonEmptyPaths: (inputs) => [...new Set(inputs)],
    focusBlockingTaskIfAny: () => false, createDraftExcludeCount: (draft) => draft.excludes.length,
    archiveBaseOrDefault: (value) => value || "archive",
    archiveStemName: (value) => value.replace(/\.(?:tar\.zst|tzst|zip|7z|sqz|wim|swm|app|exe|run)$/i, ""),
    joinFolderPath: (folder, name) => paths.joinDesktopPath(folder, name, "macos"),
    createFormatFilterName: (format) => format,
    getDialogModule: async () => ({ save() {}, confirm: async () => { calls.push(["confirm"]); return true; } }),
    saveNativeDialog: async (_purpose, _save, options) => { calls.push(["save", options]); return options.defaultPath; },
    inspectCreateDestinationForCreate: async (path) => {
      calls.push(["inspect", path]); return { conflict: true, guard: "fresh-authorization" };
    },
    ensureCreatePreflightListener: async () => {}, nextPreflightRequestId: () => "fresh-plan",
    ipc: {
      planCreate: async (spec) => { calls.push(["plan", spec]); return { entries: 2, deduplicated_entries: 0,
        workspace_budget_bytes: 20, system_temp_budget_bytes: 0, final_output_budget_bytes: 10 }; },
      checkDiskSpace: async (path, bytes) => { calls.push(["space", path, bytes]); return { ok: true }; },
    },
    tick: async () => {}, document: { documentElement: {}, body: {}, querySelectorAll: () => [], querySelector: () => ({ focus() {} }) },
    convertRouteHandle: null, archiveOpenStatus: "idle", archivePasswordPrompt: null, previewPasswordPrompt: null,
    taskReviewRequestGeneration: 0, pendingTaskReviewId: null,
    taskCenterOpen: true, taskCenterSelectedTaskId: 8, taskCenterFocusTaskId: 8,
    taskDialogTaskId: 8, taskDialogDismissedId: null,
    archiveUpdateReview: { cancelSourceChoice() {} }, nestedExtractDraftGeneration: 0,
    nestedExtractPickerRequest: 0, batchPickerRequest: 0, pendingArchiveTaskReview: null,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {},
    restoreTaskWorkspaceFocus: () => calls.push(["restore-focus"]),
    securitySettingsFocusPending: false, focusSecuritySettings: () => calls.push(["focus-security"]),
    currentArchive: null, sameFilePath: (left, right) => left === right,
    recoverySourceMode: "selected", recoverySourceOverride: "/unrelated/recovery.zip", recoveryPar2Override: "/unrelated/recovery.par2",
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0,
    createOutputPreviewBase: () => "archive", createSfxOutputLabel: () => "Self-extractor",
    createSfxUnavailableMessage: () => "SFX unavailable",
  };
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  context.calls = calls;
  return vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")}, context:globalThis, calls})`, context);
}

for (const state of ["failed", "cancelled"]) {
test(`reviewing a ${state} creation restores its sources and options without restarting it`, async () => {
  const run = harness();
  await run.reviewTask({ id: 8, state, spec: taskSpec() });
  const draft = run.context;
  assert.deepEqual(Array.from(draft.createSources, (source) => source.path), ["/original/reports", "/original/photos"]);
  assert.equal(draft.activeCreateFormat, "zip");
  assert.equal(draft.activeCreateProfile, "custom");
  assert.equal(draft.customCreateLevel, 4);
  assert.equal(draft.createPresetSplitSizeBytes, "123456789");
  assert.equal(draft.createSplitMode, "native");
  assert.equal(draft.createContentPolicy, "custom");
  assert.equal(draft.createExcludeText, "*.bak\ncache/**");
  assert.equal(draft.createCompletion, "reveal");
  assert.equal(draft.createPostSuccess, "keep_source");
  assert.equal(draft.createTestAfterCreate, true);
  assert.equal(draft.createSuggestedDestination, "/output/backup.zip");
  assert.equal(draft.createDestinationBase, "ask");
  assert.equal(draft.createOverwritePolicy, "ask");
  assert.equal(draft.selectedCreatePresetId, null);
  assert.equal(draft.createPresetDraftTouched, true);
  assert.equal(draft.createPassword, "");
  assert.equal(draft.createPasswordConfirmation, "");
  assert.equal(draft.createPasswordVisible, false);
  assert.equal(draft.pendingCreateSubmission, null);
  assert.equal(draft.lastCreateDest, null);
  assert.equal(draft.lastCreatePlan, null);
  assert.equal(draft.lastDiskSpace, null);
  assert.equal(draft.classicCreateSection, "general");
  assert.equal(run.calls.filter(([name]) => name === "plan").length, 0);
  assert.ok(run.calls.some(([name, screen]) => name === "screen" && screen === "create"));
});
}

for (const state of ["failed", "cancelled"]) {
test(`reviewing a ${state} encrypted ZIP requires a new password and keeps protection across edits and discarded plans`, async () => {
  const run = harness();
  await run.reviewTask({ id: 9, state, spec: taskSpec(), outputPasswordRequired: true });
  assert.equal(run.context.createEncryptionEnabled, true);
  assert.equal(run.context.createPassword, "");
  assert.equal(run.context.createEncryptNames, false);
  assert.equal(run.captureCreateRunDraft(), null);
  run.chooseCreateFormat("tar.zst");
  assert.equal(run.context.activeCreateFormat, "zip");
  run.updateCreatePassword("replacement");
  run.updateCreatePasswordConfirmation("different");
  assert.equal(run.captureCreateRunDraft(), null);
  run.updateCreatePasswordConfirmation("replacement");
  const draft = run.captureCreateRunDraft();
  assert.equal(draft.password, "replacement");
  assert.equal(draft.restoreCredentialPrompt, true);
  run.resetCreateCredentialsAfterPlan(draft);
  assert.equal(run.context.createPassword, "");
  assert.equal(run.context.createEncryptionEnabled, true);
  assert.equal(run.captureCreateRunDraft(), null);
  run.updateCreatePassword("replacement");
  run.updateCreatePassword("");
  assert.equal(run.captureCreateRunDraft(), null);
  run.updateCreateEncryptionEnabled(false);
  assert.equal(run.captureCreateRunDraft().password, null);
  run.chooseCreateFormat("tar.zst");
  assert.equal(run.context.activeCreateFormat, "tar.zst");
});
}

test("restored name encryption is not silently removed by choosing ZIP or a self-extractor", () => {
  const run = harness();
  run.restoreCreateTaskDraft(taskSpec({ dest: "/secure.7z", encrypt_names: true }), true);
  run.chooseCreateFormat("zip");
  run.updateCreateSfxEnabled(true);
  assert.equal(run.context.activeCreateFormat, "7z");
  assert.equal(run.context.createSfxEnabled, false);
  run.updateCreateEncryptNames(false);
  assert.equal(run.context.createEncryptionEnabled, true);
  assert.equal(run.captureCreateRunDraft(), null);
  run.chooseCreateFormat("zip");
  assert.equal(run.context.activeCreateFormat, "zip");
});

for (const state of ["failed", "cancelled"]) {
test(`restored ${state} creation chooses the destination again and checks sources, space and current overwrite permission`, async () => {
  const run = harness();
  const spec = taskSpec();
  await run.reviewTask({ id: 8, state, spec });
  assert.match(run.createOutputPreview(), /Confirm location when starting.*\/output\/backup.zip/);
  assert.equal(run.createArchivePreviewName(), "backup.zip");
  await run.submitCreateInputs(spec.inputs, "dialog");
  assert.deepEqual(run.calls.filter(([name]) => ["save", "inspect", "confirm", "plan", "space"].includes(name))
    .map(([name]) => name), ["save", "inspect", "confirm", "plan", "space", "space"]);
  assert.equal(run.calls.find(([name]) => name === "save")[1].defaultPath, spec.dest);
  const pending = run.context.pendingCreateSubmission;
  assert.equal(pending.spec.replacement_guard, "fresh-authorization");
  assert.equal(pending.spec.replace_existing, true);
  assert.equal(pending.spec.password, null);
  assert.equal(pending.spec.level, 4);
  assert.equal(pending.spec.split_size, 123456789);
  assert.equal(pending.spec.split_mode, "native");
  assert.equal(run.context.createPreflightPhase, "reviewing");
});
}

test("creation review respects format variants, current platform capability and an in-progress draft", async (t) => {
  for (const [dest, expected] of [["a.zip", "zip"], ["a.7Z", "7z"], ["a.sqz", "sqz"],
    ["a.tar.zst", "tar.zst"], ["a.TZST", "tar.zst"], ["a.wim", "wim"], ["a.swm", "wim"]]) {
    const run = harness();
    assert.equal(run.restoreCreateTaskDraft(taskSpec({ dest, sqz_inner_format: "7z" })), true);
    assert.equal(run.context.activeCreateFormat, expected);
    if (expected === "sqz") assert.equal(run.context.createPresetSqzInnerFormat, "7z");
  }
  const sfx = harness();
  assert.equal(sfx.restoreCreateTaskDraft(taskSpec({ dest: "/output/Installer.app", sfx_target: "macos", split_size: null })), true);
  assert.equal(sfx.context.createSfxEnabled, true);
  assert.equal(sfx.context.createPresetSfxTarget, "macos");
  assert.equal(sfx.context.activeCreateFormat, "zip");
  assert.equal(sfx.context.createSplitPreset, "none");
  assert.equal(sfx.captureCreateRunDraft().suggestedDestination, "/output/Installer.app");
  for (const setup of [
    (run) => { run.context.createSourcesLocked = () => true; },
    (run) => { run.context.createConfigurationPending = () => true; },
    (run) => { run.context.sfxCreateCapability.available = false; },
    (run) => { run.context.sfxCreateCapability.target = "windows"; },
  ]) {
    const run = harness();
    setup(run);
    run.reviewTask({ id: 8, state: "failed", spec: taskSpec({ sfx_target: "macos" }) });
    assert.equal(run.context.createSources[0].path, "/unrelated");
    assert.equal(run.context.lastCreateDest, "/unrelated.zip");
    assert.equal(run.calls.some(([name]) => name === "screen" || name === "dismiss"), false);
  }
  const unsupported = harness();
  assert.equal(unsupported.restoreCreateTaskDraft(taskSpec({ dest: "/output/backup.tar" })), false);
  assert.equal(unsupported.context.createSources[0].path, "/unrelated");

  const run = harness({ navigation: true });
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
    const captured = run.captureCreateRunDraft();
    assert.ok(captured);
    const { password, ...options } = captured;
    return plain({ sources: run.context.createSources,
      selectedSources: run.context.selectedCreateSourcePaths, preset: run.context.selectedCreatePresetId,
      touched: run.context.createPresetDraftTouched, advanced: run.context.createAdvancedOpen,
      section: run.context.classicCreateSection, hasPassword: Boolean(password), options });
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
    assert.equal(run.context.recoverySourceOverride, "/unrelated/recovery.zip");
    assert.equal(run.context.recoveryPar2Override, "/unrelated/recovery.par2");
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
  assert.deepEqual(Array.from(run.context.createSources, (source) => source.path), taskSpec().inputs);
  assert.equal(session.surface("modern").preflight.phase, "idle");
  assert.equal(session.surface("modern").review, null);
  assert.equal(run.calls.filter(([name]) => name === "focus").length, 1);
  assert.equal(run.calls.filter(([name]) => name === "convert-submit").length, 1);
  assert.equal(run.calls.some(([name]) => name === "plan"), false);

  run.context.createPreflightPhase = "submitting";
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
    assert.equal(run.context.createPreflightPhase, "submitting");
    assert.equal(run.context.recoverySourceOverride, "/unrelated/recovery.zip");
    assert.equal(run.context.recoveryPar2Override, "/unrelated/recovery.par2");
    assert.equal(run.context.securitySettingsFocusPending, false);
    assert.equal(run.context.taskCenterOpen, true);
    assert.equal(run.context.taskCenterSelectedTaskId, 8);
    assert.equal(run.context.taskCenterFocusTaskId, 8);
    assert.equal(run.context.taskDialogTaskId, 8);
    assert.equal(run.context.taskDialogDismissedId, null);
    assert.equal(run.calls.slice(callsBeforeReview).some(([name]) => ["focus", "focus-security", "restore-focus", "convert-submit"].includes(name)), false);
    assert.match(run.calls.at(-1)[1], /finishes adding this create task/);
  }
});

test("edited formats adapt the original output suggestion and cancelled selection never creates a plan", async () => {
  const run = harness();
  run.restoreCreateTaskDraft(taskSpec({ dest: "/output/my.backup.TZST", split_size: null }));
  assert.equal(run.createSuggestedOutputPath("tar.zst", "tar.zst"), "/output/my.backup.TZST");
  run.context.activeCreateFormat = "7z";
  assert.equal(run.captureCreateRunDraft().suggestedDestination, "/output/my.backup.7z");
  assert.equal(run.createSuggestedOutputPath("wim", "swm"), "/output/my.backup.swm");
  run.context.saveNativeDialog = async () => null;
  run.context.finishCreatePreflightWithIssue = (...args) => run.calls.push(["issue", ...args]);
  await run.submitCreateInputs(["/original/reports"], "dialog");
  assert.equal(run.calls.some(([name]) => name === "inspect" || name === "plan"), false);
  assert.equal(run.context.pendingCreateSubmission, null);
  assert.equal(run.calls.find(([name]) => name === "issue")[3], "cancelled");
});
