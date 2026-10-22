import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";

import { englishTranslate, createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";
import { extractionFixture } from "../../tests/extraction.mjs";

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function archiveInfo(id, name = "outer.zip") {
  return {
    id, name, path: `/archives/${name}`, source: `/archives/${name}`, format: "zip",
    entry_count: 2, read_only: id !== 1, encoding_override: null,
    volumes: null, non_utf8_name_count: 0, garbled_count: 0, suggested_encoding: null,
  };
}

const outerRows = ["inner.zip", "notes.txt"].map((path) => ({
  path, display: path, entry_type: "file", size: 10, compressed: null,
  modified: null, crc: null, encrypted: false, encoding: "utf-8",
}));

function previewOrigin(entryPath, outerSource = "/archives/outer.zip", virtualIndex = null) {
  return { outerSource, outerDisplayPath: outerSource, entryPath, virtualIndex };
}

function previewFailure(message, entryPath = "notes.txt", outerSource = "/archives/outer.zip") {
  return {
    entryPath, entryType: "file", displayName: entryPath.split("/").at(-1),
    policyKind: entryPath.endsWith(".zip") ? "nested" : "system-file",
    outerSource, outerDisplayPath: outerSource, message, retryAction: "preview",
  };
}

function installPreparedPreview(session, preview, failure = null) {
  const nested = "archive" in preview;
  const preparation = session.beginPreparation(previewOrigin(preview.entry_path, preview.outer_path),
    nested ? "nested" : "entry", preview.display_name ?? preview.entry_path);
  assert.ok(preparation);
  assert.equal(nested ? session.acceptNested(preparation, preview) : session.acceptFile(preparation, preview), true);
  if (failure) session.failPreparation(preparation, failure);
  session.finishPreparation(preparation);
  return preparation;
}

function installPreviewFailure(session, failure, { busy = false } = {}) {
  const preparation = session.beginPreparation(previewOrigin(failure.entryPath, failure.outerSource),
    "entry", failure.displayName);
  assert.ok(preparation);
  session.failPreparation(preparation, failure);
  if (!busy) session.finishPreparation(preparation);
  return session.failure;
}

async function withNestedOpen(run, { systemOpen = false } = {}) {
  const server = await createTestServer();
  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const closed = [];
    const cancelledPreviews = [];
    ipc.closeArchive = async (id) => { closed.push(id); };
    ipc.cancelArchiveOpen = async () => {};
    ipc.cancelEntryPreview = async (id) => { cancelledPreviews.push(id); };
    ipc.cancelArchiveSearch = async () => {};
    ipc.releasePreviewSession = async () => true;
    ipc.openNestedArchive = async () => archiveInfo(2, "inner.zip");
    ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
    const pageRequested = deferred();
    const page = deferred();
    ipc.listEntries = async () => { pageRequested.resolve(); return page.promise; };
    archive.installArchivePreview(archiveInfo(1), outerRows, { selected: ["inner.zip"] });
    const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
    const { PreviewSession } = await server.ssrLoadModule("/src/lib/preview-session.svelte.ts");
    const extraction = await extractionFixture(server);
    const { RecoverySession } = await server.ssrLoadModule("/src/lib/recovery-session.svelte.ts");
    const systemOpenHelpers = systemOpen ? await server.ssrLoadModule("/src/lib/preview-presentation.ts") : {};
    const names = ["clearNotice", "cancelTaskReview", "nestedArchiveAction", "extractNestedPreviewArchive", "retryEntryPreview", "runPreviewWithPassword", "clearEntryPreviewState", "entryPreviewForPath", "hasPreparedPreviewForPath", "selectOnlyEntry", "selectEntry", "toggleEntrySelection", "activateEntry", "submitPasswordRequest", "cancelPasswordRequest", "dismissArchivePasswordRequest", "dismissArchivePicker", "setScreen", "openArchivePath", "openRecoverySet", "passwordPromptDetail", "submitPreviewEntry",
      "openRecoveryConfiguration", "saveNativeDialog", "openNativeDialog", "withNativeDialog", "openExtractWorkspace", "selectedJobPaths"];
    if (systemOpen) names.push("openEntryPreview", "repairFilenameEncoding", "setMode");
    const declarations = selectFunctions(source, names).map((node) => node.getText(source));
    const notices = [];
    const operations = [];
    const context = {
      ...systemOpenHelpers, ipc, adoptOpenedArchive: archive.adoptOpenedArchive,
      appNotice: null, noticeTimer: null, clearTimeout, nativeDialogCount: 0,
      taskReviewRequestGeneration: 0, nestedExtractDraftGeneration: 0,
      syncUrl: () => {}, tick: async () => {},
      document: { documentElement: {}, body: {}, querySelectorAll: () => [] },
      archiveOpenGeneration: 0, archiveOpenStatus: "idle", archivePasswordAttempt: 0,
      filenameEncodingRequest: null,
      archivePickerRequest: null, cancelArchivePasswordPrompt: archive.cancelPasswordPrompt,
      archiveUpdateReview: { cancelSourceChoice() {} },
      tools: { leave() {} },
      batchPickerRequest: 0, nestedExtractPickerRequest: 0,
      recordValidationRenderReady: () => {},
      isPar2Path: () => false, openArchiveStore: archive.openArchive,
      finishOpenedArchive: () => { context.screen = "browse"; },
      preventCreateSubmissionNavigation: () => false, preventConvertSubmissionNavigation: () => false,
      focusBlockingTaskIfAny: () => false,
      platformKind: () => "macos",
      focusExtractReview: () => { context.extractFocused = true; },
      previewSession: new PreviewSession(), screen: "browse",
      jobPasswordPrompt: null, jobConflictPrompt: null, archivePasswordPrompt: null,
      workspacePasswordValue: "", workspacePasswordSubmissionAttempted: false,
      taskPasswordReady: (value) => value.length > 0, focusArchiveRow: async () => {},
      queueMicrotask,
      previewSampleForEntry: () => null,
      archiveLikePath: (path) => path.endsWith(".zip"), blockSelectionScopedAction: () => false,
      previewEntryDisplayName: (path) => path.split("/").at(-1), recordValidationEvent: () => {},
      params: new URLSearchParams(), nestedPasswordPreviewSample: () => null,
      create: { dismiss() {}, syncContext() {} },
      dismissArchiveAddPreparation() {},
      outputAuthorizationPending: false, mode: "modern", jobRows: [],
      sameFilePath: (left, right) => left === right,
      getDialogModule: async () => ({ open: async () => null }),
      recordNativeDialogRequest: () => {},
      nextPreflightRequestId: () => "recovery-output-check",
      createDestinationInspectionCancelled: () => false, CreateDestinationInspectionError: class extends Error {},
      isJobSubmitBlocked: () => false, tError: () => "Output check failed",
      archiveMutationDisabledReason: () => "", openArchiveFirstLabel: () => "Open an archive",
      createCompressionLevel: () => context.compressionLevel, compressionLevel: 2,
      activeCreateProfile: "balanced", createProfileLabel: (profile) => profile,
      archiveOutputFilterName: (format) => format,
      archiveExtensionMatch: (name) => name.split(".").at(-1),
      archiveStemName: (name) => name.replace(/\.[^.]+$/, ""),
      pathDir: (path) => path.slice(0, path.lastIndexOf("/")),
      joinFolderPath: (parent, child) => `${parent}/${child}`,
      waitForPreviewFeedbackFrame: async () => {}, archiveEncodingForJob: () => null,
      entryTypeForPath: () => "file", previewPolicyFor: () => ({ kind: "nested" }),
      previewFailureMessage: (_error, _nested, _key, fallback) => fallback,
      archiveSelectionBusyReason: () => "",
      selectRow: archive.selectRow, toggleSelect: archive.toggleSelect, selectedPaths: archive.selectedPaths,
      pathBaseName: (path) => path.split("/").at(-1),
      showNotice: (notice) => notices.push(notice), recordOperation: (operation) => operations.push(operation),
      tr: englishTranslate,
    };
    Object.defineProperty(context, "currentArchive", { get: () => archive.archive() });
    extraction.install(context);
    Object.defineProperties(context, {
      entryPreview: { get: () => context.previewSession.file },
      nestedPreview: { get: () => context.previewSession.nested },
      entryPreviewFailure: { get: () => context.previewSession.failure },
      previewOriginEntryPath: { get: () => context.previewSession.origin?.entryPath ?? null },
      previewOriginVirtualIndex: { get: () => context.previewSession.origin?.virtualIndex ?? null },
      previewPhase: { get: () => context.previewSession.phase },
      previewTargetName: { get: () => context.previewSession.targetName },
      previewPasswordPrompt: { get: () => context.previewSession.passwordPrompt },
    });
    const outputText = compileTestScript(declarations.join("\n"));
    const app = vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context);
    context.recoverySession = new RecoverySession({
      read: () => ({
        active: context.screen === "recovery", mode: context.mode, generation: context.archiveOpenGeneration,
        archive: archive.archive(), tasks: context.jobRows, selection: null, platform: "macos",
        level: context.compressionLevel, profile: context.activeCreateProfile,
      }),
      dialogs: () => context.getDialogModule(),
      openDialog: (...args) => app.openNativeDialog(...args),
      saveDialog: (...args) => app.saveNativeDialog(...args),
      submit: (spec) => context.submitJob(spec), blocked: () => false, focusBlockingTask: () => false,
      notice: context.showNotice, extension: context.archiveExtensionMatch, stem: context.archiveStemName,
      extractDestination: (path) => path.replace(/\.[^.]+$/, ""),
      mutationDisabledReason: () => "", outputFilterName: context.archiveOutputFilterName,
      inspectionId: context.nextPreflightRequestId,
    });
    context.recoverySession.selectSource("/previous.zip", "/previous.par2");

    try {
      await run({ app, archive, ipc, closed, cancelledPreviews, page, pageRequested, context, notices, operations });
    } finally {
      context.previewSession.dispose();
      context.recoverySession.dispose();
      extraction.dispose();
    }
  } finally {
    await server.close();
  }
}

