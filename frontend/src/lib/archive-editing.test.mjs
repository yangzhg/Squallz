import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

async function loadEditing(overrides = {}) {
  const server = await createTestServer();
  let helpers;
  try {
    helpers = await server.ssrLoadModule("/src/lib/archive-editing.ts");
  } finally {
    await server.close();
  }
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const names = new Set([
    "normalizeNewFolderPath", "commitNewFolderName", "submitNewFolderJob", "archivePathSet",
    "normalizeMoveTargetDir", "submitMovePlan", "buildMovePlan", "moveTargetForPath", "uniqueArchiveTarget",
    "archiveEditPathProblem", "moveTargetProblem", "submitMoveSelectedJob",
    "normalizeRenameTargetName", "selectedRenameSource", "renameTargetIssue", "archiveEntryExtension",
    "submitRenameSelectedJob",
    "openArchiveEditor",
  ]);
  const declarations = source.statements.filter(
    (node) => ts.isFunctionDeclaration(node) && names.has(node.name?.text),
  );
  const { outputText } = ts.transpileModule(
    declarations.map((node) => node.getText(source)).join("\n"),
    { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } },
  );
  const submitted = [];
  const notices = [];
  const closed = [];
  const context = {
    ...helpers,
    currentArchive: { id: 17, source: "/tmp/archive-editing.zip" },
    archiveDirs: ["docs"],
    newFolderName: "计划",
    renameTargetName: "重命名",
    moveTargetDir: "destination/",
    moveConflictReview: null,
    archiveEditKind: null,
    archiveEditReturnFocus: null,
    closeArchiveEditor: () => { closed.push(true); context.archiveEditKind = null; },
    renameSelectedDisabledReason: () => "",
    moveSelectedDisabledReason: () => "",
    openArchiveFirstLabel: () => "Open an archive first",
    document: { activeElement: null },
    HTMLElement: class {},
    setScreen: () => {},
    loadedRows: () => [{ path: "destination/", entry_type: "dir" }],
    selectedPaths: () => new Set(["docs/a.txt"]),
    pathBaseName: (path) => path.split("/").at(-1),
    blockSelectionScopedAction: () => false,
    archiveMutationDisabledReason: () => "",
    showNotice: (message) => notices.push(message),
    recordOperation: () => {},
    tr: (_key, fallback) => fallback,
    submitCurrentArchiveJob: async (spec) => { submitted.push(spec); return true; },
    ...overrides,
  };
  const handlers = vm.runInNewContext(`${outputText}\n({ normalizeNewFolderPath, commitNewFolderName, submitNewFolderJob, normalizeMoveTargetDir, submitMoveSelectedJob, submitMovePlan, buildMovePlan, normalizeRenameTargetName, submitRenameSelectedJob, openArchiveEditor })`, context);
  return { ...handlers, submitted, notices, closed, context };
}

test("new folders are created inside the displayed archive directory", async () => {
  const editing = await loadEditing();
  await editing.submitNewFolderJob();
  assert.deepEqual(Array.from(editing.submitted[0].mkdir), ["docs/计划/"]);
});

test("moving entries does not try to recreate the destination directory", async () => {
  const editing = await loadEditing();
  const rename = [{ from: "docs/a.txt", to: "destination/a.txt" }];
  await editing.submitMovePlan(rename, "destination/");
  assert.deepEqual(Array.from(editing.submitted[0].mkdir), []);
  assert.equal(editing.submitted[0].rename, rename);
});

test("an archive-root move keeps the root destination", async () => {
  const editing = await loadEditing({ moveTargetDir: "/" });
  assert.equal(editing.normalizeMoveTargetDir(), "");
});

test("moving a selected directory does not separately flatten its selected children", async () => {
  const editing = await loadEditing({
    selectedPaths: () => new Set(["docs/", "docs/a.txt", "docs/sub/", "docs/sub/b.txt", "other.txt"]),
  });
  assert.deepEqual(Array.from(editing.buildMovePlan(), ({ from, to }) => [from, to]), [
    ["docs/", "destination/docs/"],
    ["other.txt", "destination/other.txt"],
  ]);
});

test("folder input commits do not add the parent twice and support an explicit archive-root path", async () => {
  const editing = await loadEditing();
  editing.commitNewFolderName("季度/计划");
  assert.equal(editing.normalizeNewFolderPath(), "docs/季度/计划/");
  editing.commitNewFolderName();
  assert.equal(editing.normalizeNewFolderPath(), "docs/季度/计划/");
  editing.commitNewFolderName("/共享/计划");
  assert.equal(editing.normalizeNewFolderPath(), "共享/计划/");
});

test("unsafe folder and move inputs are explained without queuing altered paths", async () => {
  for (const value of ["../escape", "safe/../escape", "NUL", "folder/file?.txt", "folder. /child"]) {
    const editing = await loadEditing({ newFolderName: value, moveTargetDir: value });
    await editing.submitNewFolderJob();
    await editing.submitMoveSelectedJob();
    assert.equal(editing.submitted.length, 0, value);
    assert.equal(editing.notices.length, 2, value);
  }
});

