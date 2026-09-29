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
    "normalizeNewFolderPath", "commitNewFolderName", "submitNewFolderJob", "validateArchiveEditTarget",
    "normalizeMoveTargetDir", "submitMovePlan", "moveTargetForPath",
    "archiveEditPathProblem", "moveTargetProblem", "submitMoveSelectedJob", "submitMoveKeepBoth",
    "normalizeRenameTargetName", "selectedRenameSource", "renameTargetIssue", "archiveEntryExtension",
    "submitRenameSelectedJob", "selectedDeletePaths", "submitDeleteSelectedJob", "submitCurrentArchiveJob",
    "openArchiveEditor", "submitAddToArchiveJob", "archiveEditSubmissionFailure", "showArchiveEditError", "jobSubmitBlockedMessage",
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
    archiveEditSession: 0,
    archiveEditError: null,
    archiveEditReturnFocus: null,
    closeArchiveEditor: () => { closed.push(true); context.archiveEditKind = null; context.archiveEditContext = null; context.archiveEditChecking = false; context.archiveEditSession += 1; },
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
  context.ipc = {
    missingArchivePaths: async () => [],
    inspectArchiveTarget: async () => ({ exists: false, blocked_parent: null }),
    planArchiveMove: async (_id, paths, target) => ({ missing_sources: [], blocked_parent: null,
      items: paths.map((from) => ({ from, to: target + from.replace(/\/$/, "").split("/").at(-1) + (from.endsWith("/") ? "/" : ""),
        conflict: null, keep_both_to: null })),
    }),
    ...overrides.ipc,
  };
  const handlers = vm.runInNewContext(`${outputText}\n({ normalizeNewFolderPath, commitNewFolderName, submitNewFolderJob, normalizeMoveTargetDir, submitMoveSelectedJob, submitMoveKeepBoth, submitMovePlan, normalizeRenameTargetName, submitRenameSelectedJob, submitDeleteSelectedJob, openArchiveEditor, submitAddToArchiveJob, JobSubmitBlockedError })`, context);
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

test("rename and new-folder submission check unloaded target names before queuing", async () => {
  for (const [kind, action, value, target] of [
    ["rename", "submitRenameSelectedJob", "existing.txt", "docs/existing.txt"],
    ["new-folder", "submitNewFolderJob", "/unloaded/reports", "unloaded/reports/"],
  ]) {
    const checks = [];
    const editing = await loadEditing({ ipc: { inspectArchiveTarget: async (id, path) => {
      checks.push([id, path]);
      return { exists: true, blocked_parent: null };
    } } });
    editing.openArchiveEditor(kind);
    const field = kind === "rename" ? "renameTargetName" : "newFolderName";
    editing.context[field] = value;
    await editing[action]();
    assert.equal(editing.submitted.length, 0, kind);
    assert.deepEqual(checks, [[17, target]]);
    assert.equal(editing.context[field], value);
    assert.equal(editing.context.archiveEditKind, kind);
    assert.match(editing.context.archiveEditError, /already exists.*Choose/);
  }
});

test("destination failures and parent files retain both edit forms for a corrected retry", async () => {
  for (const [kind, action] of [["rename", "submitRenameSelectedJob"], ["new-folder", "submitNewFolderJob"]]) {
    for (const failure of ["unavailable", "parent"]) {
      const editing = await loadEditing({ ipc: { inspectArchiveTarget: async () => {
        if (failure === "unavailable") throw new Error("unavailable");
        return { exists: false, blocked_parent: "blocked" };
      } } });
      editing.openArchiveEditor(kind);
      const field = kind === "rename" ? "renameTargetName" : "newFolderName";
      editing.context[field] = "/blocked/target";
      await editing[action]();
      assert.equal(editing.submitted.length, 0);
      assert.equal(editing.context[field], "/blocked/target");
      assert.equal(editing.context.archiveEditKind, kind);
      assert.equal(editing.context.archiveEditChecking, false);
      assert.match(editing.context.archiveEditError, failure === "unavailable" ? /Could not check.*Try again/ : /blocked.*parent folder.*different path/);
      editing.context[field] = "/available/target";
      editing.context.ipc.inspectArchiveTarget = async () => ({ exists: false, blocked_parent: null });
      await editing[action]();
      assert.equal(editing.submitted.length, 1);
      assert.equal(editing.context.archiveEditError, null);
      assert.equal(editing.context.archiveEditKind, null);
    }
  }
});

