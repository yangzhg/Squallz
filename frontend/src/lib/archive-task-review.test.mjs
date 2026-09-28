import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");

function spec(overrides = {}) {
  return { kind: "extract", path: "/original/photos.zip", dest: "/original/output",
    expected_destination: "/old/checked", expected_input_guard: "old-guard",
    selection: ["photos/", "notes.txt"], overwrite: "rename", symlinks: "skip",
    smart: true, encoding: null, password: "redacted-in-real-task", verify_sfx: false,
    best_effort: false, ...overrides };
}

function archive(path = "/original/photos.zip", id = 1, encoding = null) {
  return { id, path, source: path, name: path.split("/").at(-1), encoding_override: encoding };
}

function harness() {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1],
    ts.ScriptTarget.Latest, true);
  const names = ["reviewTask", "reviewExtractTask", "reviewConvertTask", "reviewArchiveTask", "restoreExtractTaskDraft", "finishOpenedArchive",
    "openArchivePath", "extractJobPaths", "extractJobDestination", "extractSmartBase",
    "extractSelectionLabel", "extractStartBlockedReason", "submitExtractJob",
    "syncExtractDraftArchive", "cancelPasswordRequest", "submitPasswordRequest",
    "setScreen", "setScreenRespectingJobQuestion", "effectiveExtractDest", "sameFolderExtractDest",
    "extractEncodingForJob", "archiveEncodingForJob", "extractEncodingLabel"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = {
    calls, taskReviewScreen, taskWindowMode: false, currentArchive: archive(),
    screen: "browse", archiveOpenStatus: "idle", archiveOpenGeneration: 0,
    extractReviewFocusPending: false, convertReviewFocusPending: false, createPrimaryFocusPending: false,
    pendingCreateSubmission: null, classicCreateSection: "general",
    pendingArchiveTaskReview: null, extractDraftArchive: { id: 1, source: "/original/photos.zip" },
    extractScope: "all", extractSelectionSnapshot: [], extractCustomDest: "/unrelated/output",
    extractSmartBaseOverride: null, extractVerifySfx: false,
    extractDestinationMode: "same", extractOverwriteMode: "overwrite", extractSymlinkMode: "follow",
    extractPresetEncodingLabel: "gbk", selectedExtractPresetId: "old-preset", extractPresetDraftName: "Old",
    extractPresetMutationState: "saved", extractPresetDraftTouched: false,
    extractPlan: { destination: "/old/checked", input_guard: "old-guard" }, extractPlanPhase: "ready",
    extractPlanRequestKey: "old-plan", appliedDefaultExtractDir: "/unrelated/default",
    jobPasswordPrompt: null, jobConflictPrompt: null, archivePasswordPrompt: null, jobPasswordValue: "",
    passwordSubmissionAttempted: false, standalonePasswordFocusedInput: null,
    recoverySourceMode: "current", recoverySourceOverride: null, recoveryPar2Override: null,
    tr: (_key, fallback) => fallback,
    sameFilePath: (a, b) => a === b, pathBaseName: (path) => path.split("/").at(-1),
    pathDir: (path) => path.slice(0, path.lastIndexOf("/")), normalizedDefaultExtractDir: (value) => value,
    defaultExtractDest: () => "/unrelated/default/photos",
    extractArchiveRequiredReason: () => context.currentArchive ? "" : "Open an archive",
    extractSpaceFailureLabel: () => "Insufficient space", extractPlanErrorLabel: () => "Preview failed",
    showNotice: (message) => calls.push(["notice", message]),
    syncUrl() {}, tick: async () => {},
    document: { documentElement: {}, body: {}, querySelectorAll: () => [] },
    dismissTaskDialog: async () => { calls.push(["dismiss"]); },
    focusBlockingTaskIfAny: () => false, preventCreateSubmissionNavigation: () => false,
    preventConvertSubmissionNavigation: () => false,
    markExtractPresetDraftTouched: () => { context.extractPresetDraftTouched = true; },
    resetExtractPlanRequestState: () => { context.extractPlan = null; context.extractPlanPhase = "idle"; calls.push(["reset"]); },
    focusExtractReview: () => calls.push(["focus"]),
    isPar2Path: () => false, openPasswordPrompt: () => context.archivePasswordPrompt,
    archiveOpenError: () => null, archiveOpenFailureNotice: () => "Could not open archive",
    openArchiveStore: async (path, password, encoding) => {
      calls.push(["open", path, password, encoding]); context.currentArchive = archive(path, 2, encoding); return true;
    },
    cancelArchivePasswordPrompt: () => { context.archivePasswordPrompt = null; },
    rememberRecent() {}, recordOperation() {}, clearEntryPreviewState() {}, recordValidationRenderReady() {},
    extractPlanKey: (...parts) => JSON.stringify(parts),
    requestExtractPlan: async (...parts) => {
      calls.push(["plan", ...parts]); context.extractPlanRequestKey = JSON.stringify(parts);
      context.extractPlan = { destination: "/original/output/photos", input_guard: "fresh-guard", entries: 3 };
      context.extractPlanPhase = "ready";
    },
    submitCurrentArchiveJob: async (job) => { calls.push(["submit", job]); return true; },
    archiveTitle: () => context.currentArchive?.name, taskPasswordReady: (value) => Boolean(value),
    adoptRecoveryTargetFromTask: (task) => calls.push(["recovery", task.spec.path]),
  };
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")},context:globalThis,calls})`,context);
}