test("an encrypted ordinary file resumes preparation and opens only after its password succeeds", async () => {
  await withNestedOpen(async ({ app, archive, ipc, context }) => {
    const prepared = { preview_id: "file-preview", outer_path: "/archives/outer.zip", entry_path: "notes.txt", display_name: "notes.txt", size: 10, archive_like: false };
    const opened = [];
    const released = [];
    ipc.previewArchiveEntry = async (_source, _entry, password) => {
      if (password !== "entry-password") throw { key: "error.password_required", params: {}, detail: "" };
      return prepared;
    };
    ipc.releasePreviewSession = async (id) => { released.push(id); return true; };
    context.openEntryPreview = async (entry) => { opened.push(entry.preview_id); return true; };
    context.appNotice = "existing feedback";
    context.blockSelectionScopedAction = () => true;
    await app.submitPreviewEntry("notes.txt", "file", 1);
    assert.equal(context.appNotice, "existing feedback", "a blocked ordinary-file action preserves feedback");
    context.blockSelectionScopedAction = () => false;
    const opening = app.submitPreviewEntry("notes.txt", "file", 1);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(context.previewPasswordPrompt.name, "outer.zip");
    assert.deepEqual(opened, []);
    app.setScreen("password");
    context.workspacePasswordValue = "entry-password";
    await app.submitPasswordRequest();
    await opening;
    assert.deepEqual(opened, ["file-preview"]);
    assert.deepEqual(released, ["file-preview"]);
    assert.equal(context.screen, "browse");
    assert.equal(context.entryPreviewFailure, null);
    assert.equal(context.workspacePasswordValue, "");
    assert.equal(archive.archive().id, 1);
  });
});

