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
    ipc.closeArchive = async (id) => { closed.push(id); };
    ipc.cancelArchiveOpen = async () => {};
    ipc.cancelEntryPreview = async () => {};
    ipc.cancelArchiveSearch = async () => {};
    ipc.releasePreviewSession = async () => true;
    ipc.openNestedArchive = async () => archiveInfo(2, "inner.zip");
    const pageRequested = deferred();
    const page = deferred();
    ipc.listEntries = async () => { pageRequested.resolve(); return page.promise; };
    archive.installArchivePreview(archiveInfo(1), outerRows, { selected: ["inner.zip"] });
    const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
    const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
    const { createPreviewPasswordFlow } = await server.ssrLoadModule("/src/lib/preview-password.svelte.ts");
    const names = ["openNestedArchiveEntry", "runPreviewWithPassword", "clearEntryPreviewState", "selectOnlyEntry", "submitPasswordRequest", "cancelPasswordRequest", "passwordPromptDetail", "submitPreviewEntry", "submitPreviewNestedArchive", "prepareEntryPreviewSerially", "disposeEntryPreview"];
    const declarations = names.map((name) => {
      const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
      assert.ok(declaration, name);
      return declaration.getText(source);
    });
    const notices = [];
    const operations = [];
    const context = {
      ipc, adoptOpenedArchive: archive.adoptOpenedArchive,
      previewPasswordFlow: createPreviewPasswordFlow(ipc.cancelEntryPreview), screen: "browse",
      jobPasswordPrompt: null, jobConflictPrompt: null, archivePasswordPrompt: null,
      jobPasswordValue: "", passwordSubmissionAttempted: false,
      taskPasswordReady: (value) => value.length > 0, focusArchiveRow: async () => {},
      queueMicrotask,
      entryPreviewPreparationTail: Promise.resolve(), previewSampleForEntry: () => null,
      archiveLikePath: (path) => path.endsWith(".zip"), blockSelectionScopedAction: () => false,
      previewEntryDisplayName: (path) => path.split("/").at(-1), recordValidationEvent: () => {},
      setScreen: (next) => { context.screen = next; },
      previewOriginEntryPath: null, previewOriginVirtualIndex: null,
      previewRequestGeneration: 0, previewActionGeneration: 0,
      nestedPreview: null, entryPreview: null, entryPreviewFailure: null,
      previewPhase: "idle", previewTargetName: "",
      params: new URLSearchParams(), nestedPasswordPreviewSample: () => null,
      recoverySourceMode: "file", recoverySourceOverride: "/previous.zip", recoveryPar2Override: "/previous.par2",
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
    await run({ app, archive, ipc, closed, page, pageRequested, context, notices, operations });
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
    context.screen = "password";
    context.jobPasswordValue = "entry-password";
    await app.submitPasswordRequest();
    await opening;
    assert.deepEqual(opened, ["file-preview"]);
    assert.deepEqual(released, ["file-preview"]);
    assert.equal(context.screen, "browse");
    assert.equal(context.entryPreviewFailure, null);
    assert.equal(context.jobPasswordValue, "");
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
    context.screen = "password";
    assert.match(app.passwordPromptDetail(), /archive password to read/);
    for (const [password, nextScope] of [["outer-password", "inner"], ["wrong-password", "inner"], ["inner-password", null]]) {
      assert.equal(archive.archive().id, 1);
      context.jobPasswordValue = password;
      await app.submitPasswordRequest();
      assert.equal(context.jobPasswordValue, "");
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
    context.jobPasswordValue = "outer-password";
    await app.submitPasswordRequest();
    await app.cancelPasswordRequest();
    verified.resolve(archiveInfo(2, "inner.zip"));
    await opening;
    assert.equal(context.previewPasswordPrompt, null);
    assert.equal(context.jobPasswordValue, "");
    assert.equal(context.screen, "browse");
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(closed, [2]);
    assert.equal(context.entryPreviewFailure, null);
  });
});

test("closing or selecting another item during nested listing prevents late navigation and errors", async () => {
  for (const action of ["close", "select"]) {
    for (const result of ["success", "failure"]) {
      await withNestedOpen(async ({ app, archive, closed, page, pageRequested, context, notices, operations }) => {
        const opening = app.openNestedArchiveEntry("/archives/outer.zip", "inner.zip", 0);
        await pageRequested.promise;
        assert.equal(archive.archive().id, 1);
        if (action === "select") app.selectOnlyEntry({ source: outerRows[1], virtualIndex: 1 });
        else app.clearEntryPreviewState();
        if (result === "success") page.resolve({ items: [], total: 0, page: 0 });
        else page.reject(new Error("list failed"));
        await opening;
        assert.equal(archive.archive().id, 1, `${action} keeps the outer archive after ${result}`);
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
