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
    ipc.cancelArchiveSearch = async () => {};
    ipc.releasePreviewSession = async () => true;
    ipc.openNestedArchive = async () => archiveInfo(2, "inner.zip");
    const pageRequested = deferred();
    const page = deferred();
    ipc.listEntries = async () => { pageRequested.resolve(); return page.promise; };
    archive.installArchivePreview(archiveInfo(1), outerRows, { selected: ["inner.zip"] });
    const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
    const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
    const names = ["openNestedArchiveEntry", "clearEntryPreviewState", "selectOnlyEntry"];
    const declarations = names.map((name) => {
      const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
      assert.ok(declaration, name);
      return declaration.getText(source);
    });
    const notices = [];
    const operations = [];
    const context = {
      ipc, adoptOpenedArchive: archive.adoptOpenedArchive,
      previewOriginEntryPath: null, previewOriginVirtualIndex: null,
      previewRequestGeneration: 0, previewActionGeneration: 0,
      nestedPreview: null, entryPreview: null, entryPreviewFailure: null,
      previewPhase: "idle", previewTargetName: "",
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
    const { outputText } = ts.transpileModule(declarations.join("\n"), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    });
    const app = vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context);
    await run({ app, archive, ipc, closed, page, pageRequested, context, notices, operations });
  } finally {
    await server.close();
  }
}

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