test("nested opening resumes through the shared password form with separate layer credentials", async () => {
  await withNestedOpen(async ({ app, archive, ipc, page, context }) => {
    const attempts = [];
    const encodings = [];
    const feedbackFrame = deferred();
    let encoding = "shift_jis";
    context.archiveEncodingForJob = () => encoding;
    context.waitForPreviewFeedbackFrame = () => feedbackFrame.promise;
    ipc.openNestedArchive = async (_source, _entry, passwords, requestedEncoding) => {
      attempts.push({ ...passwords });
      encodings.push(requestedEncoding);
      const scope = passwords.outer !== "outer-password" ? "outer" : passwords.inner !== "inner-password" ? "inner" : null;
      if (scope) throw { key: passwords[scope] ? "error.wrong_password" : "error.password_required", params: { password_scope: scope }, detail: "" };
      return archiveInfo(2, "inner.zip");
    };
    page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
    const opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
    encoding = "gbk";
    feedbackFrame.resolve();
    await new Promise((resolve) => setImmediate(resolve));
    app.setScreen("password");
    assert.match(app.passwordPromptDetail(), /archive password to read/);
    for (const [password, nextScope] of [["outer-password", "inner"], ["wrong-password", "inner"], ["inner-password", null]]) {
      assert.equal(archive.archive().id, 1);
      context.workspacePasswordValue = password;
      await app.submitPasswordRequest();
      assert.equal(context.workspacePasswordValue, "");
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(context.previewPasswordPrompt?.scope ?? null, nextScope);
      if (password === "wrong-password") assert.match(app.passwordPromptDetail(), /rejected/);
    }
    await opening;
    assert.equal(archive.archive().id, 2);
    assert.equal(context.screen, "browse");
    assert.equal(attempts.length, 4);
    assert.deepEqual(encodings, ["shift_jis", "shift_jis", "shift_jis", "shift_jis"],
      "opening fixes its encoding before feedback and password waits");
    assert.equal(attempts[2].outer, "outer-password");
    assert.equal(context.entryPreviewFailure, null);
  });
});

test("cancelling during password verification preserves the outer archive and releases the late inner handle", async () => {
  await withNestedOpen(async ({ app, archive, ipc, context, closed }) => {
    const verified = deferred();
    ipc.openNestedArchive = async (_source, _entry, passwords) => {
      if (!passwords.outer) throw { key: "error.password_required", params: {}, detail: "" };
      return verified.promise;
    };
    const opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
    await new Promise((resolve) => setImmediate(resolve));
    context.screen = "password";
    context.workspacePasswordValue = "outer-password";
    await app.submitPasswordRequest();
    await app.cancelPasswordRequest();
    verified.resolve(archiveInfo(2, "inner.zip"));
    await opening;
    assert.equal(context.previewPasswordPrompt, null);
    assert.equal(context.workspacePasswordValue, "");
    assert.equal(context.screen, "browse");
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(closed, [2]);
    assert.equal(context.entryPreviewFailure, null);
  });
});

test("opening another archive immediately cancels a protected preview and releases a late inner result", async () => {
  for (const verifying of [false, true]) {
    await withNestedOpen(async ({ app, archive, ipc, context, closed, cancelledPreviews, page }) => {
      const inner = deferred();
      const replacement = deferred();
      ipc.openNestedArchive = async (_source, _entry, passwords) => {
        if (!passwords.outer) throw { key: "error.password_required", params: {}, detail: "" };
        return inner.promise;
      };
      const previewing = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
      await new Promise((resolve) => setImmediate(resolve));
      context.screen = "password";
      if (verifying) {
        context.workspacePasswordValue = "outer-password";
        await app.submitPasswordRequest();
      }
      ipc.openArchive = () => replacement.promise;
      const opening = app.openArchivePath("/archives/another.zip", "open-file");
      assert.equal(context.previewPasswordPrompt, null, "the old preview must stop asking before the new open completes");
      assert.equal(cancelledPreviews.length, verifying ? 1 : 0);
      inner.resolve(archiveInfo(2, "inner.zip"));
      await previewing;
      assert.equal(archive.archive().id, 1);
      assert.equal(context.screen, "password", "the discarded preview must not navigate");
      replacement.resolve(archiveInfo(3, "another.zip"));
      page.resolve({ items: [], total: 0, page: 0 });
      await opening;
      assert.equal(context.screen, "browse");
      assert.equal(archive.archive().id, 3);
      assert.deepEqual(closed.sort(), verifying ? [1, 2] : [1]);
    });
  }
});

test("opening a recovery sidecar cancels a preparing preview and preserves the chosen recovery source", async () => {
  for (const result of ["success", "password", "failure"]) {
    await withNestedOpen(async ({ app, archive, ipc, context, closed, cancelledPreviews, page }) => {
      const pending = deferred();
      const started = deferred();
      ipc.openNestedArchive = () => { started.resolve(); return pending.promise; };
      const opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
      await started.promise;
      app.openRecoverySet("/recovery/photos.par2", "/recovery/photos.zip", "open-file");
      assert.equal(cancelledPreviews.length, 1);
      assert.equal(context.previewPhase, "idle");
      if (result === "success") pending.resolve(archiveInfo(2, "inner.zip"));
      else pending.reject({ key: result === "password" ? "error.password_required" : "error.io", params: {}, detail: "" });
      page.resolve({ items: [], total: 0, page: 0 });
      await opening;
      assert.equal(context.screen, "recovery");
      assert.equal(context.recoverySession.sourcePath(), "/recovery/photos.zip");
      assert.equal(context.recoverySession.sidecarPath(), "/recovery/photos.par2");
      assert.equal(context.previewPasswordPrompt, null);
      assert.equal(context.entryPreviewFailure, null);
      assert.equal(archive.archive().id, 1);
      assert.deepEqual(closed, result === "success" ? [2] : []);
    });
  }
  await withNestedOpen(async ({ app, archive, ipc, page, context, notices }) => {
    app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
    const selection = deferred(), started = deferred();
    context.getDialogModule = async () => ({ open: () => { started.resolve(); return selection.promise; } });
    const choosing = context.recoverySession.chooseArchive();
    await started.promise;
    const replacement = deferred();
    ipc.openArchive = () => replacement.promise;
    const opening = app.openArchivePath("/archives/replacement.zip", "open-file");
    assert.equal(context.recoverySession.picker, "idle");
    const count = notices.length;
    selection.resolve("/late/old.zip");
    await choosing;
    assert.equal(context.recoverySession.sourcePath(), "/previous/photos.zip");
    assert.equal(context.recoverySession.sidecarPath(), "/previous/photos.par2");
    assert.equal(notices.length, count);
    replacement.resolve(archiveInfo(3, "replacement.zip"));
    page.resolve({ items: [], total: 0, page: 0 });
    await opening;
    assert.equal(archive.archive().id, 3);
    assert.equal(context.screen, "browse");
  });
});

