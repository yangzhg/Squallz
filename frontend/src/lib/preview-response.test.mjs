import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

import { createTestServer } from "../../tests/runtime.mjs";

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

async function withNestedOpen(run) {
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
    const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
    const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
    const { createPreviewPasswordFlow } = await server.ssrLoadModule("/src/lib/preview-password.svelte.ts");
    const recoveryResults = await server.ssrLoadModule("/src/lib/recovery-result.ts");
    const names = ["cancelTaskReview", "openNestedArchiveEntry", "extractNestedPreviewArchive", "retryEntryPreview", "runPreviewWithPassword", "clearEntryPreviewState", "selectOnlyEntry", "submitPasswordRequest", "cancelPasswordRequest", "dismissArchivePasswordRequest", "dismissArchivePicker", "setScreen", "openArchivePath", "openRecoverySet", "passwordPromptDetail", "submitPreviewEntry", "submitPreviewNestedArchive", "prepareEntryPreviewSerially", "disposeEntryPreview",
      "chooseRecoveryArchive", "chooseRecoveryPar2", "useCurrentArchiveForRecovery", "useDefaultPar2ForRecovery",
      "recoverySourcePath", "recoverySourceName", "openRecoveryConfiguration", "adoptRecoveryTargetFromTask", "dismissRecoveryPreparation",
      "recoveryOutputContext", "isCurrentRecoveryOutputPreparation", "submitRecoveryOutputJob",
      "recoverySourceForJob", "recoverySourceMatchesCurrentArchive", "recoveryPar2Path", "defaultRecoveryPath",
      "recoverySourceIsSplit", "recoverySourceFormatId", "isRecoverySourceZipFamily", "isRecoverySourceSqz",
      "recoveryZipDisabledReason", "recoverySqzRepairDisabledReason", "recoverySqzExportDisabledReason", "recoveryRepairPar2DisabledReason",
      "recoveryRepairUsesDirectory", "recoveryReportNumber", "recoveryReport", "latestRecoveryReportTask",
      "defaultSqzRepairDest", "defaultSqzExportDest", "defaultZipRepairDest", "defaultPar2RepairDest", "defaultPar2RepairDirectoryName",
      "authorizeArchiveOutput", "saveNativeDialog", "openNativeDialog"];
    const declarations = names.map((name) => {
      const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
      assert.ok(declaration, name);
      return declaration.getText(source);
    });
    const notices = [];
    const operations = [];
    const context = {
      ...recoveryResults, ipc, adoptOpenedArchive: archive.adoptOpenedArchive,
      taskReviewRequestGeneration: 0, nestedExtractDraftGeneration: 0,
      syncUrl: () => {}, tick: async () => {},
      document: { documentElement: {}, body: {}, querySelectorAll: () => [] },
      archiveOpenGeneration: 0, archiveOpenStatus: "idle", archivePasswordAttempt: 0,
      archivePickerRequest: null, cancelArchivePasswordPrompt: archive.cancelPasswordPrompt,
      archiveUpdateReview: { cancelSourceChoice() {} },
      batchPickerRequest: 0, nestedExtractPickerRequest: 0,
      recordValidationRenderReady: () => {},
      isPar2Path: () => false, openArchiveStore: archive.openArchive,
      finishOpenedArchive: () => { context.screen = "browse"; },
      preventCreateSubmissionNavigation: () => false, preventConvertSubmissionNavigation: () => false,
      focusBlockingTaskIfAny: () => false,
      openExtractWorkspace: (scope) => { context.extractScope = scope; app.setScreen("extract"); },
      focusExtractReview: () => { context.extractFocused = true; },
      previewPasswordFlow: createPreviewPasswordFlow(ipc.cancelEntryPreview), screen: "browse",
      jobPasswordPrompt: null, jobConflictPrompt: null, archivePasswordPrompt: null,
      workspacePasswordValue: "", workspacePasswordSubmissionAttempted: false,
      taskPasswordReady: (value) => value.length > 0, focusArchiveRow: async () => {},
      queueMicrotask,
      entryPreviewPreparationTail: Promise.resolve(), previewSampleForEntry: () => null,
      archiveLikePath: (path) => path.endsWith(".zip"), blockSelectionScopedAction: () => false,
      previewEntryDisplayName: (path) => path.split("/").at(-1), recordValidationEvent: () => {},
      previewOriginEntryPath: null, previewOriginVirtualIndex: null,
      previewRequestGeneration: 0, previewActionGeneration: 0,
      nestedPreview: null, entryPreview: null, entryPreviewFailure: null,
      previewPhase: "idle", previewTargetName: "",
      params: new URLSearchParams(), nestedPasswordPreviewSample: () => null,
      recoverySourceMode: "selected", recoverySourceOverride: "/previous.zip", recoveryPar2Override: "/previous.par2",
      recoveryPickerStatus: "idle", recoveryPickerRequest: 0, recoveryOutputPreparation: null,
      recoverySubmissionPending: false, outputAuthorizationPending: false, mode: "modern", jobRows: [],
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
      archiveSelectionBusyReason: () => "", entryPreviewForPath: () => null,
      selectRow: archive.selectRow, pathBaseName: (path) => path.split("/").at(-1),
      showNotice: (notice) => notices.push(notice), recordOperation: (operation) => operations.push(operation),
      tr: (_key, fallback) => fallback,
    };
    Object.defineProperty(context, "currentArchive", { get: () => archive.archive() });
    Object.defineProperty(context, "previewPasswordPrompt", { get: () => context.previewPasswordFlow.prompt });
    const { outputText } = ts.transpileModule(declarations.join("\n"), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    });
    const app = vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context);
    await run({ app, archive, ipc, closed, cancelledPreviews, page, pageRequested, context, notices, operations });
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
    ipc.openNestedArchive = async (_source, _entry, passwords) => {
      attempts.push({ ...passwords });
      const scope = passwords.outer !== "outer-password" ? "outer" : passwords.inner !== "inner-password" ? "inner" : null;
      if (scope) throw { key: passwords[scope] ? "error.wrong_password" : "error.password_required", params: { password_scope: scope }, detail: "" };
      return archiveInfo(2, "inner.zip");
    };
    page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
    const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
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
    const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
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
      const previewing = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
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
      const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
      await started.promise;
      app.openRecoverySet("/recovery/photos.par2", "/recovery/photos.zip", "open-file");
      assert.equal(cancelledPreviews.length, 1);
      assert.equal(context.previewPhase, "idle");
      if (result === "success") pending.resolve(archiveInfo(2, "inner.zip"));
      else pending.reject({ key: result === "password" ? "error.password_required" : "error.io", params: {}, detail: "" });
      page.resolve({ items: [], total: 0, page: 0 });
      await opening;
      assert.equal(context.screen, "recovery");
      assert.equal(context.recoverySourceOverride, "/recovery/photos.zip");
      assert.equal(context.recoveryPar2Override, "/recovery/photos.par2");
      assert.equal(context.previewPasswordPrompt, null);
      assert.equal(context.entryPreviewFailure, null);
      assert.equal(archive.archive().id, 1);
      assert.deepEqual(closed, result === "success" ? [2] : []);
    });
  }
  await withNestedOpen(async ({ app, archive, ipc, page, context, notices, operations }) => {
    const choose = (kind) => kind === "archive" ? app.chooseRecoveryArchive() : app.chooseRecoveryPar2();
    const recovery = () => ({ screen: context.screen, mode: context.recoverySourceMode,
      source: context.recoverySourceOverride, sidecar: context.recoveryPar2Override });
    const begin = (kind, phase) => {
      const pending = deferred();
      const started = deferred();
      const dialogs = [];
      const open = async (options) => { dialogs.push(options); started.resolve(); return pending.promise; };
      context.getDialogModule = phase === "loading"
        ? () => { started.resolve(); return pending.promise; }
        : async () => ({ open });
      const choosing = choose(kind);
      return { pending, started, dialogs, choosing };
    };
    const finish = (picker, kind, phase, result) => {
      if (result === "failure") picker.pending.reject(new Error("dialog unavailable"));
      else if (phase === "loading") picker.pending.resolve({ open: async (options) => {
        picker.dialogs.push(options);
        return result === "selected" ? `/late/old.${kind === "archive" ? "zip" : "par2"}` : null;
      } });
      else picker.pending.resolve(result === "selected" ? [`/late/old.${kind === "archive" ? "zip" : "par2"}`] : null);
    };
    const expectDiscarded = async (picker, kind, phase, result) => {
      assert.equal(context.recoveryPickerStatus, "idle", "invalidating a picker must immediately release its busy state");
      const kept = recovery();
      const noticeCount = notices.length;
      const operationCount = operations.length;
      finish(picker, kind, phase, result);
      await picker.choosing;
      assert.deepEqual(recovery(), kept, `${kind} ${phase} must keep the newer recovery source after ${result}`);
      assert.equal(context.recoveryPickerStatus, "idle");
      assert.equal(picker.dialogs.length, phase === "loading" ? 0 : 1, "a dismissed module load must not open a native dialog");
      assert.equal(notices.length, noticeCount, "a dismissed picker must not announce selection, cancellation or failure");
      assert.equal(operations.length, operationCount);
      assert.equal(context.entryPreviewFailure, null);
      assert.equal(context.previewPasswordPrompt, null);
      assert.equal(archive.archive().id, 1);
    };
    for (const kind of ["archive", "par2"]) {
      for (const phase of ["loading", "choosing"]) {
        for (const result of ["selected", "cancelled", "failure"]) {
          app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
          const picker = begin(kind, phase);
          await picker.started.promise;
          assert.equal(context.recoveryPickerStatus, kind);
          app.setScreen("settingsGeneral");
          await expectDiscarded(picker, kind, phase, result);
          const request = context.recoveryPickerRequest;
          const noticeCount = notices.length;
          await choose(kind);
          assert.equal(context.recoveryPickerRequest, request, "a callback from a departed recovery page must not start another picker");
          assert.equal(notices.length, noticeCount);
          assert.equal(picker.dialogs.length, phase === "loading" ? 0 : 1);
        }
      }
      app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
      const old = begin(kind, "choosing");
      await old.started.promise;
      app.setScreen("settingsGeneral");
      app.setScreen("recovery");
      await expectDiscarded(old, kind, "choosing", "selected");
      for (const result of ["selected", "cancelled", "failure"]) {
        app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
        const picker = begin(kind, "choosing");
        await picker.started.promise;
        app.setScreen("recovery");
        assert.equal(context.recoveryPickerStatus, kind);
        const kept = recovery();
        const noticeCount = notices.length;
        finish(picker, kind, "choosing", result);
        await picker.choosing;
        assert.equal(context.recoveryPickerStatus, "idle");
        assert.equal(notices.length, noticeCount + 1, "a current picker must preserve its outcome feedback");
        if (result === "selected") {
          assert.equal(kind === "archive" ? context.recoverySourceOverride : context.recoveryPar2Override,
            `/late/old.${kind === "archive" ? "zip" : "par2"}`);
          assert.equal(kind === "archive" ? context.recoveryPar2Override : context.recoverySourceOverride,
            kind === "archive" ? kept.sidecar : kept.source);
        } else assert.deepEqual(recovery(), kept);
        assert.equal(picker.dialogs.length, 1);
      }
    }
    for (const [action, kind] of [["new-set", "archive"], ["current-archive", "archive"], ["default-par2", "par2"]]) {
      app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
      const picker = begin(kind, "choosing");
      await picker.started.promise;
      if (action === "new-set") app.openRecoverySet("/new/photos.par2", "/new/photos.zip", "open-file");
      else if (action === "current-archive") app.useCurrentArchiveForRecovery();
      else app.useDefaultPar2ForRecovery();
      await expectDiscarded(picker, kind, "choosing", "selected");
    }
    for (const [kind, result] of [["archive", "selected"], ["par2", "failure"]]) {
      app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
      const old = begin(kind, "choosing");
      await old.started.promise;
      app.openRecoverySet("/new/photos.par2", "/new/photos.zip", "open-file");
      assert.equal(context.recoveryPickerStatus, "idle");
      const nextKind = kind === "archive" ? "par2" : "archive";
      const next = begin(nextKind, "choosing");
      await next.started.promise;
      const kept = recovery();
      const noticeCount = notices.length;
      finish(old, kind, "choosing", result);
      await old.choosing;
      assert.deepEqual(recovery(), kept);
      assert.equal(context.recoveryPickerStatus, nextKind, "the old finally must not unlock the current picker");
      assert.equal(notices.length, noticeCount);
      next.pending.resolve(`/new/selected.${nextKind === "archive" ? "zip" : "par2"}`);
      await next.choosing;
      assert.equal(context.recoveryPickerStatus, "idle");
      assert.equal(nextKind === "archive" ? context.recoverySourceOverride : context.recoveryPar2Override,
        `/new/selected.${nextKind === "archive" ? "zip" : "par2"}`);
    }
    for (const [action, kind] of [["preserve-route", "archive"], ["current-route", "archive"],
      ["missing-current", "archive"], ["missing-default", "par2"], ["unavailable-task", "par2"], ["valid-task", "archive"]]) {
      app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
      if (action === "current-route" || action === "missing-default") {
        Object.assign(context, { recoverySourceMode: "none", recoverySourceOverride: null, recoveryPar2Override: null });
      }
      const picker = begin(kind, "choosing");
      await picker.started.promise;
      const request = context.recoveryPickerRequest;
      if (action === "preserve-route" || action === "current-route") app.openRecoveryConfiguration("preserve");
      else if (action === "missing-current") {
        archive.closeArchive();
        app.useCurrentArchiveForRecovery();
        archive.installArchivePreview(archiveInfo(1), outerRows, { selected: ["inner.zip"] });
      } else if (action === "missing-default") app.useDefaultPar2ForRecovery();
      else if (action === "unavailable-task") {
        assert.equal(app.adoptRecoveryTargetFromTask({ spec: { kind: "repair_zip", src: "squallz-archive://7" } },
          { kind: "repair_zip", src: "/displayed/outer.zip" }), false);
      } else {
        assert.equal(app.adoptRecoveryTargetFromTask({ spec: { kind: "repair_recovery",
          path: "/new/photos.zip", recovery: "/new/photos.par2" } }), true);
      }
      const cancelled = action === "current-route" || action === "valid-task";
      assert.equal(context.recoveryPickerStatus, cancelled ? "idle" : kind);
      assert.equal(context.recoveryPickerRequest, request + Number(cancelled));
      const kept = recovery();
      const noticeCount = notices.length;
      finish(picker, kind, "choosing", "selected");
      await picker.choosing;
      assert.equal(context.recoveryPickerStatus, "idle");
      if (cancelled) {
        assert.deepEqual(recovery(), kept);
        assert.equal(notices.length, noticeCount);
        if (action === "current-route") assert.equal(context.recoverySourceMode, "current");
      } else {
        assert.equal(kind === "archive" ? context.recoverySourceOverride : context.recoveryPar2Override,
          `/late/old.${kind === "archive" ? "zip" : "par2"}`);
        assert.equal(notices.length, noticeCount + 1);
      }
    }
    app.openRecoverySet("/previous/photos.par2", "/previous/photos.zip", "open-file");
    const picker = begin("archive", "choosing");
    await picker.started.promise;
    const replacement = deferred();
    ipc.openArchive = () => replacement.promise;
    const opening = app.openArchivePath("/archives/replacement.zip", "open-file");
    await expectDiscarded(picker, "archive", "choosing", "selected");
    replacement.resolve(archiveInfo(3, "replacement.zip"));
    page.resolve({ items: [], total: 0, page: 0 });
    await opening;
    assert.equal(archive.archive().id, 3);
    assert.equal(context.screen, "browse");
  });
});