test("failed extraction review restores its selection and policies instead of reusing the current draft", async () => {
  const run = harness();
  await run.reviewTask({ id: 8, state: "failed", spec: spec() });
  await Promise.resolve();
  assert.deepEqual(Array.from(run.extractJobPaths() ?? []), ["photos/", "notes.txt"]);
  assert.equal(run.extractJobDestination(), "/original/output");
  assert.equal(run.context.extractDestinationMode, "smart");
  assert.equal(run.context.extractOverwriteMode, "rename");
  assert.equal(run.context.extractSymlinkMode, "skip");
  assert.equal(run.context.extractPresetEncodingLabel, null);
  assert.equal(run.context.selectedExtractPresetId, null);
  assert.equal(run.context.extractPresetDraftTouched, true);
  assert.equal(run.context.extractPlan, null);
  assert.equal(run.context.screen, "extract");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  run.syncExtractDraftArchive();
  assert.deepEqual(Array.from(run.extractJobPaths()), ["photos/", "notes.txt"]);
  await run.submitExtractJob();
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.equal(submitted.dest, "/original/output");
  assert.equal(submitted.expected_destination, "/original/output/photos");
  assert.equal(submitted.expected_input_guard, "fresh-guard");
  assert.equal(submitted.password, null);
  assert.equal(submitted.overwrite, "rename");
  assert.equal(submitted.symlinks, "skip");
  assert.deepEqual(Array.from(submitted.selection), ["photos/", "notes.txt"]);
});

test("review reopens the original archive with its encoding and preserves direct output and SFX verification", async () => {
  const run = harness();
  run.context.currentArchive = archive("/unrelated.zip");
  const original = spec({ path: "/original/Installer.app", smart: false, selection: null, encoding: "gbk", verify_sfx: true });
  await run.reviewTask({ id: 8, state: "failed", spec: original });
  assert.deepEqual(run.calls.find(([name]) => name === "open"), ["open", original.path, null, "gbk"]);
  assert.equal(run.extractEncodingLabel(), "GBK");
  assert.equal(run.context.extractDestinationMode, "choose");
  assert.equal(run.extractJobDestination(), original.dest);
  assert.equal(run.extractJobPaths(), null);
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  await run.submitExtractJob();
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.equal(submitted.path, original.path);
  assert.equal(submitted.encoding, "gbk");
  assert.equal(submitted.verify_sfx, true);
  assert.equal(submitted.password, null);
  assert.equal(submitted.selection, null);
  assert.equal(submitted.smart, false);
});

