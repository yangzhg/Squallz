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
    helpers = { ...helpers, isErrorDto: (await server.ssrLoadModule("/src/lib/ipc.ts")).isErrorDto };
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
    "submitRenameSelectedJob", "selectedDeletePaths", "submitDeleteSelectedJob", "submitCurrentArchiveJob",
    "openArchiveEditor", "submitAddToArchiveJob",
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
  const toasts = [];
  const closed = [];
  const context = {
    ...helpers,
    currentArchive: { id: 17, source: "/tmp/archive-editing.zip", encoding_override: "gbk" },
    archiveOpenGeneration: 0,
    archiveOpenStatus: "idle",
    archiveAddPending: false,
    createContentPolicy: "custom",
    createExcludeRules: () => ["*.bak"],
    createCompressionLevel: () => 8,
    activeCreateProfile: "custom",
    createProfileLabel: (value) => value,
    getDialogModule: async () => ({ open: async () => ["/inputs/资料.txt"] }),
    openNativeDialog: async (_kind, open, options) => open(options),
    archiveTitle: () => "archive-editing.zip",
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
    pushToast: (toast) => toasts.push(toast),
    tError: (error) => error.key,
    focusBlockingTaskIfAny: () => false,
    isJobSubmitBlocked: () => false,
    recordOperation: () => {},
    tr: (_key, fallback) => fallback,
    submitJob: async (spec) => { submitted.push(spec); return 1; },
    ...overrides,
  };
  const handlers = vm.runInNewContext(`${outputText}\n({ normalizeNewFolderPath, commitNewFolderName, submitNewFolderJob, normalizeMoveTargetDir, submitMoveSelectedJob, submitMovePlan, buildMovePlan, normalizeRenameTargetName, submitRenameSelectedJob, submitDeleteSelectedJob, openArchiveEditor, submitAddToArchiveJob })`, context);
  return { ...handlers, submitted, notices, toasts, closed, context };
}

test("new folders are created inside the displayed archive directory", async () => {
  const editing = await loadEditing();
  await editing.submitNewFolderJob();
  assert.deepEqual(Array.from(editing.submitted[0].mkdir), ["docs/计划/"]);
});

test("adding files never follows an archive switch while the native picker is pending", async () => {
  for (const change of ["switch", "close", "reopen", "opening", "encoding", "read-only", "refresh"]) {
    const editing = await loadEditing();
    let select;
    editing.context.getDialogModule = async () => ({ open: () => new Promise((resolve) => { select = resolve; }) });
    const pending = editing.submitAddToArchiveJob();
    await new Promise(setImmediate);
    if (change === "switch") editing.context.currentArchive = { id: 18, source: "/other.zip", encoding_override: null };
    if (change === "close") editing.context.currentArchive = null;
    if (change === "reopen") editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
    if (change === "opening") editing.context.archiveOpenGeneration++;
    if (change === "encoding") editing.context.currentArchive.encoding_override = "shift_jis";
    if (change === "read-only" || change === "refresh") editing.context.archiveMutationDisabledReason = () => change;
    select(["/inputs/资料.txt"]);
    await pending;
    assert.equal(editing.submitted.length, 0, change);
    assert.ok(editing.notices.length > 0, change);
    assert.equal(editing.context.archiveAddPending, false);
  }
});

test("archive changes before dialog loading finishes prevent an obsolete picker", async () => {
  const editing = await loadEditing();
  let loaded;
  let opened = 0;
  editing.context.getDialogModule = () => new Promise((resolve) => { loaded = resolve; });
  const pending = editing.submitAddToArchiveJob();
  editing.context.archiveOpenGeneration++;
  loaded({ open: async () => { opened++; return ["/inputs/资料.txt"]; } });
  await pending;
  assert.equal(opened, 0);
  assert.equal(editing.submitted.length, 0);
});