test("recovery output preparation preserves one snapshot and discards superseded dialogs and checks", { timeout: 15_000 }, async () => {
  await withNestedOpen(async ({ app, archive, ipc, context, notices, operations }) => {
    const submitted = [];
    const nativeRequests = [];
    context.submitJob = async (spec) => { submitted.push(JSON.parse(JSON.stringify(spec))); return submitted.length; };
    context.recordNativeDialogRequest = (purpose, options) => nativeRequests.push({ purpose, ...options });
    const dialogs = { open: async () => null, save: async () => null, confirm: async () => true };
    const prepare = (extension, sourceFileCount = 1) => {
      const info = { ...archiveInfo(1, `A.${extension}`), format: extension };
      archive.installArchivePreview(info, outerRows);
      app.openRecoverySet(`${info.path}.par2`, info.path, "open-file");
      context.jobRows = [{ id: 1, state: "done", spec: { kind: "verify_recovery", path: info.path, recovery: `${info.path}.par2` },
        result: { operation: "verify", ok: false, source_file_count: sourceFileCount,
          metrics: { repair_possible: true, blocks_needed: 1, recovery_blocks_available: 3 } } }];
      context.getDialogModule = async () => dialogs;
      ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
      context.compressionLevel = 2;
      context.activeCreateProfile = "balanced";
      dialogs.open = async () => null;
      dialogs.save = async () => null;
      dialogs.confirm = async () => true;
      return info.path;
    };
    const savePending = () => {
      const response = deferred();
      const started = deferred();
      dialogs.save = () => { started.resolve(); return response.promise; };
      return { response, started };
    };
    const expectSilent = async (work, finish) => {
      const noticeCount = notices.length;
      const operationCount = operations.length;
      const submittedCount = submitted.length;
      finish();
      await work;
      assert.equal(submitted.length, submittedCount, "a superseded preparation must never queue a task");
      assert.equal(notices.length, noticeCount, "late selection, cancellation and errors must not replace current feedback");
      assert.equal(operations.length, operationCount);
    };
    const awaitStarted = (started, work, stage) => Promise.race([
      started.promise,
      work.then(() => { throw new Error(`${stage} did not start: ${notices.at(-1)}`); }),
    ]);

    prepare("zip");
    const par2Save = savePending();
    const par2Repair = app.submitRecoveryOutputJob("repair_recovery");
    await awaitStarted(par2Save.started, par2Repair, "PAR2 save");
    assert.equal(context.recoveryOutputPreparation.phase, "choosing");
    app.openRecoverySet("/recovery/B.zip.par2", "/recovery/B.zip", "open-file");
    assert.match(app.recoveryRepairPar2DisabledReason(), /Verify this archive/);
    assert.equal(context.recoveryOutputPreparation, null);
    await expectSilent(par2Repair, () => par2Save.response.resolve("/output/A.repaired.zip"));
    assert.equal(context.recoveryPar2Override, "/recovery/B.zip.par2");

    prepare("zip");
    const zipSave = savePending();
    const zipRepair = app.submitRecoveryOutputJob("repair_zip");
    await awaitStarted(zipSave.started, zipRepair, "ZIP save");
    app.setScreen("browse");
    app.setScreen("recovery");
    await expectSilent(zipRepair, () => zipSave.response.resolve("/output/A.rebuilt.zip"));
    assert.equal(context.recoveryOutputPreparation, null, "leaving and returning must not revive the old request");

    prepare("sqz");
    const module = deferred();
    const moduleStarted = deferred();
    context.getDialogModule = () => { moduleStarted.resolve(); return module.promise; };
    const sqzRepair = app.submitRecoveryOutputJob("repair_sqz");
    await awaitStarted(moduleStarted, sqzRepair, "SQZ module");
    const nativeCount = nativeRequests.length;
    app.openRecoverySet("/recovery/B.sqz.par2", "/recovery/B.sqz", "open-file");
    await expectSilent(sqzRepair, () => module.resolve(dialogs));
    assert.equal(nativeRequests.length, nativeCount, "a superseded module load must not open a save dialog");

    prepare("sqz");
    app.useCurrentArchiveForRecovery();
    const inspection = deferred();
    const inspectionStarted = deferred();
    const confirms = [];
    dialogs.save = async () => "/output/A.zip";
    dialogs.confirm = async (...args) => { confirms.push(args); return true; };
    ipc.inspectCreateDestination = () => { inspectionStarted.resolve(); return inspection.promise; };
    const exporting = app.submitRecoveryOutputJob("export_sqz");
    await awaitStarted(inspectionStarted, exporting, "export inspection");
    assert.equal(context.recoveryOutputPreparation.phase, "checking");
    dialogs.open = async () => "/recovery/B.par2";
    await app.chooseRecoveryPar2();
    assert.equal(context.recoveryOutputPreparation, null, "an accepted source picker must cancel output preparation");
    await expectSilent(exporting, () => inspection.resolve({ conflict: true, guard: { token: "existing-output" } }));
    assert.equal(confirms.length, 0, "an obsolete inspection must not open an overwrite confirmation");

    prepare("sqz");
    app.useCurrentArchiveForRecovery();
    const confirmation = deferred();
    const confirmationStarted = deferred();
    dialogs.save = async () => "/output/existing.zip";
    ipc.inspectCreateDestination = async () => ({ conflict: true, guard: { token: "existing-output" } });
    dialogs.confirm = () => { confirmationStarted.resolve(); return confirmation.promise; };
    const awaitingConfirmation = app.submitRecoveryOutputJob("export_sqz");
    await awaitStarted(confirmationStarted, awaitingConfirmation, "export confirmation");
    app.openRecoverySet("/recovery/B.sqz.par2", "/recovery/B.sqz", "open-file");
    await expectSilent(awaitingConfirmation, () => confirmation.resolve(true));
    assert.equal(context.recoveryOutputPreparation, null, "late approval must not authorize a departed source");

    prepare("zip");
    dialogs.save = async () => "/output/existing.zip";
    ipc.inspectCreateDestination = async () => ({ conflict: true, guard: { token: "existing-output" } });
    const conflictNotices = notices.length;
    await app.submitRecoveryOutputJob("repair_zip");
    assert.equal(submitted.length, 0, "repair must keep an existing destination without queueing a task");
    assert.equal(context.recoveryOutputPreparation, null);
    assert.equal(notices.length, conflictNotices + 1);
    assert.match(notices.at(-1), /already exists.*Choose a different name or folder/);

    for (const kind of ["repair_sqz", "repair_zip", "repair_recovery", "export_sqz"]) {
      const source = prepare(kind === "repair_sqz" || kind === "export_sqz" ? "sqz" : "zip");
      if (kind === "export_sqz") app.useCurrentArchiveForRecovery();
      const save = savePending();
      const work = app.submitRecoveryOutputJob(kind);
      await awaitStarted(save.started, work, `${kind} save`);
      const request = context.recoveryOutputPreparation;
      const nativeCount = nativeRequests.length;
      await app.submitRecoveryOutputJob(kind);
      assert.equal(context.recoveryOutputPreparation, request);
      assert.equal(nativeRequests.length, nativeCount, "repeated submission must not open another chooser");
      context.compressionLevel = 9;
      context.activeCreateProfile = "fast";
      app.openRecoveryConfiguration("preserve");
      assert.equal(context.recoveryOutputPreparation, request, "an unchanged route must preserve preparation");
      const destination = `/output/${kind}.${kind === "repair_sqz" ? "sqz" : "zip"}`;
      save.response.resolve(destination);
      await work;
      assert.equal(context.recoveryOutputPreparation, null);
      const spec = submitted.at(-1);
      assert.equal(spec.kind, kind);
      if (kind === "repair_recovery") {
        assert.equal(spec.path, source);
        assert.equal(spec.output, destination);
        assert.equal(spec.output_directory, false);
        assert.equal(spec.recovery, `${source}.par2`);
      } else {
        assert.equal(spec.src, source);
        assert.equal(spec.dest, destination);
        assert.equal(spec.level, 2, "later draft edits must not change the accepted operation snapshot");
      }
      if (kind === "export_sqz") {
        assert.equal(spec.replace_existing, false);
        assert.equal(spec.replacement_guard, null);
        assert.match(operations.at(-1).detail, /balanced/);
      }
    }
    assert.equal(submitted.length, 4, "each current operation must still queue exactly once");

    prepare("zip");
    const oldSave = savePending();
    const oldRepair = app.submitRecoveryOutputJob("repair_zip");
    await awaitStarted(oldSave.started, oldRepair, "old ZIP save");
    app.openRecoverySet("/recovery/B.zip.par2", "/recovery/B.zip", "open-file");
    const nextSave = savePending();
    const nextRepair = app.submitRecoveryOutputJob("repair_zip");
    await awaitStarted(nextSave.started, nextRepair, "replacement ZIP save");
    const currentRequest = context.recoveryOutputPreparation;
    await expectSilent(oldRepair, () => oldSave.response.reject(new Error("old chooser unavailable")));
    assert.equal(context.recoveryOutputPreparation, currentRequest, "old finally must not release a newer request");
    assert.equal(currentRequest.phase, "choosing");
    nextSave.response.resolve(null);
    await nextRepair;
    assert.equal(context.recoveryOutputPreparation, null);
    assert.match(notices.at(-1), /ZIP index rebuild cancelled/);

    prepare("zip", 2);
    const unique = deferred();
    const uniqueStarted = deferred();
    dialogs.open = async () => "/output";
    ipc.uniqueCreateDestination = (proposed) => { uniqueStarted.resolve(proposed); return unique.promise; };
    const setRepair = app.submitRecoveryOutputJob("repair_recovery");
    assert.match(await awaitStarted(uniqueStarted, setRepair, "PAR2 unique destination"), /A/);
    assert.equal(app.adoptRecoveryTargetFromTask({ spec: { kind: "repair_zip", src: "/recovery/B.zip" } }), true);
    await expectSilent(setRepair, () => unique.resolve("/output/A.repaired"));
    assert.equal(context.recoveryOutputPreparation, null);

    prepare("zip");
    context.getDialogModule = async () => { throw new Error("current chooser unavailable"); };
    const noticeCount = notices.length;
    await app.submitRecoveryOutputJob("repair_zip");
    assert.equal(context.recoveryOutputPreparation, null);
    assert.equal(submitted.length, 4);
    assert.equal(notices.length, noticeCount + 1, "a current failure must retain its recovery feedback");
    assert.match(notices.at(-1), /Check the save location and try again/);
  });
});