test("moves into the selected subtree or the same folder do not become keep-both operations", async () => {
  for (const [source, destination] of [["docs/", "docs/sub/"], ["docs/a.txt", "docs/"]]) {
    const editing = await loadEditing({ selectedPaths: () => new Set([source]), moveTargetDir: destination });
    await editing.submitMoveSelectedJob();
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.context.moveConflictReview, null);
    assert.equal(editing.notices.length, 1);
  }
});

test("directory rename submits one subtree mapping and keeps the current parent", async () => {
  const editing = await loadEditing({ selectedPaths: () => new Set(["docs/reports/"]) });
  assert.equal(editing.normalizeRenameTargetName(), "docs/重命名/");
  await editing.submitRenameSelectedJob();
  assert.deepEqual(JSON.parse(JSON.stringify(editing.submitted[0].rename)), [{
    from: "docs/reports/", to: "docs/重命名/",
  }]);
});

test("empty, unchanged, unsafe, or descendant rename targets do not queue updates", async () => {
  for (const value of ["", "reports", "../escape", "docs/reports/inside", "docs/NUL"]) {
    const editing = await loadEditing({ selectedPaths: () => new Set(["docs/reports/"]), renameTargetName: value });
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 0, value);
    assert.equal(editing.notices.length, 1, value);
  }
});

test("archive edit actions open a fresh form and queue only after explicit submission", async () => {
  const editing = await loadEditing();
  editing.openArchiveEditor("rename");
  assert.equal(editing.context.archiveEditKind, "rename");
  assert.equal(editing.context.renameTargetName, "a.txt");
  assert.equal(editing.submitted.length, 0);
  editing.context.renameTargetName = "renamed.txt";
  await editing.submitRenameSelectedJob();
  assert.equal(editing.submitted.length, 1);
  assert.equal(editing.context.archiveEditKind, null);
  assert.equal(editing.closed.length, 1);
  editing.openArchiveEditor("new-folder");
  assert.equal(editing.context.newFolderName, "");
  assert.equal(editing.submitted.length, 1);
});

test("read-only edit entrypoints are blocked and failed submissions retain the form", async () => {
  const blocked = await loadEditing({ archiveMutationDisabledReason: () => "Read only" });
  blocked.openArchiveEditor("new-folder");
  assert.equal(blocked.context.archiveEditKind, null);
  assert.deepEqual(blocked.notices, ["Read only"]);
  const failed = await loadEditing({ submitCurrentArchiveJob: async () => false });
  failed.openArchiveEditor("rename");
  failed.context.renameTargetName = "renamed.txt";
  await failed.submitRenameSelectedJob();
  assert.equal(failed.context.archiveEditKind, "rename");
  assert.equal(failed.closed.length, 0);
});

test("the shared archive editor renders one named modal form", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Editor } = await server.ssrLoadModule("/src/components/ArchiveEntryEditor.svelte");
    const { body } = render(Editor, { props: {
      title: "Rename selected", label: "Rename target name", value: "资料", status: "reports/ -> 资料/",
      cancelLabel: "Cancel", rootClass: "archive-editor-overlay design-root", rootVariables: {},
      onChange: () => {}, onSubmit: async () => {}, onClose: () => {},
    } });
    assert.match(body, /role="dialog" aria-modal="true"/u);
    assert.equal((body.match(/<form\b/gu) ?? []).length, 1);
    assert.equal((body.match(/<input\b/gu) ?? []).length, 1);
    assert.match(body, /role="status"/u);
    assert.match(body, /type="submit"/u);
    assert.doesNotMatch(body, /style=/u);
  } finally { await server.close(); }
});

test("modal focus wraps at both ends and handles an empty panel", async () => {
  const server = await createTestServer();
  try {
    const { trapModalFocus } = await server.ssrLoadModule("/src/lib/modal-focus.ts");
    const doc = { activeElement: null };
    const first = { hasAttribute: () => false, focus: () => { doc.activeElement = first; } };
    const last = { hasAttribute: () => false, focus: () => { doc.activeElement = last; } };
    let elements = [first, last];
    const root = { ownerDocument: doc, querySelectorAll: () => elements, contains: (node) => elements.includes(node), focus: () => { doc.activeElement = root; } };
    let prevented = 0;
    const tab = (shiftKey = false) => trapModalFocus({ key: "Tab", shiftKey, preventDefault: () => { prevented += 1; } }, root);
    doc.activeElement = first;
    tab(true);
    assert.equal(doc.activeElement, last);
    tab();
    assert.equal(doc.activeElement, first);
    doc.activeElement = root;
    tab();
    assert.equal(doc.activeElement, first);
    elements = [];
    tab();
    assert.equal(doc.activeElement, root);
    assert.equal(prevented, 4);
  } finally { await server.close(); }
});
