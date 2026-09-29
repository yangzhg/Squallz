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
    "openArchiveEditor", "submitAddToArchiveJob", "archiveEditSubmissionFailure", "jobSubmitBlockedMessage",
    "archiveEditSelectedPaths", "archiveEditorBlockedReason", "validateArchiveEditContext", "archiveEditCheckIsCurrent",
  ]);
  const declarations = source.statements.filter(
    (node) => (ts.isFunctionDeclaration(node) && names.has(node.name?.text))
      || (ts.isClassDeclaration(node) && node.name?.text === "JobSubmitBlockedError"),
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
    archiveEditContext: null,
    archiveEditChecking: false,
    ipc: { missingArchivePaths: async () => [] },
    archiveEditSession: 0,
    archiveEditError: null,
    archiveEditReturnFocus: null,
    closeArchiveEditor: () => { closed.push(true); context.archiveEditKind = null; context.archiveEditContext = null; context.archiveEditSession += 1; },
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
    $effect: (callback) => callback(), untrack: (callback) => callback(),
    ...overrides,
  };
  const handlers = vm.runInNewContext(`${outputText}\n({ normalizeNewFolderPath, commitNewFolderName, submitNewFolderJob, normalizeMoveTargetDir, submitMoveSelectedJob, submitMovePlan, buildMovePlan, normalizeRenameTargetName, submitRenameSelectedJob, submitDeleteSelectedJob, openArchiveEditor, submitAddToArchiveJob, JobSubmitBlockedError })`, context);
  const reset = source.statements.find((node) => ts.isExpressionStatement(node)
    && node.getText(source).startsWith("$effect(") && node.getText(source).includes('archiveDirs.join("\\u0000")')
    && node.getText(source).includes("renameTargetName"));
  assert.ok(reset);
  return { ...handlers, submitted, notices, toasts, closed, context,
    refreshEditor: () => vm.runInNewContext(ts.transpileModule(reset.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context),
  };
}

test("refresh keeps rename text and checks the original selection before queuing", async () => {
  const checks = [];
  const editing = await loadEditing({ ipc: { missingArchivePaths: async (id, paths) => { checks.push([id, [...paths]]); return []; } } });
  editing.openArchiveEditor("rename");
  editing.context.renameTargetName = "kept.txt";
  editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
  editing.context.selectedPaths = () => new Set(["unrelated.txt"]);
  editing.refreshEditor();
  assert.equal(editing.context.archiveEditKind, "rename");
  assert.equal(editing.context.renameTargetName, "kept.txt");
  await editing.submitRenameSelectedJob();
  assert.deepEqual(checks, [[18, ["docs/a.txt"]]]);
  assert.deepEqual(JSON.parse(JSON.stringify(editing.submitted[0].rename)), [{ from: "docs/a.txt", to: "docs/kept.txt" }]);
});

test("move and new-folder drafts keep their original entries and parent after refresh", async () => {
  const moving = await loadEditing({ selectedPaths: () => new Set(["docs/", "docs/a.txt", "other.txt"]) });
  moving.openArchiveEditor("move");
  moving.context.currentArchive = { ...moving.context.currentArchive, id: 18 };
  moving.context.selectedPaths = () => new Set();
  moving.refreshEditor();
  await moving.submitMoveSelectedJob();
  assert.deepEqual(JSON.parse(JSON.stringify(moving.submitted[0].rename)), [
    { from: "docs/", to: "destination/docs/" }, { from: "other.txt", to: "destination/other.txt" },
  ]);
  const folder = await loadEditing();
  folder.openArchiveEditor("new-folder");
  folder.context.newFolderName = "Plans";
  folder.context.currentArchive = { ...folder.context.currentArchive, id: 18 };
  folder.context.archiveDirs = [];
  folder.refreshEditor();
  await folder.submitNewFolderJob();
  assert.deepEqual([...folder.submitted[0].mkdir], ["docs/Plans/"]);
});

test("missing targets and lookup failures retain drafts and can be checked again", async () => {
  for (const failure of ["missing", "unavailable"]) {
    const editing = await loadEditing({ ipc: { missingArchivePaths: async () => {
      if (failure === "missing") return ["docs/a.txt"];
      throw new Error("unavailable");
    } } });
    editing.openArchiveEditor("rename");
    editing.context.renameTargetName = "kept.txt";
    editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.context.archiveEditKind, "rename");
    assert.equal(editing.context.renameTargetName, "kept.txt");
    assert.match(editing.context.archiveEditError, failure === "missing" ? /no longer exist/ : /Could not check/);
    editing.context.ipc.missingArchivePaths = async () => [];
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 1);
  }
});

test("an absent original parent never silently redirects a new folder to a surviving ancestor", async () => {
  const editing = await loadEditing({ ipc: { missingArchivePaths: async () => ["docs/"] } });
  editing.openArchiveEditor("new-folder");
  editing.context.newFolderName = "Plans";
  editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
  editing.context.archiveDirs = [];
  await editing.submitNewFolderJob();
  assert.equal(editing.submitted.length, 0);
  assert.equal(editing.context.newFolderName, "Plans");
  editing.context.newFolderName = "/Plans";
  await editing.submitNewFolderJob();
  assert.deepEqual([...editing.submitted[0].mkdir], ["Plans/"]);
});