test("unlocking a failed task retries the password then restores only its non-sensitive draft", async () => {
  const run = harness();
  run.context.currentArchive = archive("/unrelated.zip");
  run.context.openArchiveStore = async (path, password, encoding) => {
    if (password !== "correct") {
      run.context.archivePasswordPrompt = { path, encoding, wrong: Boolean(password) };
      return false;
    }
    run.context.currentArchive = archive(path, 2, encoding);
    run.context.archivePasswordPrompt = null;
    return true;
  };
  await run.reviewTask({ id: 8, state: "failed", spec: spec({ encoding: "gbk" }) });
  assert.equal(run.context.screen, "password");
  assert.equal(run.context.extractCustomDest, "/unrelated/output");
  assert.equal("password" in run.context.pendingArchiveTaskReview, false);
  assert.equal("expected_input_guard" in run.context.pendingArchiveTaskReview, false);
  assert.equal("expected_destination" in run.context.pendingArchiveTaskReview, false);
  run.context.jobPasswordValue = "wrong";
  await run.submitPasswordRequest();
  assert.equal(run.context.screen, "password");
  assert.ok(run.context.pendingArchiveTaskReview);
  assert.equal(run.context.jobPasswordValue, "");
  run.context.jobPasswordValue = "correct";
  await run.submitPasswordRequest();
  assert.equal(run.context.screen, "extract");
  assert.equal(run.extractJobDestination(), "/original/output");
  assert.deepEqual(Array.from(run.extractJobPaths()), ["photos/", "notes.txt"]);
  assert.equal(run.context.pendingArchiveTaskReview, null);
  assert.equal(run.context.jobPasswordValue, "");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
});

test("a different decoding reopens the same archive, while a same-source reload retains the verification requirement", async () => {
  const run = harness();
  run.context.currentArchive = archive("/original/photos.zip", 1, "gbk");
  await run.reviewTask({ id: 8, state: "failed", spec: spec({ verify_sfx: true }) });
  assert.deepEqual(run.calls.find(([name]) => name === "open"), ["open", "/original/photos.zip", null, null]);
  assert.equal(run.extractEncodingForJob(), null);
  run.context.currentArchive = archive("/original/photos.zip", 3, "shift_jis");
  run.syncExtractDraftArchive();
  assert.equal(run.context.extractVerifySfx, true);
  assert.equal(run.extractJobDestination(), "/original/output");
  assert.deepEqual(Array.from(run.extractJobPaths()), []);
  run.context.currentArchive = archive("/another.zip", 4);
  run.syncExtractDraftArchive();
  assert.equal(run.context.extractVerifySfx, false);
  assert.equal(run.context.extractSmartBaseOverride, null);
});

test("cancelling, navigating away, or opening another archive drops a pending extraction review", async () => {
  for (const action of ["cancel", "navigate", "open"]) {
    const run = harness();
    run.context.screen = "password";
    run.context.archivePasswordPrompt = { path: "/original/photos.zip", encoding: null };
    run.context.pendingArchiveTaskReview = { path: spec().path, encoding: null, restore: () => true };
    if (action === "cancel") run.cancelPasswordRequest();
    if (action === "navigate") run.setScreen("extract");
    if (action === "open") await run.openArchivePath("/another.zip", "open-file");
    assert.equal(run.context.pendingArchiveTaskReview, null);
    assert.equal(run.context.extractCustomDest, "/unrelated/output");
  }
  const run = harness();
  run.context.currentArchive = archive("/unrelated.zip");
  let finishOpen;
  let openStarted;
  const opening = new Promise((resolve) => { openStarted = resolve; });
  run.context.openArchiveStore = () => new Promise((resolve) => { finishOpen = resolve; openStarted(); });
  const reviewing = run.reviewTask({ id: 8, state: "failed", spec: spec() });
  await opening;
  assert.ok(run.context.pendingArchiveTaskReview);
  run.context.openArchiveStore = async (path) => { run.context.currentArchive = archive(path, 3); return true; };
  await run.openArchivePath("/another.zip", "open-file");
  finishOpen(true);
  await reviewing;
  assert.equal(run.context.screen, "browse");
  assert.equal(run.context.extractCustomDest, "/unrelated/output");
  assert.equal(run.context.pendingArchiveTaskReview, null);
});

