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
const { taskReviewScreen, isTaskActiveState, applyCreateDestinationAuthorization } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");

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

function harness({ navigation = false, preparation = false, preview = false } = {}) {
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
    "resetCreateCredentialsAfterPlan", "preventCreateSubmissionNavigation", "preventConvertSubmissionNavigation", "preventTaskWorkspaceNavigation", "dismissRecoveryPreparation",
    "isCurrentCreateSourcePicker", "isCurrentCreateOutputPreparation", "dismissCreatePreparation",
    ...(preparation ? ["submitCreateJob", "appendCreateSources", "showCreateSourcesAdded", "clearCreateSources",
      "createPreflightBusy", "createSourcesLocked", "createDestinationInspectionCancellable",
      "inspectCreateDestinationForCreate", "createDestinationInspectionCancelled", "cancelCreateDestinationInspection",
      "finishCreatePreflightWithIssue", "discardPendingCreatePlan", "cancelCreatePlanReview",
      "refreshConfirmedCreateDestination", "confirmCreatePlan", "commonCreateSourceParent", "setMode",
      "applyCreatePreset", "archivePresetById", "isCreateFormatId", "createPrimaryAction", "focusCreatePrimaryAction"] : []),
    ...(navigation ? ["setScreen", "dismissTaskDialog", "closeTaskCenter", "cancelTaskReview", "adoptRecoveryTargetFromTask"] : [])];
  const declarations = source.statements.filter((node) =>
    (ts.isFunctionDeclaration(node) && names.includes(node.name?.text))
    || (ts.isClassDeclaration(node) && node.name?.text === "CreateDestinationInspectionError"));
  const preparationEffect = preparation ? source.statements.find((node) =>
    ts.isExpressionStatement(node) && ts.isCallExpression(node.expression)
    && node.expression.expression.getText(source) === "$effect"
    && node.expression.arguments[0]?.getText(source).includes("isCurrentCreateSourcePicker")) : null;
  const previewDeclaration = source.statements.find((node) => ts.isVariableStatement(node)
    && node.declarationList.declarations.some((declaration) =>
      ts.isIdentifier(declaration.name) && declaration.name.text === "previewDestinationRequestId"));
  const calls = [];
  const context = {
    ...model, ...paths, ...sources, taskReviewScreen, isTaskActiveState, applyCreateDestinationAuthorization,
    taskWindowMode: false, screen: "create", mode: "modern", blockingModalVisible: () => false,
    runtimePreviews: { preflightDestinationBytes: preview ? 1024 : 0 },
    createSources: [{ path: "/unrelated", kind: "folder" }],
    selectedCreateSourcePaths: ["/unrelated"],
    createSourcePicker: null, createOutputPreparation: null, createPreflightClosed: false,
    createPreflightScanned: 0, createPreflightCurrent: "", createPreflightRequestId: null,
    createPreflightRequestKind: null, createPreflightProcessedBytes: 0, createPreflightCancelPending: false,
    createPreflightIssueStage: null,
    createPrimaryFocusPending: false,
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
    dismissArchiveAddPreparation() {},
    restoreTaskWorkspaceFocus: () => calls.push(["restore-focus"]),
    securitySettingsFocusPending: false, focusSecuritySettings: () => calls.push(["focus-security"]),
    currentArchive: null, sameFilePath: (left, right) => left === right,
    recoverySourceMode: "selected", recoverySourceOverride: "/unrelated/recovery.zip", recoveryPar2Override: "/unrelated/recovery.par2",
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0, recoveryOutputPreparation: null,
    createOutputPreviewBase: () => "archive", createSfxOutputLabel: () => "Self-extractor",
    createSfxUnavailableMessage: () => "SFX unavailable",
    isErrorDto: (error) => Boolean(error && typeof error === "object" && typeof error.key === "string"),
    tError: (error) => error.key, formatBytes: (value) => `${value} B`,
    createProfileLabel: (profile) => profile,
    pathBaseName: (path) => paths.desktopBasename(path, "macos"),
    submitJob: async (spec) => { calls.push(["submit", spec]); return 42; },
    isJobSubmitBlocked: () => false, taskCenterReturnFocus: null,
    HTMLElement: class {}, createPrimaryAction: () => ({ focus() { calls.push(["primary-focus"]); } }),
    trackAppearanceSave() {}, persistUiMode: (next) => { context.mode = next; return Promise.resolve(); },
    presetDocument: null,
  };
  if (preparation) {
    context.document.querySelector = () => ({ focus() { calls.push(["review-focus"]); } });
    context.document.getElementById = () => ({ focus() { calls.push(["primary-focus"]); } });
  }
  Object.defineProperty(context, "createSourceInputs", {
    get: () => sources.createSourcePaths(context.createSources),
  });
  const effectDeclaration = preparationEffect
    ? `const runCreatePreparationEffects = ${preparationEffect.expression.arguments[0].getText(source)};` : "";
  const { outputText } = ts.transpileModule(`${previewDeclaration.getText(source)}\n${declarations.map((node) => node.getText(source)).join("\n")}\n${effectDeclaration}`
    .replaceAll("import.meta.env.DEV", String(preview)), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  context.calls = calls;
  return vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")}${preparationEffect ? ",runCreatePreparationEffects" : ""}, previewDestinationRequestId, context:globalThis, calls})`, context);
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
  assert.equal(draft.createCompletion, "reveal_output");
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

function creationPreparation({ stop = null, pauseNewPlan = false, automaticDestination = false } = {}) {
  const run = harness({ navigation: true, preparation: true });
  run.context.taskCenterOpen = false;
  run.restoreCreateTaskDraft(taskSpec({ split_size: null }));
  if (automaticDestination) run.context.createDestinationBase = "source_parent";
  const blocked = deferred();
  const newerPlan = deferred();
  let hit = false;
  let newerPlanHit = false;
  let request = 0;
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
  run.context.ensureCreatePreflightListener = () => invoke(
    run.context.createPreflightPhase === "measuring" ? "source-listener" : "destination-listener",
    undefined,
  );
  run.context.nextPreflightRequestId = () => `request-${++request}`;
  run.context.ipc = {
    inspectCreateDestination: (path, split, requestId) => invoke("inspection", {
      conflict: true, guard: "current-output",
    }, { path, split, requestId }),
    cancelCreateDestinationInspection: (requestId) => invoke("cancel-inspection", undefined, requestId),
    uniqueCreateDestination: (path) => invoke("unique-name", path, path),
    planCreate: (spec, requestId) => invoke("plan", plan(spec), { ...spec, requestId }),
    tempDir: () => invoke("system-temp-directory", "/system-temporary"),
    checkDiskSpace: (path, bytes) => invoke(bytes === 20 ? "workspace-space"
      : bytes === 30 ? "system-temp-space" : "destination-space", { ok: true, available_bytes: 10000 }, { path, bytes }),
  };
  run.context.tick = () => run.context.screen === "create" && run.context.createPreflightPhase === "reviewing"
    ? invoke("review-tick", undefined) : run.context.createPrimaryFocusPending
      ? invoke("primary-focus-tick", undefined) : Promise.resolve();
  return { run, blocked, newerPlan, plan, reached: () => hit, newPlanReached: () => newerPlanHit };
}

function preparationState(run) {
  return {
    owner: run.context.createOutputPreparation,
    picker: run.context.createSourcePicker,
    phase: run.context.createPreflightPhase,
    requestId: run.context.createPreflightRequestId,
    requestKind: run.context.createPreflightRequestKind,
    scanned: run.context.createPreflightScanned,
    processedBytes: run.context.createPreflightProcessedBytes,
    cancelling: run.context.createPreflightCancelPending,
    primaryFocusPending: run.context.createPrimaryFocusPending,
    current: run.context.createPreflightCurrent,
    issue: run.context.createPreflightIssue,
    issueStage: run.context.createPreflightIssueStage,
    plan: run.context.lastCreatePlan,
    dest: run.context.lastCreateDest,
    workspace: run.context.lastTempDiskSpace,
    systemTemp: run.context.lastSystemTempDiskSpace,
    disk: run.context.lastDiskSpace,
    pending: run.context.pendingCreateSubmission,
    noticeCount: run.calls.filter(([name]) => name === "notice").length,
    focusCount: run.calls.filter(([name]) => name === "review-focus" || name === "focus" || name === "primary-focus").length,
  };
}

test("create preparation abandons every late boundary and preserves a newer preparation after leaving and returning", async () => {
  const stages = ["dialog-module", "save", "destination-listener", "inspection", "confirm",
    "source-listener", "plan", "workspace-space", "system-temp-directory", "system-temp-space",
    "destination-space", "review-tick", "unique-name"];
  for (const stage of stages) {
    for (const rejects of stage === "review-tick" ? [false] : [false, true]) {
      const { run, blocked, newerPlan, plan, reached, newPlanReached } = creationPreparation({
        stop: stage, pauseNewPlan: true, automaticDestination: stage === "unique-name",
      });
      const first = run.submitCreateInputs(taskSpec().inputs, "dialog");
      await waitFor(reached, stage);
      run.setScreen("browse");
      assert.equal(run.context.screen, "browse", stage);
      assert.equal(run.context.createOutputPreparation, null, stage);
      assert.equal(run.context.pendingCreateSubmission, null, stage);
      run.setScreen("create");
      assert.equal(run.restoreCreateTaskDraft(taskSpec({ inputs: ["/new/source.txt"],
        dest: "/new-output/new.zip", level: 6, split_size: null })), true);
      const second = run.submitCreateInputs(["/new/source.txt"], "dialog");
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
      assert.equal(run.context.createPreflightPhase, "reviewing", stage);
      assert.equal(run.context.createOutputPreparation, null, stage);
      assert.deepEqual(Array.from(run.context.pendingCreateSubmission.spec.inputs), ["/new/source.txt"], stage);
      assert.equal(run.context.pendingCreateSubmission.spec.level, 6, stage);
      assert.equal(run.context.pendingCreateSubmission.spec.dest, "/new-output/new.zip", stage);
    }
  }
  for (const change of ["mode", "source"]) {
    const { run, blocked, reached } = creationPreparation({ stop: "plan" });
    const preparing = run.submitCreateInputs(taskSpec().inputs, "dialog");
    await waitFor(reached, "source plan before context invalidation");
    if (change === "mode") run.setMode("classic");
    else run.appendCreateSources(["/new/source.txt"], "file");
    run.runCreatePreparationEffects();
    assert.equal(run.context.createOutputPreparation, null, change);
    assert.equal(run.context.createPreflightPhase, "idle", change);
    assert.equal(run.createPreflightBusy(), false, change);
    const retained = preparationState(run);
    blocked.resolve(blocked.value);
    await preparing;
    assert.deepEqual(preparationState(run), retained, `${change} invalidation cannot become a review`);
  }
});

test("create source choosing retains new sources, ignores stale errors and clears only its own picker", async () => {
  for (const stage of ["dialog-module", "open"]) {
    for (const rejects of [false, true]) {
      const { run, blocked, reached } = creationPreparation({ stop: stage });
      const first = run.submitCreateJob("files");
      await waitFor(reached, stage);
      run.setScreen("browse");
      assert.equal(run.context.createSourcePicker, null);
      run.setScreen("create");
      run.restoreCreateTaskDraft(taskSpec({ inputs: ["/new/source.txt"], dest: "/new-output/new.zip", split_size: null }));
      const newer = deferred();
      let newerOpened = false;
      run.context.openNativeDialog = () => { newerOpened = true; return newer.promise; };
      const second = run.submitCreateJob("folder");
      await waitFor(() => newerOpened, "new source chooser");
      const picker = run.context.createSourcePicker;
      const notices = run.calls.filter(([name]) => name === "notice").length;
      const calls = run.calls.length;
      if (rejects) blocked.reject(new Error("Old source chooser failed"));
      else blocked.resolve(blocked.value);
      await first;
      assert.equal(run.context.createSourcePicker, picker);
      assert.deepEqual(Array.from(run.context.createSources, (source) => source.path), ["/new/source.txt"]);
      assert.equal(run.calls.filter(([name]) => name === "notice").length, notices);
      assert.equal(run.calls.length, calls);
      newer.resolve(["/new/folder"]);
      await second;
      assert.equal(run.context.createSourcePicker, null);
      assert.deepEqual(Array.from(run.context.createSources, (source) => source.path), ["/new/source.txt", "/new/folder"]);
      assert.equal(run.context.createSources[1].kind, "folder");
    }
  }

  const { run } = creationPreparation();
  const sourcesBeforeCancel = run.context.createSources;
  run.context.openNativeDialog = async () => null;
  await run.submitCreateJob("files");
  assert.equal(run.context.createSources, sourcesBeforeCancel);
  assert.equal(run.context.createSourcePicker, null);
  assert.match(run.calls.at(-1)[1], /Source selection cancelled/);

  for (const change of ["mode", "source", "preset"]) {
    const { run: pending, blocked, reached } = creationPreparation({ stop: "open" });
    const choosing = pending.submitCreateJob("files");
    await waitFor(reached, "source chooser before a draft change");
    const picker = pending.context.createSourcePicker;
    const sourceRoots = pending.context.createSources;
    pending.appendCreateSources(taskSpec().inputs, "unknown");
    assert.equal(pending.context.createSources, sourceRoots, "unchanged roots keep their identity");
    assert.equal(pending.context.createSourcePicker, picker, "a duplicate source is not a new intent");
    assert.equal(pending.restoreCreateTaskDraft(taskSpec({ dest: "/unsupported.tar" })), false);
    assert.equal(pending.context.createSourcePicker, picker, "a blocked review keeps its owner");

    pending.context.presetDocument = { presets: [{ id: "invalid", kind: "create", label: "Invalid",
      options: { format: "rar", output: { kind: "archive" } } },
    { id: "replacement", kind: "create", label: "Replacement", options: {
      format: "zip", level: 2, credential: { kind: "none" }, encrypt_names: false,
      volumes: { kind: "single" }, content_policy: "custom", excludes: ["*.tmp"],
      output: { kind: "archive" }, format_options: { kind: "none" },
      destination: { base: "ask", existing_output: "ask" }, completion: "none", post_success: "keep_source", test_after_create: false,
    } }] };
    pending.applyCreatePreset("invalid", false);
    assert.equal(pending.context.createSourcePicker, picker, "an unavailable preset keeps its owner");
    if (change === "mode") {
      pending.setMode("classic");
      pending.runCreatePreparationEffects();
    } else if (change === "source") {
      pending.appendCreateSources(["/new/source.txt"], "file");
      pending.runCreatePreparationEffects();
    } else {
      pending.applyCreatePreset("replacement", false);
      assert.equal(pending.context.customCreateLevel, 2);
      assert.equal(pending.context.selectedCreatePresetId, "replacement");
    }
    assert.equal(pending.context.createSourcePicker, null, `${change} ends the old chooser`);
    const retainedRoots = pending.context.createSources;
    const retained = preparationState(pending);
    blocked.resolve(blocked.value);
    await choosing;
    assert.equal(pending.context.createSources, retainedRoots, `${change} ignores the old picked source`);
    assert.deepEqual(preparationState(pending), retained, `${change} keeps the new draft and feedback`);
  }
});

test("same-page creation completes all checks, cancels cleanly and retains an in-flight submission when navigation is requested", async () => {
  const { run } = creationPreparation();
  const firstDraft = run.captureCreateRunDraft();
  await run.submitCreateInputs(taskSpec().inputs, "dialog");
  assert.deepEqual(run.calls.filter(([name]) => ["save", "inspection", "confirm", "plan", "workspace-space",
    "system-temp-directory", "system-temp-space", "destination-space"].includes(name)).map(([name]) => name),
  ["save", "inspection", "confirm", "plan", "workspace-space", "system-temp-directory", "system-temp-space", "destination-space"]);
  assert.equal(run.context.createPreflightPhase, "reviewing");
  assert.equal(run.context.lastCreatePlan.entries, 2);
  assert.equal(run.context.createPreflightScanned, 3);
  assert.equal(run.context.pendingCreateSubmission.spec.level, firstDraft.level);
  assert.equal(run.context.pendingCreateSubmission.spec.replacement_guard, "current-output");
  assert.equal(run.context.createOutputPreparation, null);
  assert.equal(run.calls.filter(([name]) => name === "review-focus").length, 1);
  run.cancelCreatePlanReview();
  assert.equal(run.context.pendingCreateSubmission, null);
  assert.equal(run.context.lastCreatePlan, null);
  assert.equal(run.context.createPreflightPhase, "idle");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);

  run.context.saveNativeDialog = async () => null;
  await run.submitCreateInputs(taskSpec().inputs, "dialog");
  assert.equal(run.context.createPreflightPhase, "cancelled");
  assert.equal(run.context.createOutputPreparation, null);
  assert.equal(run.context.pendingCreateSubmission, null);
  run.context.saveNativeDialog = async (_purpose, _save, options) => options.defaultPath;
  await run.submitCreateInputs(taskSpec().inputs, "dialog");
  const pending = run.context.pendingCreateSubmission;
  const submission = deferred();
  let submitted;
  run.context.submitJob = (spec) => { submitted = spec; return submission.promise; };
  const confirming = run.confirmCreatePlan();
  await waitFor(() => Boolean(submitted), "queue submission");
  run.setScreen("browse");
  assert.equal(run.context.screen, "create");
  assert.equal(run.context.pendingCreateSubmission, pending);
  assert.equal(run.context.createPreflightPhase, "submitting");
  assert.match(run.calls.at(-1)[1], /finishes adding this create task/);
  submission.resolve(42);
  await confirming;
  assert.equal(run.context.createPreflightPhase, "ready");
  assert.equal(run.context.pendingCreateSubmission, null);
  assert.equal(run.context.createSources.length, 0);
  assert.deepEqual(JSON.parse(JSON.stringify(submitted)), taskSpec({ split_size: null, split_mode: "generic",
    replacement_guard: "current-output" }));

  const cancelled = creationPreparation({ stop: "inspection" });
  const checking = cancelled.run.submitCreateInputs(taskSpec().inputs, "dialog");
  await waitFor(cancelled.reached, "cancellable output inspection");
  await cancelled.run.cancelCreateDestinationInspection();
  assert.equal(cancelled.run.context.createPreflightCancelPending, true);
  cancelled.blocked.resolve(cancelled.blocked.value);
  await checking;
  assert.equal(cancelled.run.context.createPreflightPhase, "cancelled");
  assert.equal(cancelled.run.context.createOutputPreparation, null);
  assert.equal(cancelled.run.context.pendingCreateSubmission, null);
  assert.equal(cancelled.run.context.createPreflightRequestId, null);
  assert.equal(cancelled.run.context.createPreflightCancelPending, false);
  assert.equal(cancelled.run.calls.some(([name]) => name === "plan" || name === "submit"), false);
  assert.match(cancelled.run.calls.findLast(([name]) => name === "notice")[1], /Output check cancelled/);
  assert.equal(cancelled.run.calls.filter(([name]) => name === "primary-focus").length, 1);

  const delayedFocus = creationPreparation({ stop: "primary-focus-tick", pauseNewPlan: true });
  const inspection = deferred();
  const inspectCurrent = delayedFocus.run.context.ipc.inspectCreateDestination;
  let firstInspection = true;
  delayedFocus.run.context.ipc.inspectCreateDestination = (...args) => {
    if (!firstInspection) return inspectCurrent(...args);
    firstInspection = false;
    return inspection.promise;
  };
  const oldPreparation = delayedFocus.run.submitCreateInputs(taskSpec().inputs, "dialog");
  await waitFor(() => delayedFocus.run.context.createPreflightRequestKind === "destination", "inspection before focus cancellation");
  await delayedFocus.run.cancelCreateDestinationInspection();
  inspection.resolve({ conflict: true, guard: "current-output" });
  await waitFor(delayedFocus.reached, "cancelled inspection focus tick");
  delayedFocus.run.setScreen("browse");
  delayedFocus.run.setScreen("create");
  delayedFocus.run.restoreCreateTaskDraft(taskSpec({ inputs: ["/new/source.txt"],
    dest: "/new-output/new.zip", split_size: null }));
  const newPreparation = delayedFocus.run.submitCreateInputs(["/new/source.txt"], "dialog");
  await waitFor(delayedFocus.newPlanReached, "new preparation before the old focus tick");
  const current = preparationState(delayedFocus.run);
  delayedFocus.blocked.resolve();
  await oldPreparation;
  assert.deepEqual(preparationState(delayedFocus.run), current);
  assert.equal(delayedFocus.run.calls.some(([name]) => name === "primary-focus"), false);
  const newSpec = delayedFocus.run.calls.findLast(([name]) => name === "plan")[1];
  delayedFocus.newerPlan.resolve(delayedFocus.plan(newSpec));
  await newPreparation;
  assert.equal(delayedFocus.run.context.createPreflightPhase, "reviewing");
  assert.deepEqual(Array.from(delayedFocus.run.context.pendingCreateSubmission.spec.inputs), ["/new/source.txt"]);

  for (const leaveAndReturn of [false, true]) {
    const preview = harness({ navigation: true, preparation: true, preview: true });
    const timer = deferred();
    preview.context.window = { setTimeout(callback, delay) {
      assert.equal(delay, 180);
      preview.calls.push(["preview-cancel-timer", delay]);
      timer.promise.then(callback);
      return 1;
    } };
    preview.context.createPreflightPhase = "choosingDest";
    preview.context.createPreflightRequestId = preview.previewDestinationRequestId;
    preview.context.createPreflightRequestKind = "destination";
    preview.context.createPreflightProcessedBytes = 1024;
    preview.context.createPreflightIssue = "";
    preview.context.lastCreatePlan = null;
    preview.context.lastCreateDest = null;
    preview.context.lastDiskSpace = null;
    preview.context.lastTempDiskSpace = null;
    preview.context.lastSystemTempDiskSpace = null;
    const sources = preview.context.createSources;
    const mode = preview.context.mode;
    const cancelling = preview.cancelCreateDestinationInspection();
    assert.equal(preview.calls.filter(([name]) => name === "preview-cancel-timer").length, 1);
    assert.equal(preview.context.createPreflightCancelPending, true);
    assert.equal(preview.calls.some(([name]) => name === "primary-focus" || name === "notice"), false);
    if (leaveAndReturn) {
      preview.setScreen("browse");
      preview.setScreen("create");
      assert.equal(preview.context.screen, "create");
      assert.equal(preview.context.mode, mode);
      assert.equal(preview.context.createSources, sources);
      assert.equal(preview.context.createOutputPreparation, null);
      const retained = preparationState(preview);
      const calls = preview.calls.length;
      timer.resolve();
      await cancelling;
      assert.deepEqual(preparationState(preview), retained, "returning to the same page, mode and sources cannot revive a preview cancellation");
      assert.equal(preview.calls.length, calls);
      assert.equal(preview.context.createPreflightPhase, "idle");
      assert.equal(preview.createPreflightBusy(), false);
    } else {
      timer.resolve();
      await cancelling;
      assert.equal(preview.context.createPreflightPhase, "cancelled");
      assert.equal(preview.context.createOutputPreparation, null);
      assert.equal(preview.context.createPreflightRequestId, null);
      assert.equal(preview.context.createPreflightCancelPending, false);
      assert.equal(preview.context.createSources, sources);
      assert.equal(preview.calls.filter(([name]) => name === "primary-focus").length, 1);
      assert.match(preview.calls.findLast(([name]) => name === "notice")[1], /Output check cancelled/);
    }
  }
});