test("drafts cannot follow another archive, encoding, refresh, or late target check", async () => {
  for (const change of ["source", "encoding", "reopen", "refresh", "during-check"]) {
    const editing = await loadEditing();
    editing.openArchiveEditor("rename");
    editing.context.renameTargetName = "kept.txt";
    if (change === "source") editing.context.currentArchive.source = "/other.zip";
    if (change === "encoding") editing.context.currentArchive.encoding_override = "shift_jis";
    if (change === "reopen") editing.context.archiveOpenGeneration += 1;
    if (change === "refresh") editing.context.archiveMutationDisabledReason = () => "Refreshing archive contents…";
    if (change === "during-check") {
      editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
      editing.context.ipc.missingArchivePaths = async () => { editing.context.currentArchive = { ...editing.context.currentArchive, id: 19 }; return []; };
    }
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 0, change);
    assert.equal(editing.context.renameTargetName, "kept.txt", change);
    assert.ok(editing.context.archiveEditError, change);
  }
});

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

test("archive edit submission failures are shown in the current form and retry clears them", async () => {
  for (const [kind, action] of [["rename", "submitRenameSelectedJob"], ["move", "submitMoveSelectedJob"], ["new-folder", "submitNewFolderJob"]]) {
    const editing = await loadEditing({ submitJob: async () => { throw new Error("unavailable"); } });
    editing.openArchiveEditor(kind);
    if (kind === "rename") editing.context.renameTargetName = "renamed.txt";
    if (kind === "new-folder") editing.context.newFolderName = "Plans";
    await editing[action]();
    assert.equal(editing.context.archiveEditKind, kind);
    assert.match(editing.context.archiveEditError, /Could not queue the task.*try again/u);
    assert.equal(editing.toasts.length, 0, "modal errors cannot depend on a notification hidden beneath the editor");
    editing.context.submitJob = async (spec) => { editing.submitted.push(spec); return 1; };
    await editing[action]();
    assert.equal(editing.context.archiveEditError, null);
    assert.equal(editing.closed.length, 1);
    assert.equal(editing.submitted.length, 1);
  }
});

test("an obsolete edit submission cannot put its failure in a newly opened form", async () => {
  const editing = await loadEditing();
  let fail;
  editing.context.submitJob = () => new Promise((_resolve, reject) => { fail = reject; });
  editing.openArchiveEditor("rename");
  editing.context.renameTargetName = "renamed.txt";
  const pending = editing.submitRenameSelectedJob();
  await new Promise(setImmediate);
  editing.context.closeArchiveEditor();
  editing.openArchiveEditor("rename");
  fail(new Error("late failure"));
  await pending;
  assert.equal(editing.context.archiveEditKind, "rename");
  assert.equal(editing.context.archiveEditError, null);
  assert.equal(editing.toasts.length, 0);
});

test("archive editor feedback retains backend errors and explains blocked submissions", async () => {
  for (const failure of ["backend", "blocked-before", "blocked-during"]) {
    const editing = await loadEditing();
    editing.openArchiveEditor("new-folder");
    editing.context.newFolderName = "Plans";
    if (failure === "backend") editing.context.submitJob = async () => { throw { key: "error.io", params: {}, detail: "Write unavailable" }; };
    if (failure === "blocked-before") editing.context.focusBlockingTaskIfAny = () => "starting";
    if (failure === "blocked-during") {
      editing.context.isJobSubmitBlocked = (error) => error instanceof editing.JobSubmitBlockedError;
      editing.context.submitJob = async () => { throw new editing.JobSubmitBlockedError("starting"); };
    }
    await editing.submitNewFolderJob();
    assert.match(editing.context.archiveEditError, failure === "backend" ? /error.io/u : /previous task.*queue/u);
    assert.equal(editing.context.newFolderName, "Plans");
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.closed.length, 0);
    assert.equal(editing.toasts.length, 0);
  }
});

test("the shared archive editor renders one named modal form", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Editor } = await server.ssrLoadModule("/src/components/ArchiveEntryEditor.svelte");
    const props = {
      title: "Rename selected", label: "Rename target name", value: "资料", status: "reports/ -> 资料/",
      cancelLabel: "Cancel", submittingLabel: "Adding to the queue…", rootClass: "archive-editor-overlay design-root", rootVariables: {},
      retryLabel: "Retry", onRetry: async () => {},
      onChange: () => {}, onSubmit: async () => {}, onClose: () => {},
    };
    const { body } = render(Editor, { props });
    assert.match(body, /role="dialog" aria-modal="true"/u);
    assert.equal((body.match(/<form\b/gu) ?? []).length, 1);
    assert.equal((body.match(/<input\b/gu) ?? []).length, 1);
    assert.match(body, /role="status"/u);
    assert.match(body, /type="submit"/u);
    assert.doesNotMatch(body, /style=/u);
    const failed = render(Editor, { props: { ...props, error: "Could not queue the task. Try again." } }).body;
    assert.match(failed, /role="alert">Could not queue the task\. Try again\./u);
    assert.doesNotMatch(failed, /reports\/ -&gt; 资料\//u, "failure replaces the ready status");
    const refreshing = render(Editor, { props: { ...props, disabled: true, recovery: { message: "Refreshing archive contents…", busy: true } } }).body;
    assert.match(refreshing, /Refreshing archive contents/u);
    assert.match(refreshing, /type="submit" disabled/u);
    assert.doesNotMatch(refreshing, /<input[^>]*disabled/u, "drafts remain editable while the archive refreshes");
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