test("leaving a workspace cancels preparation and discards late files, nested previews, passwords and errors", async () => {
  for (const [kind, origin, disposing] of [["file", "browse"], ["preview", "browse"], ["open", "browse"], ["file", "recovery"],
    ["file", "browse", true], ["preview", "browse", true]]) {
    for (const result of ["success", "password", "failure"]) {
      await withNestedOpen(async ({ app, archive, ipc, context, closed, cancelledPreviews, notices, operations, page }) => {
        const pending = deferred();
        const started = deferred();
        const released = [];
        const prepare = () => { started.resolve(); return pending.promise; };
        ipc.previewArchiveEntry = prepare;
        ipc.previewNestedArchive = prepare;
        ipc.openNestedArchive = prepare;
        ipc.releasePreviewSession = async (id) => { released.push(id); return true; };
        context.openEntryPreview = async () => { assert.fail("a departed preview must not open another application"); };
        app.setScreen(origin);
        let addCancellations = 0;
        context.dismissArchiveAddPreparation = () => { addCancellations += 1; };
        context.appNotice = "existing feedback";
        const preparing = kind === "file" ? app.submitPreviewEntry("notes.txt", "file", 1)
          : kind === "preview" ? app.nestedArchiveAction("preview", "/archives/outer.zip", "inner.zip", 0)
            : app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
        assert.equal(addCancellations, kind === "open" ? 1 : 0, "only opening replaces the Add context");
        assert.equal(context.appNotice, null, "an admitted file or nested action clears older feedback");
        await started.promise;
        const target = origin === "recovery" ? "browse" : kind === "preview" ? "recovery" : "settingsGeneral";
        if (disposing) context.previewSession.dispose();
        app.setScreen(target);
        if (disposing) assert.equal(context.previewSession.beginPreparation(previewOrigin("notes.txt"), "entry", "notes.txt"), null);
        assert.equal(context.previewPhase, "idle");
        assert.equal(cancelledPreviews.length, 1);
        if (kind === "open") app.setScreen("browse");
        const info = archiveInfo(2, "inner.zip");
        if (result === "success") pending.resolve(kind === "file" ? {
          preview_id: "departed-file", outer_path: "/archives/outer.zip", entry_path: "notes.txt", display_name: "notes.txt",
        } : kind === "preview" ? {
          outer_path: "/archives/outer.zip", entry_path: "inner.zip", archive: info, items: [], truncated: false,
        } : info);
        else pending.reject({ key: result === "password" ? "error.password_required" : "error.io", params: {}, detail: "" });
        page.resolve({ items: [], total: 0, page: 0 });
        await preparing;
        assert.equal(context.screen, kind === "open" ? "browse" : target);
        assert.equal(archive.archive().id, 1);
        assert.deepEqual([...archive.selectedPaths()], ["inner.zip"]);
        assert.equal(context.entryPreview, null);
        assert.equal(context.nestedPreview, null);
        assert.equal(context.entryPreviewFailure, null);
        assert.equal(context.previewPasswordPrompt, null);
        assert.deepEqual(released, result === "success" && kind === "file" ? ["departed-file"] : []);
        assert.deepEqual(closed, result === "success" && kind !== "file" ? [2] : []);
        assert.deepEqual(notices, []);
        assert.deepEqual(operations, []);
      });
    }
  }
});

test("navigation releases ready previews, but a repeated or blocked route keeps them available", async () => {
  await withNestedOpen(async ({ app, context, ipc, closed }) => {
    const released = [];
    ipc.releasePreviewSession = async (id) => { released.push(id); return true; };
    installPreparedPreview(context.previewSession, {
      preview_id: "ready-file", outer_path: "/archives/outer.zip", entry_path: "notes.txt", display_name: "notes.txt",
    });
    app.setScreen("browse");
    assert.equal(context.entryPreview.preview_id, "ready-file");
    context.preventCreateSubmissionNavigation = () => true;
    app.setScreen("settingsGeneral");
    assert.equal(context.screen, "browse");
    assert.equal(context.entryPreview.preview_id, "ready-file");
    context.preventCreateSubmissionNavigation = () => false;
    app.setScreen("settingsGeneral");
    assert.equal(context.entryPreview, null);
    assert.deepEqual(released, ["ready-file"]);
    app.setScreen("browse");
    installPreparedPreview(context.previewSession, {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip", archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
    });
    app.setScreen("recovery");
    assert.equal(context.nestedPreview, null);
    assert.deepEqual(closed, [2]);
  });
});

test("closing, navigating or selecting another item during nested listing prevents late navigation and errors", async () => {
  for (const action of ["close", "select", "navigate", "leave-return"]) {
    for (const result of ["success", "failure"]) {
      await withNestedOpen(async ({ app, archive, closed, page, pageRequested, context, notices, operations }) => {
        const opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
        await pageRequested.promise;
        assert.equal(archive.archive().id, 1);
        if (action === "select") app.selectOnlyEntry({ source: outerRows[1], virtualIndex: 1 });
        else if (action === "navigate" || action === "leave-return") {
          app.setScreen("recovery");
          if (action === "leave-return") app.setScreen("browse");
        }
        else app.clearEntryPreviewState();
        if (result === "success") page.resolve({ items: [], total: 0, page: 0 });
        else page.reject(new Error("list failed"));
        await opening;
        assert.equal(archive.archive().id, 1, `${action} keeps the outer archive after ${result}`);
        assert.equal(context.screen, action === "navigate" ? "recovery" : "browse");
        assert.deepEqual(archive.loadedRows().map((row) => row.path), ["inner.zip", "notes.txt"]);
        assert.deepEqual([...archive.selectedPaths()], [action === "select" ? "notes.txt" : "inner.zip"]);
        assert.deepEqual(closed, [2], "only the abandoned inner handle is released");
        assert.deepEqual(notices, []);
        assert.deepEqual(operations, []);
        assert.equal(context.entryPreviewFailure, null);
        assert.equal(context.previewPhase, "idle");
        assert.equal(context.recoverySession.sourcePath(), "/previous.zip");
      });
    }
  }
});

