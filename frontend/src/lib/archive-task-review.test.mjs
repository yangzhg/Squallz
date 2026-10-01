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
  const names = ["cancelTaskReview", "reviewTask", "reviewExtractTask", "reviewConvertTask", "reviewArchiveTask", "restoreExtractTaskDraft", "finishOpenedArchive",
    "openArchivePath", "openArchiveFromDialog", "dismissArchivePicker", "extractJobPaths", "extractJobDestination", "extractSmartBase",
    "extractSelectionLabel", "extractStartBlockedReason", "submitExtractJob",
    "syncExtractDraftArchive", "cancelPasswordRequest", "submitPasswordRequest", "dismissArchivePasswordRequest",
    "isCurrentTaskPasswordPrompt", "submitTaskPasswordRequest", "cancelTaskPasswordRequest", "passwordWorkspaceSurface",
    "passwordPromptName", "passwordPromptDetail", "passwordSessionDetail", "passwordFailureDetail", "taskPasswordQuestion",
    "setScreen", "effectiveExtractDest", "sameFolderExtractDest",
    "extractEncodingForJob", "archiveEncodingForJob", "extractEncodingLabel"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = {
    calls, taskReviewScreen, taskWindowMode: false, currentArchive: archive(),
    screen: "browse", archiveOpenStatus: "idle", archiveOpenGeneration: 0, archivePasswordAttempt: 0,
    archivePickerRequest: null,
    getDialogModule: async () => ({ open: async () => null }),
    openNativeDialog: async (_kind, open, options) => open(options), platformKind: () => "macos",
    taskReviewRequestGeneration: 0, nestedExtractDraftGeneration: 0, nestedExtractReviewFocusPending: false,
    archiveUpdateReviewFocusPending: false,
    archiveUpdateReview: { cancelSourceChoice() {} },
    batchPickerRequest: 0, nestedExtractPickerRequest: 0,
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
    jobPasswordPrompt: null, jobConflictPrompt: null, archivePasswordPrompt: null, workspacePasswordValue: "",
    workspacePasswordSubmissionAttempted: false, standalonePasswordFocusedInput: null,
    workspacePasswordSubmissionError: null, secretStoreLabel: () => "Keychain",
    isTaskActiveState: (state) => state === "running",
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
    previewPasswordPrompt: null,
    rememberRecent() {}, recordOperation() {}, clearEntryPreviewState() {}, recordValidationRenderReady() {},
    extractPlanKey: (...parts) => JSON.stringify(parts),
    requestExtractPlan: async (...parts) => {
      calls.push(["plan", ...parts]); context.extractPlanRequestKey = JSON.stringify(parts);
      context.extractPlan = { destination: "/original/output/photos", input_guard: "fresh-guard", entries: 3 };
      context.extractPlanPhase = "ready";
    },
    submitCurrentArchiveJob: async (job) => { calls.push(["submit", job]); return true; },
    archiveTitle: () => context.currentArchive?.name, taskPasswordReady: (value) => Boolean(value),
    adoptRecoveryTargetFromTask: (task) => { calls.push(["recovery", task.spec.path]); return true; },
  };
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")},context:globalThis,calls})`,context);
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function withArchiveStore(run, exercise) {
  const store = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
  const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
  const original = Object.fromEntries(["openArchive", "cancelArchiveOpen", "closeArchive", "listEntries"]
    .map((name) => [name, ipc[name]]));
  const pending = deferred();
  const started = deferred();
  const cancelled = [];
  const closed = [];
  ipc.openArchive = async (path, _password, encoding, requestId) => {
    if (path === "/unrelated.zip") return { ...archive(path), read_only: true };
    started.resolve({ path, encoding, requestId });
    return pending.promise;
  };
  ipc.listEntries = async () => ({ page: 0, total: 0, items: [] });
  ipc.cancelArchiveOpen = async (requestId) => { cancelled.push(requestId); };
  ipc.closeArchive = async (id) => { closed.push(id); };
  Object.defineProperties(run.context, {
    currentArchive: { configurable: true, get: () => store.archive() },
    archivePasswordPrompt: { configurable: true, get: () => store.openPasswordPrompt() },
  });
  run.context.openArchiveStore = store.openArchive;
  run.context.cancelPendingArchiveOpen = store.cancelPendingArchiveOpen;
  run.context.cancelArchivePasswordPrompt = store.cancelPasswordPrompt;
  run.context.archiveOpenError = store.archiveOpenError;
  try {
    assert.equal(await store.openArchive("/unrelated.zip"), true);
    await exercise({ store, ipc, pending, started, cancelled, closed });
  } finally {
    store.closeArchive();
    Object.assign(ipc, original);
  }
}

test("leaving a task review while its archive opens keeps the current workspace and releases a late handle", async () => {
  for (const kind of ["extract", "convert"]) {
    const run = harness();
    run.context.loadConvertRouteForReview = async () => ({
      canReviewTask: () => true, syncArchive() {}, restoreTaskDraft: () => { run.calls.push(["restore"]); return true; },
    });
    await withArchiveStore(run, async ({ store, pending, started, cancelled, closed }) => {
      const reviewSpec = kind === "extract" ? spec() : {
        kind, src: "/original/photos.zip", dest: "/out/photos.7z", src_encoding: null,
      };
      const reviewing = run.reviewTask({ id: 8, state: "failed", spec: reviewSpec });
      const request = await started.promise;
      assert.ok(run.context.pendingArchiveTaskReview);
      run.setScreen("create");
      pending.resolve({ ...archive(request.path, 2), read_only: true });
      await reviewing;
      assert.equal(run.context.screen, "create", "a late archive must not replace the page the user chose");
      assert.equal(store.archive().source, "/unrelated.zip");
      assert.equal(run.context.archiveOpenStatus, "idle");
      assert.equal(run.context.pendingArchiveTaskReview, null);
      assert.deepEqual(cancelled, [request.requestId]);
      assert.deepEqual(closed, [2]);
      assert.equal(run.context.extractCustomDest, "/unrelated/output");
      assert.equal(run.calls.some(([name]) => name === "restore" || name === "submit" || name === "notice"), false);
    });
  }
});

test("returning to the original page cannot revive a dismissed open's password or corruption response", async () => {
  for (const key of ["error.password_required", "error.corrupt_archive"]) {
    const run = harness();
    await withArchiveStore(run, async ({ store, pending, started, cancelled }) => {
      const opening = run.openArchivePath("/old.zip", "open-file");
      const request = await started.promise;
      run.setScreen("settingsGeneral");
      run.setScreen("browse");
      pending.reject({ key, params: {}, detail: "" });
      await opening;
      assert.equal(run.context.screen, "browse");
      assert.equal(store.archive().source, "/unrelated.zip");
      assert.equal(store.openPasswordPrompt(), null);
      assert.equal(store.archiveOpenError(), null);
      assert.equal(run.context.archiveOpenStatus, "idle");
      assert.deepEqual(cancelled, [request.requestId]);
      assert.equal(run.calls.some(([name]) => name === "notice"), false);
    });
  }
});

test("leaving while the first page loads releases its handle without clearing a newer open's waiting state", async () => {
  const run = harness();
  await withArchiveStore(run, async ({ store, ipc, pending, started, cancelled, closed }) => {
    const listing = deferred();
    const listingStarted = deferred();
    ipc.listEntries = async (id) => {
      if (id === 2) { listingStarted.resolve(); return listing.promise; }
      return { page: 0, total: 0, items: [] };
    };
    const reviewing = run.reviewTask({ id: 8, state: "failed", spec: spec() });
    const request = await started.promise;
    pending.resolve({ ...archive(request.path, 2), read_only: true });
    await listingStarted.promise;
    run.setScreen("settingsGeneral");
    assert.equal(run.context.archiveOpenStatus, "idle");
    assert.equal(store.archive().source, "/unrelated.zip");
    const newer = deferred();
    ipc.openArchive = () => newer.promise;
    const opening = run.openArchivePath("/newer.zip", "open-file");
    listing.resolve({ page: 0, total: 1, items: [{ path: "old.txt", entry_type: "file" }] });
    await reviewing;
    assert.equal(run.context.screen, "settingsGeneral");
    assert.equal(run.context.archiveOpenStatus, "opening");
    assert.equal(store.archive().source, "/unrelated.zip");
    assert.deepEqual(cancelled, [request.requestId]);
    assert.deepEqual(closed, [2]);
    newer.resolve({ ...archive("/newer.zip", 3), read_only: true });
    await opening;
    assert.equal(run.context.screen, "browse");
    assert.equal(run.context.archiveOpenStatus, "idle");
    assert.equal(store.archive().source, "/newer.zip");
    assert.equal(run.context.extractCustomDest, "/unrelated/output");
  });
});

test("a superseded archive picker cannot replace a newer open or its feedback", async () => {
  for (const outcome of ["selected", "cancelled", "failed"]) {
    const run = harness();
    const picker = deferred();
    const started = deferred();
    run.context.openNativeDialog = () => { started.resolve(); return picker.promise; };
    const choosing = run.openArchiveFromDialog();
    await started.promise;
    const opened = deferred();
    run.context.openArchiveStore = async (path) => {
      run.calls.push(["open", path]);
      if (path === "/newer.zip") await opened.promise;
      return false;
    };
    const opening = run.openArchivePath("/newer.zip", "open-file");
    const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
    if (outcome === "failed") picker.reject(new Error("dialog unavailable"));
    else picker.resolve(outcome === "selected" ? "/older.zip" : null);
    await choosing;
    assert.deepEqual(run.calls.filter(([kind]) => kind === "open"), [["open", "/newer.zip"]]);
    assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
    assert.equal(run.context.archiveOpenStatus, "opening", "old picker cleanup cannot finish the newer read");
    opened.resolve();
    await opening;
    assert.equal(run.context.archiveOpenStatus, "idle");
  }
});

test("navigation abandons a waiting picker even after returning to its original page", async () => {
  for (const phase of ["loading", "choosing"]) {
    const run = harness();
    const pending = deferred();
    const started = deferred();
    let dialogs = 0;
    if (phase === "loading") run.context.getDialogModule = () => { started.resolve(); return pending.promise; };
    run.context.openNativeDialog = () => { dialogs += 1; started.resolve(); return pending.promise; };
    const choosing = run.openArchiveFromDialog();
    await started.promise;
    run.setScreen("settingsGeneral");
    assert.equal(run.context.archiveOpenStatus, "idle");
    run.setScreen("browse");
    const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
    pending.resolve(phase === "loading" ? { open: async () => "/older.zip" } : "/older.zip");
    await choosing;
    assert.equal(dialogs, phase === "loading" ? 0 : 1);
    assert.equal(run.calls.some(([kind]) => kind === "open"), false);
    assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
    assert.equal(run.context.screen, "browse");
  }
});

test("a current archive picker preserves selection, cancellation, and failure feedback", async () => {
  for (const outcome of ["selected", "cancelled", "failed"]) {
    const run = harness();
    run.context.openNativeDialog = async (_kind, _open, options) => {
      assert.equal(options.multiple, false);
      assert.equal(options.filters, undefined, "macOS still accepts arbitrary numbered volumes");
      if (outcome === "failed") throw new Error("dialog unavailable");
      return outcome === "selected" ? ["/chosen.zip"] : null;
    };
    await run.openArchiveFromDialog();
    assert.equal(run.context.archiveOpenStatus, "idle");
    assert.equal(run.context.archivePickerRequest, null);
    if (outcome === "selected") assert.equal(run.context.currentArchive.path, "/chosen.zip");
    else assert.match(run.calls.at(-1)[1], outcome === "cancelled" ? /cancelled/ : /desktop file dialog/);
  }
});

for (const state of ["failed", "cancelled"]) {
test(`${state} extraction review restores its selection and policies instead of reusing the current draft`, async () => {
  const run = harness();
  await run.reviewTask({ id: 8, state, spec: spec() });
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
}

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
  run.context.workspacePasswordValue = "wrong";
  await run.submitPasswordRequest();
  assert.equal(run.context.screen, "password");
  assert.ok(run.context.pendingArchiveTaskReview);
  assert.equal(run.context.workspacePasswordValue, "");
  run.context.workspacePasswordValue = "correct";
  await run.submitPasswordRequest();
  assert.equal(run.context.screen, "extract");
  assert.equal(run.extractJobDestination(), "/original/output");
  assert.deepEqual(Array.from(run.extractJobPaths()), ["photos/", "notes.txt"]);
  assert.equal(run.context.pendingArchiveTaskReview, null);
  assert.equal(run.context.workspacePasswordValue, "");
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

test("an archive unlock response cannot clear a password being entered for a background task", async () => {
  const run = harness();
  run.context.archivePasswordPrompt = { path: "/opening.zip", encoding: null, wrong: false };
  run.context.workspacePasswordValue = "archive-input";
  let finish;
  run.context.openArchiveStore = () => new Promise((resolve) => { finish = resolve; });
  const opening = run.submitPasswordRequest();
  run.context.jobPasswordPrompt = { id: 8, version: 12, name: "background.zip", wrong: false };
  run.context.jobPasswordValue = "task-input";
  finish(false);
  await opening;
  assert.equal(run.context.jobPasswordValue, "task-input");
  assert.equal(run.context.workspacePasswordValue, "");
});

test("overlapping task and archive or preview prompts keep their labels and answers separate", async () => {
  for (const source of ["archive", "preview"]) {
    const run = harness();
    const context = run.context;
    if (source === "archive") context.archivePasswordPrompt = { path: "/opening.zip", encoding: null, wrong: true };
    else context.previewPasswordPrompt = { id: 3, name: "inner.7z", scope: "inner", wrong: true, busy: false };
    context.jobPasswordPrompt = { id: 8, version: 12, name: "background.zip", wrong: false };
    context.jobPasswordValue = "task-input";
    context.workspacePasswordValue = "workspace-input";
    const workspace = run.passwordWorkspaceSurface("modern");
    assert.equal(workspace.name, source === "archive" ? "opening.zip" : "inner.7z");
    assert.equal(workspace.rejected, true);
    assert.match(workspace.detail, /rejected/);
    assert.equal(workspace.value, "workspace-input");
    const taskQuestion = run.taskPasswordQuestion({ id: 8, state: "running" });
    assert.equal(taskQuestion.name, "background.zip");
    assert.match(taskQuestion.detail, /waiting/);
    workspace.onValueChange("workspace-retry");
    assert.equal(context.jobPasswordValue, "task-input");
    const answers = [];
    context.openArchiveStore = async (_path, value) => { answers.push(["archive", value]); return false; };
    context.previewPasswordFlow = { answer: (value) => { answers.push(["preview", value]); return true; } };
    context.answerJobPassword = async (value) => { answers.push(["task", value]); context.jobPasswordPrompt = null; return true; };
    context.returnTaskQuestionToCenter = () => {};
    await workspace.onSubmit();
    assert.deepEqual(answers, [[source, "workspace-retry"]]);
    assert.equal(context.jobPasswordValue, "task-input");
    assert.equal(context.workspacePasswordValue, "");
    workspace.onValueChange("workspace-next");
    await run.submitTaskPasswordRequest(run.context.jobPasswordPrompt);
    assert.deepEqual(answers.at(-1), ["task", "task-input"]);
    assert.equal(context.workspacePasswordValue, "workspace-next");
    assert.equal(context.jobPasswordValue, "");
  }
});

test("a dismissed task password callback cannot submit or cancel an archive password request", async () => {
  const run = harness();
  const prompt = { path: "/opening.zip", encoding: null, wrong: false };
  run.context.archivePasswordPrompt = prompt;
  run.context.workspacePasswordValue = "workspace-input";
  run.context.workspacePasswordSubmissionAttempted = true;
  run.context.screen = "password";
  await run.submitTaskPasswordRequest(run.context.jobPasswordPrompt);
  await run.cancelTaskPasswordRequest(null);
  assert.equal(run.context.archivePasswordPrompt, prompt);
  assert.equal(run.context.workspacePasswordValue, "workspace-input");
  assert.equal(run.context.workspacePasswordSubmissionAttempted, true);
  assert.equal(run.context.screen, "password");
  assert.equal(run.calls.some(([kind]) => kind === "open"), false);
});

test("stale task password actions cannot answer a newer task or a newer attempt", async () => {
  const stale = { id: 8, version: 12, name: "previous.zip", wrong: false };
  for (const next of [{ ...stale, id: 9 }, { ...stale, version: 13, wrong: true }]) {
    const run = harness();
    const answers = [];
    run.context.jobPasswordPrompt = next;
    run.context.jobPasswordValue = "new-task-input";
    run.context.answerJobPassword = async (value) => { answers.push(value); return true; };
    await run.submitTaskPasswordRequest(stale);
    await run.cancelTaskPasswordRequest(stale);
    assert.deepEqual(answers, []);
    assert.equal(run.context.jobPasswordValue, "new-task-input");
    assert.equal(run.context.jobPasswordPrompt, next);
  }
});

test("cancelling, navigating away, or opening another archive drops a pending extraction review", async () => {
  for (const action of ["cancel", "navigate", "open"]) {
    const run = harness();
    run.context.screen = "password";
    run.context.archivePasswordPrompt = { path: "/original/photos.zip", encoding: null };
    run.context.workspacePasswordValue = "old-input";
    run.context.pendingArchiveTaskReview = { path: spec().path, encoding: null, restore: () => true };
    if (action === "cancel") run.cancelPasswordRequest();
    if (action === "navigate") run.setScreen("extract");
    if (action === "open") await run.openArchivePath("/another.zip", "open-file");
    assert.equal(run.context.pendingArchiveTaskReview, null);
    assert.equal(run.context.archivePasswordPrompt, null);
    assert.equal(run.context.workspacePasswordValue, "");
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

for (const state of ["failed", "cancelled"]) {
test(`${state} conversion review opens its source and returns to its session after password retry`, async () => {
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
  await run.reviewTask({ id: 12, state, spec: conversion });
  assert.equal(run.context.screen, "password");
  assert.equal(restored, null);
  assert.deepEqual(run.calls.find(([name]) => name === "open"), ["open", conversion.src, null, "gbk"]);
  run.context.workspacePasswordValue = "correct";
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
}

test("a locked conversion session keeps its draft and task dialog without opening another archive", async () => {
  const run = harness();
  run.context.loadConvertRouteForReview = async () => ({ canReviewTask: () => false });
  await run.reviewTask({ id: 12, state: "failed", spec: { kind: "convert", src: "/old/backup.7z" } });
  assert.equal(run.calls.length, 0);
  assert.equal(run.context.screen, "browse");
});

test("leaving a conversion review during route loading preserves the newer workspace", async () => {
  for (const action of ["navigation", "archive", "later-review"]) {
    for (const outcome of ["ready", "failed"]) {
      const run = harness();
      const loading = deferred();
      run.context.focusConvertReview = () => run.calls.push(["focus-convert"]);
      run.context.loadConvertRouteForReview = () => loading.promise;
      const pending = run.reviewTask({ id: 12, state: "failed", spec: {
        kind: "convert", src: "/old/backup.7z", dest: "/out/backup.zip", src_encoding: null,
      } });
      if (action === "navigation") {
        run.setScreen("settingsGeneral");
        run.setScreen("browse");
      }
      if (action === "archive") run.context.currentArchive = archive("/newer.zip", 9);
      if (action === "later-review") await run.reviewTask({ id: 13, state: "failed", spec: spec() });
      const screen = run.context.screen;
      const callCount = run.calls.length;
      if (outcome === "failed") loading.reject(new Error("route unavailable"));
      else loading.resolve({
        canReviewTask: () => { run.calls.push(["can-review"]); return true; },
        syncArchive: () => run.calls.push(["sync"]),
        restoreTaskDraft: () => { run.calls.push(["restore"]); return true; },
      });
      await pending;
      assert.equal(run.context.screen, screen);
      assert.equal(run.calls.length, callCount, "stale loading must not open archives, replace settings or report errors");
    }
  }
});

test("a current conversion loading failure keeps the task available for another review", async () => {
  const run = harness();
  run.context.loadConvertRouteForReview = async () => { throw new Error("route unavailable"); };
  const task = { id: 12, state: "failed", spec: {
    kind: "convert", src: "/old/backup.7z", dest: "/out/backup.zip", src_encoding: null,
  } };
  await run.reviewTask(task);
  assert.equal(run.context.screen, "browse");
  assert.equal(run.calls.length, 1);
  assert.match(run.calls[0][1], /Could not load conversion settings/);
  run.context.focusConvertReview = () => {};
  run.context.loadConvertRouteForReview = async () => ({
    canReviewTask: () => true, syncArchive() {}, restoreTaskDraft: () => true,
  });
  await run.reviewTask(task);
  assert.equal(run.context.screen, "convert");
  assert.equal(run.context.currentArchive.path, task.spec.src);
});