test("adding files captures settings once and rejects duplicate selection and submission", async () => {
  const operations = [];
  const editing = await loadEditing({ recordOperation: (operation) => operations.push(operation) });
  let select;
  let submitted;
  let opened = 0;
  editing.context.getDialogModule = async () => ({ open: () => { opened++; return new Promise((resolve) => { select = resolve; }); } });
  editing.context.submitJob = (spec) => { editing.submitted.push(spec); return new Promise((resolve) => { submitted = resolve; }); };
  const pending = editing.submitAddToArchiveJob();
  await new Promise(setImmediate);
  assert.equal(editing.context.archiveAddPending, true);
  await editing.submitAddToArchiveJob();
  assert.equal(opened, 1);
  editing.context.createContentPolicy = "keep_all_files";
  editing.context.createCompressionLevel = () => 0;
  editing.context.createExcludeRules = () => ["*.txt"];
  editing.context.activeCreateProfile = "fast";
  select(["/inputs/资料.txt"]);
  await new Promise(setImmediate);
  await editing.submitAddToArchiveJob();
  assert.equal(editing.submitted.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(editing.submitted[0])), {
    kind: "update", path: "/tmp/archive-editing.zip", encoding: "gbk", add: ["/inputs/资料.txt"],
    delete: [], rename: [], mkdir: [], excludes: ["*.bak"], content_policy: "custom", password: null, level: 8,
  });
  submitted(1); await pending;
  assert.equal(editing.context.archiveAddPending, false);
  assert.equal(operations.length, 1);
  assert.match(operations[0].detail, /custom/);
});

test("add cancellation, picker failure, and submission errors release the action and report their actual cause", async () => {
  for (const failure of ["cancel", "picker", "submit", "blocked", "error-dto"]) {
    const operations = [];
    const editing = await loadEditing({ recordOperation: (operation) => operations.push(operation) });
    if (failure === "cancel") editing.context.getDialogModule = async () => ({ open: async () => null });
    if (failure === "picker") editing.context.getDialogModule = async () => { throw new Error("unavailable"); };
    if (failure === "blocked") editing.context.isJobSubmitBlocked = () => true;
    if (["submit", "blocked", "error-dto"].includes(failure)) {
      editing.context.submitJob = async () => { throw failure === "error-dto" ? { key: "error.io", params: {} } : new Error("offline"); };
    }
    await editing.submitAddToArchiveJob();
    assert.equal(editing.context.archiveAddPending, false);
    assert.equal(operations.length, 0);
    if (failure === "cancel") assert.match(editing.notices.at(-1), /cancelled/);
    if (failure === "picker") assert.match(editing.notices.at(-1), /file (dialog|chooser)/);
    if (failure === "submit") assert.match(editing.toasts.at(-1).body, /desktop service/);
    if (failure === "error-dto") assert.equal(editing.toasts.at(-1).body, "error.io");
    if (failure === "blocked") assert.equal(editing.toasts.length, 0);
  }
});

test("deleting selected entries preserves literal paths and directory boundaries", async () => {
  const editing = await loadEditing({
    selectedPaths: () => new Set(["notes.txt", "资料[1]/", "资料[1]/a.txt", "资料[1]/sub/", "literal?.txt"]),
  });
  await editing.submitDeleteSelectedJob();
  assert.equal(editing.submitted.length, 1);
  const job = editing.submitted[0];
  assert.equal(job.kind, "update");
  assert.equal(job.path, "/tmp/archive-editing.zip");
  assert.equal(job.encoding, "gbk");
  assert.deepEqual(Array.from(job.delete), ["notes.txt", "资料[1]/", "literal?.txt"]);
  assert.deepEqual(Array.from(job.rename), []);
  assert.deepEqual(Array.from(job.add), []);
});

test("deletion does not queue an empty, read-only, or unfinished selection", async () => {
  for (const overrides of [
    { selectedPaths: () => new Set() },
    { archiveMutationDisabledReason: () => "Read only" },
    { blockSelectionScopedAction: () => true },
    { currentArchive: null },
  ]) {
    const editing = await loadEditing(overrides);
    await editing.submitDeleteSelectedJob();
    assert.equal(editing.submitted.length, 0);
  }
  const operations = [];
  const editing = await loadEditing({
    submitJob: async () => { throw new Error("unavailable"); },
    recordOperation: (operation) => operations.push(operation),
  });
  await editing.submitDeleteSelectedJob();
  assert.deepEqual(operations, []);
  assert.equal(editing.toasts.length, 1);
  assert.equal(editing.toasts[0].kind, "danger");
  assert.equal(editing.toasts[0].title, "Could not queue the task");
  assert.match(editing.toasts[0].body, /try again/u);
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
  const failed = await loadEditing({ submitJob: async () => { throw new Error("unavailable"); } });
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