test("a current nested listing failure remains retryable and success replaces the archive once", async () => {
  await withNestedOpen(async ({ app, archive, ipc, closed, page, pageRequested, context, notices, operations }) => {
    context.appNotice = "existing feedback";
    await app.nestedArchiveAction("open", null, "inner.zip", 0);
    assert.equal(context.appNotice, "existing feedback", "an invalid source preserves feedback");
    const opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
    await pageRequested.promise;
    page.reject(new Error("list failed"));
    await opening;
    assert.equal(archive.archive().id, 1);
    assert.equal(context.entryPreviewFailure.entryPath, "inner.zip");
    assert.equal(context.entryPreviewFailure.retryAction, "open");
    assert.equal(context.previewPhase, "idle");
    assert.match(context.entryPreviewFailure.message, /Could not open this inner archive/u);
    assert.deepEqual(notices, [], "the inline failure owns feedback without a duplicate notice");
    ipc.openNestedArchive = async () => archiveInfo(3, "inner.zip");
    ipc.listEntries = async () => ({ items: [outerRows[1]], total: 1, page: 0 });
    await app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(archive.loadedRows().map((row) => row.path), ["notes.txt"]);
    assert.deepEqual(closed, [2, 1]);
    assert.equal(context.entryPreviewFailure, null);
    assert.equal(context.previewPhase, "idle");
    assert.equal(context.recoverySession.usesCurrentArchive(), true);
    assert.equal(context.recoverySession.sourcePath(), archive.archive().path);
    assert.equal(operations.length, 1);
    assert.match(notices.at(-1), /Opened nested archive/);
  });
});

test("a late nested listing cannot replace a newer inner archive or clear its result", async () => {
  await withNestedOpen(async ({ app, archive, ipc, closed, page, pageRequested, operations, notices }) => {
    const first = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip", 0);
    await pageRequested.promise;
    ipc.openNestedArchive = async () => archiveInfo(3, "newer.zip");
    ipc.listEntries = async () => ({ items: [outerRows[1]], total: 1, page: 0 });
    await app.nestedArchiveAction("open", "/archives/outer.zip", "newer.zip", 1);
    page.resolve({ items: [], total: 0, page: 0 });
    await first;
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(closed, [1, 2]);
    assert.equal(operations.length, 1);
    assert.equal(notices.length, 1);
    assert.match(notices[0], /newer.zip/);
  });
});