test("late destination checks cannot queue edits after cancellation, refresh or changed text", async () => {
  for (const [kind, action] of [["rename", "submitRenameSelectedJob"], ["new-folder", "submitNewFolderJob"]]) {
    for (const change of ["cancel", "refresh", "new-editor", "text"]) {
      for (const failure of [false, true]) {
        let resolve, reject;
        let calls = 0;
        const editing = await loadEditing({ ipc: { inspectArchiveTarget: () => {
          calls++;
          return new Promise((yes, no) => { resolve = yes; reject = no; });
        } } });
        editing.openArchiveEditor(kind);
        const field = kind === "rename" ? "renameTargetName" : "newFolderName";
        editing.context[field] = "new-name";
        const pending = editing[action]();
        await new Promise(setImmediate);
        assert.equal(editing.context.archiveEditChecking, true);
        await editing[action]();
        assert.equal(calls, 1);
        if (change === "cancel") editing.context.closeArchiveEditor();
        if (change === "refresh") editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
        if (change === "new-editor") { editing.openArchiveEditor("move"); editing.context.archiveEditError = "new error"; }
        if (change === "text") editing.context[field] = "another-name";
        if (failure) reject(new Error("unavailable"));
        else resolve({ exists: false, blocked_parent: null });
        await pending;
        assert.equal(editing.submitted.length, 0, `${kind}: ${change}, failure=${failure}`);
        assert.equal(editing.context.archiveEditChecking, false);
        if (change === "new-editor") assert.equal(editing.context.archiveEditError, "new error");
        if (change === "cancel" || change === "text") assert.equal(editing.context.archiveEditError, null);
      }
    }
  }
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

test("move confirmation uses the complete archive plan even when target rows are unloaded", async () => {
  const checks = [];
  const editing = await loadEditing({
    loadedRows: () => [{ path: "docs/a.txt", entry_type: "file" }],
    ipc: {
      missingArchivePaths: async () => [],
      planArchiveMove: async (id, paths, targetDir) => {
        checks.push([id, [...paths], targetDir]);
        return { missing_sources: [], blocked_parent: null, items: [{
          from: "docs/a.txt", to: "destination/a.txt", conflict: "existing_target",
          keep_both_to: "destination/a copy 3.txt",
        }] };
      },
    },
  });
  editing.openArchiveEditor("move");
  await editing.submitMoveSelectedJob();
  assert.equal(editing.submitted.length, 0, "unloaded conflicts must be reviewed before queuing");
  assert.deepEqual(checks, [[17, ["docs/a.txt"], "destination/"]]);
  assert.equal(editing.context.moveConflictReview.items[0].keepBothTo, "destination/a copy 3.txt");
  await editing.submitMoveKeepBoth();
  assert.deepEqual(JSON.parse(JSON.stringify(editing.submitted[0].rename)), [{
    from: "docs/a.txt", to: "destination/a copy 3.txt",
  }]);
});

test("an archive-root move keeps the root destination", async () => {
  const editing = await loadEditing({ moveTargetDir: "/" });
  assert.equal(editing.normalizeMoveTargetDir(), "");
});

test("moving a selected directory does not separately flatten its selected children", async () => {
  const editing = await loadEditing({
    selectedPaths: () => new Set(["docs/", "docs/a.txt", "docs/sub/", "docs/sub/b.txt", "other.txt"]),
  });
  await editing.submitMoveSelectedJob();
  assert.deepEqual(Array.from(editing.submitted[0].rename, ({ from, to }) => [from, to]), [
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
  for (const [kind, action, field] of [["new-folder", "submitNewFolderJob", "newFolderName"], ["move", "submitMoveSelectedJob", "moveTargetDir"]]) {
    for (const value of ["../escape", "safe/../escape", "  safe\\..\\escape  ", "NUL", "folder/file?.txt", "folder. /child"]) {
      const editing = await loadEditing();
      editing.openArchiveEditor(kind);
      editing.context[field] = value;
      await editing[action]();
      assert.equal(editing.submitted.length, 0, value);
      assert.equal(editing.notices.length, 0, "input errors belong in the active editor");
      assert.match(editing.context.archiveEditError, /Enter|Choose|Remove/);
      assert.equal(editing.context[field], value, "invalid input stays intact for correction");
      assert.equal(editing.context.archiveEditKind, kind);
      editing.context[field] = "corrected";
      await editing[action]();
      assert.equal(editing.submitted.length, 1);
      assert.equal(editing.context.archiveEditError, null);
    }
  }
});

test("move check failures, missing sources and blocked parents keep the destination for retry", async () => {
  for (const result of ["failure", "missing", "parent"]) {
    const editing = await loadEditing({ ipc: { planArchiveMove: async () => {
      if (result === "failure") throw new Error("unavailable");
      return { items: [], missing_sources: result === "missing" ? ["docs/a.txt"] : [],
        blocked_parent: result === "parent" ? "destination" : null };
    } } });
    editing.openArchiveEditor("move");
    await editing.submitMoveSelectedJob();
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.context.archiveEditKind, "move");
    assert.equal(editing.context.moveTargetDir, "destination/");
    assert.equal(editing.context.archiveEditChecking, false);
    assert.match(editing.context.archiveEditError, result === "failure" ? /Try again/ : result === "missing" ? /no longer exist/ : /Choose a different path/);
    editing.context.ipc.planArchiveMove = async () => ({ missing_sources: [], blocked_parent: null,
      items: [{ from: "docs/a.txt", to: "destination/a.txt", conflict: null, keep_both_to: null }] });
    await editing.submitMoveSelectedJob();
    assert.equal(editing.submitted.length, 1);
  }
});

test("late move checks cannot queue or replace a changed archive or newer editor", async () => {
  for (const change of ["close", "new-editor", "refresh", "open", "destination"]) {
    for (const fail of [false, true]) {
      let finish, reject;
      const editing = await loadEditing({ ipc: { planArchiveMove: () => new Promise((resolve, fail) => { finish = resolve; reject = fail; }) } });
      editing.openArchiveEditor("move");
      const pending = editing.submitMoveSelectedJob();
      await new Promise(setImmediate);
      assert.equal(editing.context.archiveEditChecking, true);
      await editing.submitMoveSelectedJob();
      if (change === "close") editing.context.closeArchiveEditor();
      if (change === "new-editor") { editing.openArchiveEditor("rename"); editing.context.archiveEditError = "new error"; }
      if (change === "refresh") editing.context.currentArchive = { ...editing.context.currentArchive, id: 18 };
      if (change === "open") editing.context.archiveOpenGeneration++;
      if (change === "destination") editing.context.moveTargetDir = "different/";
      if (fail) reject(new Error("unavailable"));
      else finish({ missing_sources: [], blocked_parent: null, items: [{ from: "docs/a.txt", to: "destination/a.txt", conflict: null, keep_both_to: null }] });
      await pending;
      assert.equal(editing.submitted.length, 0, `${change}, failure=${fail}`);
      assert.equal(editing.context.moveConflictReview, null);
      assert.equal(editing.context.archiveEditChecking, false);
      if (change === "new-editor") assert.equal(editing.context.archiveEditError, "new error");
      if (change === "close" || change === "destination") assert.equal(editing.context.archiveEditError, null);
    }
  }
});

test("a confirmed move plan cannot follow an archive refresh or reopen", async () => {
  for (const change of ["id", "generation", "opening"]) {
    const editing = await loadEditing();
    editing.context.moveConflictReview = { archiveId: 17, generation: 0, targetDir: "destination/",
      items: [{ from: "docs/a.txt", to: "destination/a.txt", conflict: true, keepBothTo: "destination/a copy.txt" }] };
    if (change === "id") editing.context.currentArchive.id = 18;
    if (change === "generation") editing.context.archiveOpenGeneration++;
    if (change === "opening") editing.context.archiveOpenStatus = "opening";
    await editing.submitMoveKeepBoth();
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.context.moveConflictReview, null);
    assert.match(editing.notices[0], /archive changed/);
  }
});

test("moves into the selected subtree or the same folder do not become keep-both operations", async () => {
  for (const [source, destination] of [["docs/", "docs/sub/"], ["docs/a.txt", "docs/"]]) {
    const editing = await loadEditing({ selectedPaths: () => new Set([source]), moveTargetDir: destination });
    editing.openArchiveEditor("move");
    await editing.submitMoveSelectedJob();
    assert.equal(editing.submitted.length, 0);
    assert.equal(editing.context.moveConflictReview, null);
    assert.equal(editing.notices.length, 0);
    assert.match(editing.context.archiveEditError, /Choose/);
    assert.equal(editing.context.moveTargetDir, destination);
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
    const editing = await loadEditing({ selectedPaths: () => new Set(["docs/reports/"]) });
    editing.openArchiveEditor("rename");
    editing.context.renameTargetName = value;
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 0, value);
    assert.equal(editing.notices.length, 0, value);
    assert.match(editing.context.archiveEditError, /Enter|Choose/);
    assert.equal(editing.context.renameTargetName, value);
    assert.equal(editing.context.archiveEditKind, "rename");
    editing.context.renameTargetName = "updated";
    await editing.submitRenameSelectedJob();
    assert.equal(editing.submitted.length, 1);
    assert.equal(editing.context.archiveEditError, null);
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