test("leaving a workspace cancels preparation and discards late files, nested previews, passwords and errors", async () => {
  for (const [kind, origin] of [["file", "browse"], ["preview", "browse"], ["open", "browse"], ["file", "recovery"]]) {
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
        const preparing = kind === "file" ? app.submitPreviewEntry("notes.txt", "file", 1)
          : kind === "preview" ? app.submitPreviewNestedArchive("inner.zip", 0)
            : app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
        await started.promise;
        const target = origin === "recovery" ? "browse" : kind === "preview" ? "recovery" : "settingsGeneral";
        app.setScreen(target);
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
    context.entryPreview = { preview_id: "ready-file" };
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
    context.nestedPreview = { archive: archiveInfo(2, "inner.zip") };
    app.setScreen("recovery");
    assert.equal(context.nestedPreview, null);
    assert.deepEqual(closed, [2]);
  });
});

test("closing, navigating or selecting another item during nested listing prevents late navigation and errors", async () => {
  for (const action of ["close", "select", "navigate", "leave-return"]) {
    for (const result of ["success", "failure"]) {
      await withNestedOpen(async ({ app, archive, closed, page, pageRequested, context, notices, operations }) => {
        const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
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
        assert.equal(context.recoverySourceOverride, "/previous.zip");
      });
    }
  }
});