test("system opening and encoding repair keep preparation and feedback within the current context", async () => {
  await withNestedOpen(async ({ app, archive, ipc, context, notices, closed }) => {
    const preview = {
      preview_id: "prepared-command", outer_path: "/archives/outer.zip", entry_path: "fixture.command",
      display_name: "fixture.command", size: 10, archive_like: false,
    };
    const confirmations = [];
    const opened = [];
    const released = [];
    let draftUpdates = 0;
    ipc.releasePreviewSession = async (id) => { released.push(id); return true; };
    ipc.archivePasswordStatus = async () => ({ session: false, available: true, saved: false, error: null });
    context.reopenWithEncoding = archive.reopenWithEncoding;
    const encodingReopened = context.extraction.options.encodingReopened.bind(context.extraction.options);
    context.extraction.options.encodingReopened = () => { draftUpdates += 1; encodingReopened(); };
    const applyEncodingPreset = (label) => context.extraction.options.applyPreset({
      ...context.extraction.options.snapshotPreset(context.currentArchive?.encoding_override ?? null),
      encoding: { kind: "named", label },
    });
    context.persistUiMode = (next) => { context.mode = next; return Promise.resolve(); };
    context.trackAppearanceSave = () => {};
    let preparedRequest;
    function prepare(entry = preview, failure = previewFailure("current feedback", entry.entry_path, entry.outer_path)) {
      app.clearEntryPreviewState();
      archive.installArchivePreview(archiveInfo(1), outerRows, { selected: ["inner.zip"] });
      context.screen = "browse";
      preparedRequest = installPreparedPreview(context.previewSession, { ...entry }, failure);
      context.previewPolicyFor = () => ({ kind: "system-file" });
      notices.length = 0;
      confirmations.length = 0;
      opened.length = 0;
      released.length = 0;
      closed.length = 0;
      draftUpdates = 0;
      applyEncodingPreset("current draft encoding");
      ipc.openPreviewSession = async (id) => { opened.push(id); };
      return context.entryPreviewFailure;
    }
    const confirm = async (...args) => { confirmations.push(args); return true; };
    async function repairEncoding() {
      const preparation = preparedRequest;
      const requests = [];
      ipc.openArchive = async (path, password, encoding) => {
        requests.push({ path, password, encoding });
        return { ...archiveInfo(18), read_only: false, encoding_override: encoding };
      };
      ipc.listEntries = async (_id, pageNumber) => ({ items: outerRows, total: outerRows.length, page: pageNumber });
      const repairing = app.repairFilenameEncoding("gbk");
      context.preventCreateSubmissionNavigation = () => true;
      app.setScreen("settingsGeneral");
      context.preventCreateSubmissionNavigation = () => false;
      app.setScreen("browse");
      app.setMode("modern");
      assert.equal(context.entryPreview, null, "encoding repair must dismiss the old prepared file before reopening");
      assert.equal(context.previewSession.isCurrent(preparation), false, "encoding repair invalidates its prepared request");
      assert.deepEqual(released, [preview.preview_id]);
      app.selectOnlyEntry({ source: outerRows[1], virtualIndex: 1 });
      await repairing;
      assert.deepEqual(requests, [{ path: preview.outer_path, password: null, encoding: "gbk" }]);
      assert.equal(archive.archive().id, 18);
      assert.equal(archive.archive().source, preview.outer_path, "encoding repair keeps the same source path");
      assert.equal(archive.archive().encoding_override, "gbk");
      assert.equal(archive.selectedPaths().size, 0);
      assert.equal(draftUpdates, 1, "a current successful reopen updates its extract draft once");
      assert.equal(context.extraction.state.presetEncodingLabel, null);
      assert.match(notices.at(-1), /reopened with GBK/u);
    }

    for (const change of ["cancel", "navigation", "archive", "selection", "encoding"]) {
      const preparedFeedback = prepare(preview, previewFailure("new feedback", preview.entry_path));
      const module = deferred();
      context.getDialogModule = () => module.promise;
      const opening = app.openEntryPreview();
      if (change === "cancel") app.clearEntryPreviewState();
      if (change === "navigation") { app.setScreen("settingsGeneral"); app.setScreen("browse"); }
      if (change === "archive") archive.installArchivePreview(archiveInfo(3, "replacement.zip"), outerRows);
      if (change === "selection") app.selectOnlyEntry({ source: outerRows[1], virtualIndex: 1 });
      if (change === "encoding") await repairEncoding();
      const feedback = change === "archive" ? preparedFeedback
        : installPreviewFailure(context.previewSession, previewFailure("new feedback"));
      const noticeCount = notices.length;
      module.resolve({ confirm });
      assert.equal(await opening, false, change);
      assert.equal(confirmations.length, 0, `${change}: an expired module must not invoke confirmation`);
      assert.deepEqual(opened, [], change);
      assert.equal(notices.length, noticeCount, change);
      assert.equal(context.entryPreviewFailure, feedback, change);
    }

    for (const accepted of [true, false]) {
      const feedback = prepare();
      context.getDialogModule = async () => ({ confirm: async (...args) => { confirmations.push(args); return accepted; } });
      assert.equal(await app.openEntryPreview(), accepted);
      assert.equal(confirmations.length, 1);
      assert.match(confirmations[0][0], /fixture.command/u);
      assert.deepEqual(opened, accepted ? [preview.preview_id] : []);
      assert.equal(context.entryPreviewFailure, accepted ? null : feedback);
      assert.equal(notices.length, 1);
      assert.match(notices[0], accepted ? /^Opened:/u : /was not opened/u);
    }

    for (const change of ["archive", "encoding"]) {
      const preparedFeedback = prepare(preview, previewFailure("replacement archive feedback", preview.entry_path));
      const confirmation = deferred();
      const confirmationStarted = deferred();
      context.getDialogModule = async () => ({ confirm: (...args) => {
        confirmations.push(args);
        confirmationStarted.resolve();
        return confirmation.promise;
      } });
      const awaitingConfirmation = app.openEntryPreview();
      await confirmationStarted.promise;
      if (change === "archive") archive.installArchivePreview(archiveInfo(3, "replacement.zip"), outerRows);
      else await repairEncoding();
      const changedSourceFeedback = change === "archive" ? preparedFeedback
        : installPreviewFailure(context.previewSession, previewFailure("replacement archive feedback"));
      const noticeCount = notices.length;
      confirmation.resolve(true);
      assert.equal(await awaitingConfirmation, false, change);
      assert.equal(confirmations.length, 1, change);
      assert.deepEqual(opened, [], "a late approval must not hand the expired file to the system");
      assert.equal(notices.length, noticeCount, change);
      assert.equal(context.entryPreviewFailure, changedSourceFeedback, change);
    }

    prepare();
    ipc.openArchive = async () => { throw { key: "error.io", params: {}, detail: "" }; };
    await app.repairFilenameEncoding("gbk");
    assert.equal(archive.archive().id, 1, "a genuine encoding failure keeps the original archive");
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(draftUpdates, 0);
    assert.equal(context.extraction.state.presetEncodingLabel, "current draft encoding");
    assert.deepEqual(notices, ["Could not reopen archive with that encoding"]);

    for (const change of ["other archive", "navigation ABA", "mode ABA", "same-source reopen"]) {
      prepare();
      const response = deferred();
      const replacement = { ...archiveInfo(19, "replacement.zip"), read_only: false };
      ipc.openArchive = async (path, _password, encoding) => path === replacement.source
        ? replacement : encoding === "gbk" ? response.promise
          : { ...archiveInfo(19), read_only: false, encoding_override: encoding };
      ipc.listEntries = async (_id, pageNumber) => ({ items: outerRows, total: outerRows.length, page: pageNumber });
      const repairing = app.repairFilenameEncoding("gbk");
      if (change === "other archive") {
        assert.equal(await archive.openArchive(replacement.source), true);
      } else if (change === "navigation ABA") {
        app.setScreen("settingsGeneral");
        app.setScreen("browse");
      } else if (change === "mode ABA") {
        app.setMode("classic");
        app.setMode("modern");
      } else {
        assert.equal(await archive.openArchive(preview.outer_path, null, "shift_jis"), true);
      }
      context.showNotice("new context feedback");
      applyEncodingPreset("new context draft encoding");
      const noticeCount = notices.length;
      response.resolve({ ...archiveInfo(18), read_only: false, encoding_override: "gbk" });
      await repairing;
      if (change === "other archive" || change === "same-source reopen") {
        assert.equal(archive.archive().id, 19, change);
        assert.equal(archive.archive().source, change === "other archive" ? replacement.source : preview.outer_path, change);
        assert.ok(closed.includes(18), "the actual store releases the discarded encoding result");
      }
      assert.equal(notices.length, noticeCount, `${change}: old encoding feedback must remain silent`);
      assert.equal(notices.at(-1), "new context feedback", change);
      assert.equal(draftUpdates, 0, change);
      assert.equal(context.extraction.state.presetEncodingLabel, "new context draft encoding", change);
    }

    prepare();
    ipc.openArchive = async (_path, _password, encoding) => ({
      ...archiveInfo(encoding === "gbk" ? 18 : 19), read_only: false, encoding_override: encoding,
    });
    context.reopenWithEncoding = async (...args) => {
      const result = await archive.reopenWithEncoding(...args);
      assert.equal(result.status, "applied");
      assert.equal(result.isCurrent(), true, "the real encoding result initially owns its published archive");
      assert.equal(archive.archive().id, 18);
      assert.equal(await archive.openArchive(preview.outer_path, null, "shift_jis"), true);
      assert.equal(result.isCurrent(), false, "a newer same-source open supersedes an already applied result");
      context.showNotice("newer same-source feedback");
      applyEncodingPreset("newer same-source draft encoding");
      return result;
    };
    await app.repairFilenameEncoding("gbk");
    assert.equal(archive.archive().id, 19);
    assert.equal(archive.archive().encoding_override, "shift_jis");
    assert.deepEqual(notices, ["newer same-source feedback"]);
    assert.equal(draftUpdates, 0);
    assert.equal(context.extraction.state.presetEncodingLabel, "newer same-source draft encoding");
    context.reopenWithEncoding = archive.reopenWithEncoding;

    const preparedFile = { ...preview, entry_path: "notes.txt", display_name: "notes.txt" };
    prepare(preparedFile);
    context.appNotice = "existing feedback";
    ipc.openPreviewSession = async () => { throw new Error("system open failed"); };
    const opening = app.openEntryPreview();
    assert.equal(context.appNotice, null, "system opening clears older feedback before waiting");
    context.appNotice = "newer independent feedback";
    assert.equal(await opening, false);
    assert.match(context.entryPreviewFailure.message, /Could not open this item.*Try again or extract/u);
    assert.equal(context.entryPreview.preview_id, preview.preview_id, "system-open failure keeps the prepared file retryable");
    assert.equal(context.appNotice, "newer independent feedback", "a late system-open failure preserves other actions' feedback");
    assert.deepEqual(notices, [], "system-open failure is visible inline without a duplicate notice");

    prepare(preparedFile);
    app.clearEntryPreviewState();
    const preparing = deferred();
    const preparationRequested = deferred();
    ipc.previewArchiveEntry = () => { preparationRequested.resolve(); return preparing.promise; };
    ipc.openPreviewSession = async () => { throw new Error("system open failed"); };
    context.appNotice = "existing feedback";
    const automaticOpening = app.submitPreviewEntry("notes.txt", "file", 1);
    assert.equal(context.appNotice, null);
    await preparationRequested.promise;
    context.appNotice = "newer preparation feedback";
    preparing.resolve(preparedFile);
    await automaticOpening;
    assert.equal(context.appNotice, "newer preparation feedback", "automatic system opening preserves feedback received during preparation");
    assert.match(context.entryPreviewFailure.message, /Could not open this item.*Try again or extract/u);
    assert.equal(context.entryPreview.preview_id, preparedFile.preview_id);
    assert.deepEqual(notices, []);

    for (const result of ["success", "failure"]) {
      prepare({ ...preview, entry_path: "notes.txt", display_name: "notes.txt" });
      const response = deferred();
      const requested = deferred();
      context.getDialogModule = () => assert.fail("an ordinary file must not require confirmation");
      ipc.openPreviewSession = (id) => { opened.push(id); requested.resolve(); return response.promise; };
      const opening = app.openEntryPreview();
      await requested.promise;
      app.clearEntryPreviewState();
      installPreparedPreview(context.previewSession, { ...preview, preview_id: "newer-preview" },
        previewFailure("newer preview feedback", preview.entry_path));
      const feedback = context.previewSession.failure;
      if (result === "success") response.resolve();
      else response.reject(new Error("expired system-open failure"));
      assert.equal(await opening, false, result);
      assert.deepEqual(opened, [preview.preview_id], "an already submitted OS open cannot be withdrawn");
      assert.deepEqual(notices, [], result);
      assert.equal(context.entryPreview.preview_id, "newer-preview", result);
      assert.equal(context.entryPreviewFailure, feedback, result);
    }
  }, { systemOpen: true });
});