test("failed opens and busy navigation preserve the current draft; best-effort tasks retain recovery routing", async () => {
  for (const reason of ["open-failed", "opening", "create-busy", "convert-busy"]) {
    const run = harness();
    run.context.currentArchive = archive("/unrelated.zip");
    if (reason === "open-failed") run.context.openArchiveStore = async () => false;
    if (reason === "opening") run.context.archiveOpenStatus = "opening";
    if (reason === "create-busy") run.context.preventCreateSubmissionNavigation = () => true;
    if (reason === "convert-busy") run.context.preventConvertSubmissionNavigation = () => true;
    await run.reviewTask({ id: 8, state: "failed", spec: spec() });
    assert.equal(run.context.extractCustomDest, "/unrelated/output");
    assert.equal(run.context.extractOverwriteMode, "overwrite");
    assert.equal(run.context.pendingArchiveTaskReview, null);
    assert.equal(run.calls.some(([name]) => name === "submit" || name === "reset"), false);
  }
  const run = harness();
  await run.reviewTask({ id: 8, state: "failed", spec: spec({ best_effort: true }) });
  assert.equal(run.context.screen, "recovery");
  assert.deepEqual(run.calls.find(([name]) => name === "recovery"), ["recovery", "/original/photos.zip"]);
  assert.equal(run.context.extractCustomDest, "/unrelated/output");
});

test("empty or invalidated selections cannot expand to extracting all entries", async () => {
  for (const changeArchive of [false, true]) {
    const run = harness();
    await run.reviewTask({ id: 8, state: "failed", spec: spec({ selection: changeArchive ? ["photos/"] : [] }) });
    if (changeArchive) {
      run.context.currentArchive = archive("/another.zip", 3);
      run.syncExtractDraftArchive();
    }
    assert.deepEqual(Array.from(run.extractJobPaths()), []);
    assert.equal(run.extractSelectionLabel(), "0 selected");
    assert.match(run.extractStartBlockedReason(), /Select one or more entries/);
    await run.submitExtractJob();
    assert.equal(run.calls.some(([name]) => name === "submit" || name === "plan"), false);
  }
});

test("changing verification while rechecking the plan prevents submitting stale settings", async () => {
  const run = harness();
  await run.reviewTask({ id: 8, state: "failed", spec: spec({ verify_sfx: true }) });
  const plan = run.context.requestExtractPlan;
  run.context.requestExtractPlan = async (...parts) => {
    await plan(...parts);
    run.context.extractVerifySfx = false;
  };
  await run.submitExtractJob();
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  assert.ok(run.calls.some(([name, text]) => name === "notice" && text.includes("settings changed")));
});

test("failed conversion review opens its source and returns to its session after password retry", async () => {
  const run = harness();
  const conversion = { kind: "convert", src: "/old/backup.7z", dest: "/out/backup.zip",
    level: 4, src_encoding: "gbk", src_password: "old-source-password", dest_password: "old-output-password",
    encrypt_names: false, split_size: 123456789, split_mode: "native", replace_existing: true, replacement_guard: "old-guard" };
  let restored = null;
  run.context.focusConvertReview = () => run.calls.push(["focus-convert"]);
  run.context.loadConvertRouteForReview = async () => ({
    canReviewTask: () => true,
    syncArchive() {},
    restoreTaskDraft: (draft) => { restored = draft; return true; },
  });
  run.context.openArchiveStore = async (path, password, encoding) => {
    run.calls.push(["open", path, password, encoding]);
    if (password !== "correct") {
      run.context.archivePasswordPrompt = { path, encoding };
      return false;
    }
    run.context.currentArchive = archive(path, 3, encoding);
    run.context.archivePasswordPrompt = null;
    return true;
  };
  await run.reviewTask({ id: 12, state: "failed", spec: conversion });
  assert.equal(run.context.screen, "password");
  assert.equal(restored, null);
  assert.deepEqual(run.calls.find(([name]) => name === "open"), ["open", conversion.src, null, "gbk"]);
  run.context.jobPasswordValue = "correct";
  await run.submitPasswordRequest();
  assert.equal(run.context.screen, "convert");
  assert.equal(restored.dest, conversion.dest);
  assert.equal(restored.level, 4);
  assert.equal(restored.split_size, 123456789);
  for (const key of ["src_password", "dest_password", "replace_existing", "replacement_guard"]) {
    assert.equal(key in restored, false);
  }
  assert.equal(run.context.pendingArchiveTaskReview, null);
  assert.ok(run.calls.some(([name]) => name === "focus-convert"));
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
});

test("a locked conversion session keeps its draft and task dialog without opening another archive", async () => {
  const run = harness();
  run.context.loadConvertRouteForReview = async () => ({ canReviewTask: () => false });
  await run.reviewTask({ id: 12, state: "failed", spec: { kind: "convert", src: "/old/backup.7z" } });
  assert.equal(run.calls.length, 0);
  assert.equal(run.context.screen, "browse");
});