test("a current nested listing failure remains retryable and success replaces the archive once", async () => {
  await withNestedOpen(async ({ app, archive, ipc, closed, page, pageRequested, context, notices, operations }) => {
    const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
    await pageRequested.promise;
    page.reject(new Error("list failed"));
    await opening;
    assert.equal(archive.archive().id, 1);
    assert.equal(context.entryPreviewFailure.entryPath, "inner.zip");
    assert.equal(context.entryPreviewFailure.retryAction, "open");
    assert.equal(context.previewPhase, "idle");
    assert.equal(notices.length, 1);
    ipc.openNestedArchive = async () => archiveInfo(3, "inner.zip");
    ipc.listEntries = async () => ({ items: [outerRows[1]], total: 1, page: 0 });
    await app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(archive.loadedRows().map((row) => row.path), ["notes.txt"]);
    assert.deepEqual(closed, [2, 1]);
    assert.equal(context.entryPreviewFailure, null);
    assert.equal(context.previewPhase, "idle");
    assert.equal(context.recoverySourceOverride, null);
    assert.equal(operations.length, 1);
    assert.match(notices.at(-1), /Opened nested archive/);
  });
});

test("a late nested listing cannot replace a newer inner archive or clear its result", async () => {
  await withNestedOpen(async ({ app, archive, ipc, closed, page, pageRequested, operations, notices }) => {
    const first = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
    await pageRequested.promise;
    ipc.openNestedArchive = async () => archiveInfo(3, "newer.zip");
    ipc.listEntries = async () => ({ items: [outerRows[1]], total: 1, page: 0 });
    await app.openNestedArchiveEntry("/archives/outer.zip", "newer.zip", 1);
    page.resolve({ items: [], total: 0, page: 0 });
    await first;
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(closed, [1, 2]);
    assert.equal(operations.length, 1);
    assert.equal(notices.length, 1);
    assert.match(notices[0], /newer.zip/);
  });
});