test("opening a nested preview adopts its existing handle without another extraction or password request", async () => {
  for (const action of ["panel", "row", "click", "checkbox"]) {
    await withNestedOpen(async ({ app, archive, ipc, page, pageRequested, context, closed }) => {
      installPreparedPreview(context.previewSession, {
        outer_path: "/archives/outer.zip", entry_path: "inner.zip",
        archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
      });
      ipc.openNestedArchive = async () => { assert.fail("the prepared archive must be reused"); };
      const row = { source: outerRows[0], virtualIndex: 0 };
      if (action === "click") await app.selectEntry(row);
      if (action === "checkbox") app.toggleEntrySelection(row);
      assert.equal(context.nestedPreview.archive.id, 2, action);
      assert.deepEqual(closed, [], action);
      const opening = action === "panel"
        ? app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip")
        : app.activateEntry(row);
      await pageRequested.promise;
      assert.equal(archive.archive().id, 1, "the outer archive stays installed while the inner listing waits");
      assert.equal(context.nestedPreview, null, "adoption detaches the prepared preview before cleanup");
      assert.deepEqual(closed, [], "the transferred handle is not released during selection or adoption");
      page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
      await opening;
      assert.equal(archive.archive().id, 2, action);
      assert.equal(context.nestedPreview, null, action);
      assert.equal(context.previewPasswordPrompt, null, action);
      assert.deepEqual(closed, [1], action);
    });
  }
  for (const mismatch of ["entry", "source", "failure"]) {
    await withNestedOpen(async ({ app, archive, context, closed }) => {
      const source = mismatch === "source" ? "/archives/other.zip" : "/archives/outer.zip";
      const failure = previewFailure("previous feedback", "inner.zip", source);
      if (mismatch === "failure") installPreviewFailure(context.previewSession, failure);
      else installPreparedPreview(context.previewSession, {
        outer_path: source, entry_path: "inner.zip", archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
      }, failure);
      const index = mismatch === "entry" ? 1 : 0;
      app.selectOnlyEntry({ source: outerRows[index], virtualIndex: index });
      assert.equal(archive.archive().id, 1, mismatch);
      assert.equal(context.nestedPreview, null, mismatch);
      assert.equal(context.entryPreviewFailure, null, mismatch);
      assert.deepEqual(closed, mismatch === "failure" ? [] : [2], mismatch);
    });
  }
});