test("late system-open responses cannot revive a dismissed or replaced preview", async () => {
  const server = await createTestServer();

  try {
    const { previewResponseIsCurrent } = await server.ssrLoadModule(
      "/src/lib/preview-response.ts",
    );
    const expected = {
      previewGeneration: 7,
      actionGeneration: 3,
      previewId: "preview-a",
      archiveSource: "/archives/a.zip",
    };

    assert.equal(previewResponseIsCurrent(expected, expected), true);
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, previewGeneration: 8 }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, actionGeneration: 4 }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, previewId: "preview-b" }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, archiveSource: "/archives/b.zip" }),
      false,
    );
  } finally {
    await server.close();
  }
});

test("opening a nested preview adopts its existing handle without another extraction or password request", async () => {
  await withNestedOpen(async ({ app, archive, ipc, page, context, closed }) => {
    context.nestedPreview = {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip",
      archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
    };
    ipc.openNestedArchive = async () => { assert.fail("the prepared archive must be reused"); };
    page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
    await app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip");
    assert.equal(archive.archive().id, 2);
    assert.equal(context.nestedPreview, null);
    assert.equal(context.previewPasswordPrompt, null);
    assert.deepEqual(closed, [1]);
  });
});

test("extracting a nested preview opens the shared extraction workspace with the verified source", async () => {
  await withNestedOpen(async ({ app, archive, ipc, page, context, closed }) => {
    const inner = { ...archiveInfo(2, "inner.zip"), source: "squallz-archive://2" };
    context.nestedPreview = {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip",
      archive: inner, items: [], truncated: false,
    };
    ipc.openNestedArchive = async () => { assert.fail("do not extract the inner archive again"); };
    context.focusBlockingTaskIfAny = () => true;
    await app.extractNestedPreviewArchive();
    assert.equal(archive.archive().id, 1);
    assert.equal(context.nestedPreview.archive.id, 2);
    assert.deepEqual(closed, []);
    context.focusBlockingTaskIfAny = () => false;
    page.resolve({ items: [outerRows[1]], total: 1, page: 0 });
    await app.extractNestedPreviewArchive();
    assert.equal(archive.archive().source, inner.source);
    assert.equal(context.screen, "extract");
    assert.equal(context.extractScope, "all");
    assert.equal(context.extractFocused, true);
    assert.equal(context.previewPasswordPrompt, null);
    assert.equal(context.nestedPreview, null);
    assert.deepEqual(closed, [1]);
  });
});