test("extracting a nested preview opens the shared extraction workspace with the verified source", async () => {
  await withNestedOpen(async ({ app, archive, ipc, page, context, closed }) => {
    const inner = { ...archiveInfo(2, "inner.zip"), source: "squallz-archive://2" };
    installPreparedPreview(context.previewSession, {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip",
      archive: inner, items: [], truncated: false,
    });
    ipc.openNestedArchive = async () => { assert.fail("do not extract the inner archive again"); };
    let addCancellations = 0;
    context.dismissArchiveAddPreparation = () => { addCancellations += 1; };
    context.appNotice = "existing feedback";
    context.focusBlockingTaskIfAny = () => true;
    await app.extractNestedPreviewArchive();
    assert.equal(archive.archive().id, 1);
    assert.equal(context.nestedPreview.archive.id, 2);
    assert.deepEqual(closed, []);
    assert.equal(addCancellations, 0, "a blocked extraction preserves Add preparation");
    assert.equal(context.appNotice, "existing feedback", "a blocked extraction preserves feedback");
    context.focusBlockingTaskIfAny = () => false;
    page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
    const extracting = app.extractNestedPreviewArchive();
    assert.equal(addCancellations, 1, "an admitted extraction cancels Add before preparation waits");
    assert.equal(context.appNotice, null);
    await extracting;
    assert.equal(archive.archive().source, inner.source);
    assert.equal(context.screen, "extract");
    assert.equal(context.extraction.state.scope, "all");
    assert.equal(context.extractFocused, true);
    assert.equal(context.previewPasswordPrompt, null);
    assert.equal(context.nestedPreview, null);
    assert.deepEqual(closed, [1]);
  });
});

test("leaving or dismissing during preview extraction keeps the current archive and releases the inner source", async () => {
  for (const action of ["leave", "leave-return", "dismiss"]) {
    await withNestedOpen(async ({ app, archive, page, pageRequested, context, closed }) => {
      installPreparedPreview(context.previewSession, {
        outer_path: "/archives/outer.zip", entry_path: "inner.zip",
        archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
      });
      const extracting = app.extractNestedPreviewArchive();
      await pageRequested.promise;
      if (action === "leave") app.setScreen("settingsGeneral");
      else if (action === "leave-return") { app.setScreen("settingsGeneral"); app.setScreen("browse"); }
      else app.clearEntryPreviewState();
      page.resolve({ items: [], total: 0, page: 0 });
      await extracting;
      assert.equal(archive.archive().id, 1);
      assert.equal(context.screen, action === "leave" ? "settingsGeneral" : "browse");
      assert.equal(context.extractFocused, undefined);
      assert.deepEqual(closed, [2]);
    });
  }
});

test("retrying a failed prepared listing preserves the extraction intent", async () => {
  await withNestedOpen(async ({ app, archive, ipc, page, context, closed }) => {
    installPreparedPreview(context.previewSession, {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip",
      archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
    });
    page.reject(new Error("list failed"));
    await app.extractNestedPreviewArchive();
    assert.equal(archive.archive().id, 1);
    assert.equal(context.entryPreviewFailure.retryAction, "extract");
    assert.equal(context.screen, "browse");
    ipc.openNestedArchive = async () => archiveInfo(3, "inner.zip");
    ipc.listEntries = async () => ({ items: [outerRows[1]], total: 1, page: 0 });
    app.retryEntryPreview();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(archive.archive().id, 3);
    assert.equal(context.screen, "extract");
    assert.equal(context.entryPreviewFailure, null);
    assert.deepEqual(closed, [2, 1]);
  });
});

test("dismissing a nested preview releases it once and cancellation during adoption releases the transferred handle", async () => {
  for (const duringAdoption of [false, true]) {
    for (const disposing of [false, true]) {
      await withNestedOpen(async ({ app, archive, page, pageRequested, context, closed }) => {
        installPreparedPreview(context.previewSession, {
          outer_path: "/archives/outer.zip", entry_path: "inner.zip",
          archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
        });
        let opening;
        if (duringAdoption) {
          opening = app.nestedArchiveAction("open", "/archives/outer.zip", "inner.zip");
          await pageRequested.promise;
        }
        if (disposing) context.previewSession.dispose();
        app.clearEntryPreviewState();
        app.clearEntryPreviewState();
        if (disposing) assert.equal(context.previewSession.beginPreparation(previewOrigin("inner.zip"), "nested", "inner.zip"), null);
        page.resolve({ items: [], total: 0, page: 0 });
        await opening;
        assert.equal(archive.archive().id, 1);
        assert.deepEqual(closed, [2]);
      });
    }
  }
});

test("a late nested preview releases the prepared archive after dismissal", async () => {
  for (const action of ["dismiss", "same-row-selection"]) {
    await withNestedOpen(async ({ app, archive, ipc, context, closed, notices, operations }) => {
      const pending = deferred();
      const requested = deferred();
      const feedbackFrame = deferred();
      const encodings = [];
      let encoding = "shift_jis";
      context.archiveEncodingForJob = () => encoding;
      context.waitForPreviewFeedbackFrame = () => feedbackFrame.promise;
      ipc.previewNestedArchive = async (_source, _entry, passwords, requestedEncoding) => {
        encodings.push(requestedEncoding);
        if (!passwords.outer) throw { key: "error.password_required", params: {}, detail: "" };
        requested.resolve();
        return pending.promise;
      };
      const previewing = app.nestedArchiveAction("preview", "/archives/outer.zip", "inner.zip", 0);
      encoding = "gbk";
      feedbackFrame.resolve();
      await new Promise((resolve) => setImmediate(resolve));
      encoding = "big5";
      context.workspacePasswordValue = "outer-password";
      await app.submitPasswordRequest();
      await requested.promise;
      assert.deepEqual(encodings, ["gbk", "big5"], "preview reads the current encoding for each password attempt");
      if (action === "same-row-selection") app.selectOnlyEntry({ source: outerRows[0], virtualIndex: 0 });
      else app.clearEntryPreviewState();
      const feedback = installPreviewFailure(context.previewSession, previewFailure("newer preview feedback"), { busy: true });
      pending.resolve({ outer_path: "/archives/outer.zip", entry_path: "inner.zip", archive: archiveInfo(2, "inner.zip"), items: [], truncated: false });
      await previewing;
      assert.equal(archive.archive().id, 1);
      assert.equal(context.nestedPreview, null);
      assert.deepEqual(closed, [2]);
      assert.equal(context.entryPreviewFailure, feedback);
      assert.equal(context.previewPhase, "entry", "the old finally cannot clear newer preparation");
      assert.equal(context.previewTargetName, "notes.txt");
      assert.deepEqual(notices, []);
      assert.deepEqual(operations, []);
    });
  }
});