test("leaving or dismissing during preview extraction keeps the current archive and releases the inner source", async () => {
  for (const action of ["leave", "leave-return", "dismiss"]) {
    await withNestedOpen(async ({ app, archive, page, pageRequested, context, closed }) => {
      context.nestedPreview = {
        outer_path: "/archives/outer.zip", entry_path: "inner.zip",
        archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
      };
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
    context.nestedPreview = {
      outer_path: "/archives/outer.zip", entry_path: "inner.zip",
      archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
    };
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
    await withNestedOpen(async ({ app, archive, page, pageRequested, context, closed }) => {
      context.nestedPreview = {
        outer_path: "/archives/outer.zip", entry_path: "inner.zip",
        archive: archiveInfo(2, "inner.zip"), items: [], truncated: false,
      };
      let opening;
      if (duringAdoption) {
        opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip");
        await pageRequested.promise;
      }
      app.clearEntryPreviewState();
      app.clearEntryPreviewState();
      page.resolve({ items: [], total: 0, page: 0 });
      await opening;
      assert.equal(archive.archive().id, 1);
      assert.deepEqual(closed, [2]);
    });
  }
});

test("a late nested preview releases the prepared archive after dismissal", async () => {
  await withNestedOpen(async ({ app, archive, ipc, context, closed }) => {
    const pending = deferred();
    const requested = deferred();
    ipc.previewNestedArchive = async () => { requested.resolve(); return pending.promise; };
    const previewing = app.submitPreviewNestedArchive("inner.zip", 0);
    await requested.promise;
    app.clearEntryPreviewState();
    pending.resolve({ outer_path: "/archives/outer.zip", entry_path: "inner.zip", archive: archiveInfo(2, "inner.zip"), items: [], truncated: false });
    await previewing;
    assert.equal(archive.archive().id, 1);
    assert.equal(context.nestedPreview, null);
    assert.deepEqual(closed, [2]);
  });
});
