import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

function row(path, size = 1) {
  return {
    path, display: path.split("/").at(-1), entry_type: "file", size,
    compressed: null, modified: null, crc: null, encrypted: false, encoding: "utf-8",
  };
}

function info(id = 1) {
  return {
    id, source: "/tmp/refresh.zip", path: "/tmp/refresh.zip", name: "refresh.zip",
    format: "zip", entry_count: 1, volumes: null, non_utf8_name_count: 0,
    garbled_count: 0, suggested_encoding: null, encoding_override: "gbk", read_only: false,
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function until(predicate) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail("Expected archive request was not made");
}

async function withArchive(run, options = {}) {
  const server = await createTestServer();
  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const toasts = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    const closed = [];
    const requests = [];
    const pendingOpen = deferred();
    ipc.openArchive = () => pendingOpen.promise;
    ipc.closeArchive = async (id) => { closed.push(id); };
    ipc.cancelArchiveOpen = async () => {};
    ipc.cancelArchiveSearch = async () => {};
    ipc.archivePasswordStatus = async () => ({ session: false, available: true, saved: false, error: null });
    ipc.resolveArchiveDirectory = async (_id, prefix) => prefix;
    ipc.listEntries = async (id, page, prefix) => {
      requests.push({ id, page, prefix });
      return { items: [row(`${prefix}new.txt`)], total: 1, page };
    };
    archive.installArchivePreview(info(), [row("docs/old.txt")], {
      dirs: ["docs"], selected: ["docs/old.txt"], ...options,
    });
    try {
      await run({ archive, ipc, toasts, closed, requests, pendingOpen });
    } finally {
      archive.closeArchive();
    }
  } finally {
    await server.close();
  }
}

async function updateRefreshEffect(archive, jobs) {
  const app = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/u)[1], ts.ScriptTarget.Latest, true);
  const effect = source.statements.find((node) => ts.isExpressionStatement(node)
    && node.getText(source).startsWith("$effect(") && node.getText(source).includes("refreshedUpdateJobs"));
  assert.ok(effect);
  const helpers = source.statements.filter((node) => ts.isFunctionDeclaration(node) && node.name?.text === "pendingArchiveUpdateJobs");
  const { outputText } = ts.transpileModule([...helpers, effect].map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  const pending = [];
  const notices = [];
  const context = {
    jobRows: jobs, archiveOpenStatus: "idle", refreshedUpdateJobs: new Set(),
    get currentArchive() { return archive.archive(); },
    get archivePasswordPrompt() { return archive.openPasswordPrompt(); },
    archiveRefreshStatus: archive.archiveRefreshStatus,
    refreshCurrentArchive: (window) => {
      const result = archive.refreshCurrentArchive(window);
      pending.push(result);
      return result;
    },
    mode: "modern", CLASSIC_ROW_HEIGHT: 32, MODERN_ROW_HEIGHT: 40,
    browseVirtualWindow: () => ({ start: 0, end: 20 }),
    showNotice: (notice) => notices.push(notice), tr: (_key, fallback) => fallback,
    $effect: (callback) => callback(), untrack: (callback) => callback(),
  };
  vm.createContext(context);
  return { context, pending, notices, run: () => vm.runInContext(outputText, context) };
}

function completedUpdate(id, path = "/tmp/refresh.zip") {
  return { id, state: "done", spec: { kind: "update", path } };
}

async function passwordRefreshActions(archive) {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/u)[1], ts.ScriptTarget.Latest, true);
  const names = ["cancelTaskReview", "submitPasswordRequest", "cancelPasswordRequest", "dismissArchivePasswordRequest", "dismissArchivePicker", "dismissRecoveryPreparation", "setScreen", "openArchivePath", "archiveEditorVisible", "blockingModalVisible", "archiveEditorBlockedReason"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  const context = {
    get currentArchive() { return archive.archive(); },
    get archivePasswordPrompt() { return archive.openPasswordPrompt(); },
    previewPasswordPrompt: null, screen: "password", workspacePasswordValue: "correct",
    workspacePasswordSubmissionAttempted: false, standalonePasswordFocusedInput: null,
    archiveOpenStatus: "idle", archiveOpenGeneration: 7, archivePasswordAttempt: 0,
    archivePickerRequest: null,
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0, recoveryOutputPreparation: null,
    dismissCreatePreparation() {},
    archiveUpdateReview: { cancelSourceChoice() {} },
    batchPickerRequest: 0, nestedExtractPickerRequest: 0,
    archiveEditKind: "rename", renameTargetName: "kept.txt",
    archiveEditContext: { source: info().source, encoding: "gbk", generation: 7, id: 1 },
    taskDialogVisible: () => false, macosSfxPublisherTask: null,
    taskPasswordReady: (value) => value.length > 0,
    archiveMutationDisabledReason: () => archive.archiveRefreshStatus() === "idle" ? "" : "Refresh required",
    openArchiveStore: archive.openArchive, cancelArchivePasswordPrompt: archive.cancelPasswordPrompt,
    openPasswordPrompt: archive.openPasswordPrompt, archiveOpenError: archive.archiveOpenError,
    finishOpenedArchive: () => assert.fail("Refresh must not complete a new archive-open workflow"),
    showNotice: () => {}, tr: (_key, fallback) => fallback,
    preventCreateSubmissionNavigation: () => false, preventConvertSubmissionNavigation: () => false,
    clearEntryPreviewState: () => {}, isPar2Path: () => false,
    taskReviewRequestGeneration: 0, nestedExtractDraftGeneration: 0,
    syncUrl: () => {}, tick: async () => {},
    document: { documentElement: {}, body: {}, querySelectorAll: () => [] },
  };
  return { context, ...vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")}})`, context) };
}

test("leaving a refresh password cancels its read and keeps the editor dormant until returning", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen, closed }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    const run = await passwordRefreshActions(archive);
    const late = deferred();
    const cancelled = [];
    ipc.cancelArchiveOpen = async (id) => { cancelled.push(id); };
    ipc.openArchive = () => late.promise;
    const unlocking = run.submitPasswordRequest();
    run.setScreen("settingsGeneral");
    assert.equal(archive.openPasswordPrompt(), null);
    assert.equal(cancelled.length, 1);
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(run.context.workspacePasswordValue, "");
    assert.equal(run.context.archiveOpenGeneration, 7, "navigation retains the original edit session");
    assert.equal(run.archiveEditorVisible(), false, "the retained draft must not cover the chosen page");
    late.resolve(info(2));
    await unlocking;
    assert.equal(run.context.screen, "settingsGeneral");
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(closed, [2]);
    run.setScreen("browse");
    assert.equal(run.archiveEditorVisible(), true);
    assert.equal(run.context.renameTargetName, "kept.txt");
    assert.notEqual(run.archiveEditorBlockedReason(), "");
  });
});

test("an explicit same-source open abandons password refresh intent and clears its input", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    const run = await passwordRefreshActions(archive);
    const next = deferred();
    ipc.openArchive = () => next.promise;
    run.context.finishOpenedArchive = () => { run.context.screen = "browse"; };
    const opening = run.openArchivePath(info().source, "open-file");
    assert.equal(archive.openPasswordPrompt(), null, "a replacement open cannot reuse the old password prompt");
    assert.equal(run.context.workspacePasswordValue, "");
    next.resolve(info(2));
    await opening;
    assert.deepEqual(archive.currentDirs(), [], "an explicit open uses its own initial view");
    assert.notEqual(run.archiveEditorBlockedReason(), "", "old edit targets cannot follow a new open session");
  });
});

test("refresh password attempts suspend the editor and resume its original session after unlocking", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    const run = await passwordRefreshActions(archive);
    assert.equal(archive.openPasswordPrompt().refresh, true);
    assert.equal(run.blockingModalVisible(), false, "the editor must not make the password form inert");
    ipc.openArchive = async () => { throw { key: "error.wrong_password", params: {}, detail: "" }; };
    await run.submitPasswordRequest();
    assert.equal(run.context.archiveOpenGeneration, 7);
    assert.equal(run.context.workspacePasswordValue, "");
    assert.equal(run.context.screen, "password");
    assert.equal(run.archiveEditorVisible(), false);
    ipc.openArchive = async () => info(2);
    run.context.workspacePasswordValue = "correct";
    await run.submitPasswordRequest();
    assert.equal(run.context.archiveOpenGeneration, 7);
    assert.equal(run.context.screen, "browse");
    assert.equal(run.archiveEditorVisible(), true);
    assert.equal(run.archiveEditorBlockedReason(), "");
    assert.equal(run.context.renameTargetName, "kept.txt");
    assert.deepEqual(archive.currentDirs(), ["docs"]);
  });
});

test("cancelling an in-flight refresh password keeps retry and rejects the late attempt", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen, closed }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    const run = await passwordRefreshActions(archive);
    const late = deferred();
    ipc.openArchive = () => late.promise;
    const unlocking = run.submitPasswordRequest();
    await run.cancelPasswordRequest();
    assert.equal(run.context.archiveOpenGeneration, 7);
    assert.equal(run.archiveEditorVisible(), true);
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.notEqual(run.archiveEditorBlockedReason(), "", "old rows cannot become editable after cancellation");
    ipc.openArchive = async (_path, password, encoding) => {
      assert.equal(password, null, "retry must not retain the previous password input");
      assert.equal(encoding, "gbk");
      throw { key: "error.password_required", params: {}, detail: "" };
    };
    await archive.retryArchiveBrowse();
    run.context.screen = "password";
    run.context.workspacePasswordValue = "new attempt";
    late.resolve(info(2));
    await unlocking;
    assert.equal(run.context.workspacePasswordValue, "new attempt");
    assert.equal(run.context.screen, "password");
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(closed, [2]);
    assert.equal(archive.openPasswordPrompt().refresh, true);
    ipc.openArchive = async () => info(3);
    await run.submitPasswordRequest();
    assert.equal(run.archiveEditorBlockedReason(), "");
    assert.equal(archive.archive().id, 3);
  });
});

test("a listing failure after a refresh password returns to the retryable editor", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    const run = await passwordRefreshActions(archive);
    ipc.openArchive = async () => info(2);
    ipc.listEntries = async () => { throw { key: "error.io", params: {}, detail: "" }; };
    await run.submitPasswordRequest();
    assert.equal(run.context.screen, "browse");
    assert.equal(run.archiveEditorVisible(), true);
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(archive.archive().id, 1);
    assert.equal(run.context.workspacePasswordValue, "");
  });
});

test("cancelled refresh retries cannot replace a later archive open", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    await refreshing;
    archive.cancelPasswordPrompt();
    let calls = 0;
    ipc.openArchive = async () => { calls += 1; return { ...info(2), source: "/other.zip", path: "/other.zip" }; };
    await archive.openArchive("/other.zip");
    await archive.retryArchiveBrowse();
    assert.equal(calls, 1);
    assert.equal(archive.archive().source, "/other.zip");
    assert.equal(archive.archiveRefreshStatus(), "idle");
  });
});

test("completed updates from one snapshot reopen the current archive only once", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const run = await updateRefreshEffect(archive, [
      completedUpdate(1), completedUpdate(2), completedUpdate(3),
      completedUpdate(4, "/tmp/other.zip"),
      { ...completedUpdate(5), state: "running" },
      { id: 6, state: "done", spec: { kind: "test", path: "/tmp/refresh.zip" } },
    ]);
    const cancelled = [];
    ipc.cancelArchiveOpen = async (id) => { cancelled.push(id); };
    run.run();
    assert.equal(run.pending.length, 1);
    assert.deepEqual([...run.context.refreshedUpdateJobs], [1, 2, 3]);
    assert.equal(cancelled.length, 0);
    pendingOpen.resolve(info(2));
    await run.pending[0];
    run.run();
    assert.equal(run.pending.length, 1, "a snapshot replay cannot refresh the same updates again");
    assert.equal(run.notices.length, 1);
  });
});

test("updates completed during a refresh wait and share one follow-up read", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const run = await updateRefreshEffect(archive, [completedUpdate(1)]);
    const cancelled = [];
    ipc.cancelArchiveOpen = async (id) => { cancelled.push(id); };
    run.run();
    run.context.jobRows.push(completedUpdate(2), completedUpdate(3));
    run.run();
    assert.equal(run.pending.length, 1, "later updates must not interrupt the first read");
    assert.equal(cancelled.length, 0);
    pendingOpen.resolve(info(2));
    await run.pending[0];
    assert.equal(run.notices.length, 0, "completion feedback waits for the latest updates to be read");
    ipc.openArchive = async () => info(3);
    run.run();
    await run.pending[1];
    assert.equal(run.pending.length, 2);
    assert.equal(archive.archive().id, 3);
    assert.deepEqual([...run.context.refreshedUpdateJobs], [1, 2, 3]);
    assert.equal(run.notices.length, 1);
  });
});

test("completed updates wait for an encoding change and refresh using its accepted encoding", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const run = await updateRefreshEffect(archive, [completedUpdate(1)]);
    const changing = archive.reopenWithEncoding("shift_jis");
    run.run();
    assert.equal(run.pending.length, 0);
    assert.equal(run.context.refreshedUpdateJobs.size, 0);
    pendingOpen.resolve({ ...info(2), encoding_override: "shift_jis" });
    await changing;
    ipc.openArchive = async (_path, _password, encoding) => {
      assert.equal(encoding, "shift_jis");
      return { ...info(3), encoding_override: encoding };
    };
    run.run();
    await run.pending[0];
    assert.equal(archive.archiveEncoding(), "shift_jis");
  });
});

test("failed automatic refreshes stay retryable without looping or refreshing another archive", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const run = await updateRefreshEffect(archive, [completedUpdate(1), completedUpdate(2)]);
    run.context.archiveOpenStatus = "opening";
    run.run();
    assert.equal(run.pending.length, 0);
    run.context.archiveOpenStatus = "idle";
    run.run();
    pendingOpen.reject({ key: "error.io", params: {}, detail: "read failed" });
    await run.pending[0];
    run.run();
    assert.equal(run.pending.length, 1);
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(run.notices.length, 0);
    ipc.openArchive = async () => info(2);
    await archive.retryArchiveBrowse();
    assert.equal(archive.archiveRefreshStatus(), "idle");
    run.context.jobRows.push(completedUpdate(3));
    archive.installArchivePreview({ ...info(3), path: "/tmp/other.zip", source: "/tmp/other.zip" }, [row("other.txt")]);
    run.run();
    assert.equal(run.pending.length, 1);
    assert.equal(run.context.refreshedUpdateJobs.has(3), false);
  });
  await withArchive(async ({ archive, ipc, requests }) => {
    const cached = Array.from({ length: 500 }, (_, index) => row(`entry-${index}.txt`));
    archive.installArchivePreview(info(), cached, { total: 1001 });
    const failedPage = deferred();
    const inFlightPage = deferred();
    const retriedPage = deferred();
    let attempts = 0;
    ipc.listEntries = (id, page, prefix) => {
      requests.push({ id, page, prefix });
      if (page === 0) return Promise.resolve({ items: cached, total: 1001, page });
      if (page === 2) return inFlightPage.promise;
      attempts += 1;
      return attempts === 1 ? failedPage.promise : retriedPage.promise;
    };
    assert.equal(archive.rowAt(500), null);
    assert.equal(archive.rowAt(1000), null);
    await until(() => requests.length === 2);
    const failure = { key: "error.io", params: {}, detail: "page read failed" };
    failedPage.reject(failure);
    await until(() => archive.archiveBrowseError() !== null);
    await new Promise((resolve) => setImmediate(resolve));
    for (let render = 0; render < 3; render += 1) {
      assert.equal(archive.rowAt(500), null);
      archive.prefetchAround(500, 0);
      await new Promise((resolve) => setImmediate(resolve));
    }
    assert.equal(requests.length, 2, "rendering failed page slots must not restart page reads");
    assert.equal(archive.rowAt(0)?.path, "entry-0.txt", "cached rows remain available after another page fails");
    inFlightPage.resolve({ items: [row("entry-1000.txt")], total: 1001, page: 2 });
    await until(() => archive.rowAt(1000) !== null);
    assert.equal(archive.rowAt(1000)?.path, "entry-1000.txt", "already-running page reads still populate the current cache");
    assert.deepEqual(archive.archiveBrowseError(), failure);

    archive.selectRow(cached[0], 0);
    const retry = archive.retryArchiveBrowse();
    assert.equal(archive.totalRows(), 1001, "retrying a missing page must keep the virtual list height");
    assert.equal(archive.rowAt(0)?.path, "entry-0.txt");
    assert.equal(archive.rowAt(1000)?.path, "entry-1000.txt");
    assert.deepEqual([...archive.selectedPaths()], ["entry-0.txt"]);
    await until(() => requests.length === 3);
    assert.equal(archive.archiveBrowseError(), null);
    retriedPage.resolve({ items: Array.from({ length: 500 }, (_, index) => row(`entry-${500 + index}.txt`)),
      total: 1001, page: 1 });
    await retry;
    assert.equal(archive.rowAt(500)?.path, "entry-500.txt");
    assert.deepEqual(requests.map(({ page }) => page), [1, 2, 1]);
  });
});

test("refresh prepares the displayed directory before replacing its rows and archive handle", async () => {
  await withArchive(async ({ archive, ipc, closed, requests, pendingOpen }) => {
    const pendingPage = deferred();
    ipc.listEntries = (id, page, prefix) => {
      requests.push({ id, page, prefix });
      return pendingPage.promise;
    };
    const refreshing = archive.refreshCurrentArchive();
    assert.equal(archive.archiveRefreshStatus(), "refreshing");
    pendingOpen.resolve(info(2));
    await until(() => requests.length > 0);
    assert.deepEqual(requests, [{ id: 2, page: 0, prefix: "docs/" }]);
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(archive.loadedRows().map(({ path }) => path), ["docs/old.txt"]);
    assert.deepEqual([...archive.selectedPaths()], ["docs/old.txt"]);
    assert.deepEqual(closed, []);
    pendingPage.resolve({ items: [row("docs/new.txt")], total: 1, page: 0 });
    assert.equal(await refreshing, true);
    assert.equal(archive.archiveRefreshStatus(), "idle");
    assert.equal(archive.archive().id, 2);
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.deepEqual(archive.loadedRows().map(({ path }) => path), ["docs/new.txt"]);
    assert.deepEqual([...archive.selectedPaths()], []);
    assert.deepEqual(closed, [1]);
  });
});

test("navigation while reopening keeps the user's latest directory", async () => {
  await withArchive(async ({ archive, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    await archive.enterDirPath("pictures/");
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.deepEqual(archive.currentDirs(), ["pictures"]);
    assert.deepEqual(archive.loadedRows().map(({ path }) => path), ["pictures/new.txt"]);
  });
});

test("navigation during a refresh page request discards that page and reads the latest directory", async () => {
  await withArchive(async ({ archive, ipc, requests, pendingOpen }) => {
    const pendingPage = deferred();
    ipc.listEntries = async (id, page, prefix) => {
      requests.push({ id, page, prefix });
      if (id === 2 && prefix === "docs/") return pendingPage.promise;
      return { items: [row(`${prefix}new.txt`)], total: 1, page };
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    await until(() => requests.some(({ id, prefix }) => id === 2 && prefix === "docs/"));
    await archive.enterDirPath("pictures/");
    pendingPage.resolve({ items: [row("docs/new.txt")], total: 1, page: 0 });
    assert.equal(await refreshing, true);
    assert.deepEqual(archive.currentDirs(), ["pictures"]);
    assert.deepEqual(archive.loadedRows().map(({ path }) => path), ["pictures/new.txt"]);
    assert.ok(requests.some(({ id, prefix }) => id === 2 && prefix === "pictures/"));
  });
});

test("a failed refresh keeps the previous view and offers a full reopen retry", async () => {
  await withArchive(async ({ archive, ipc, toasts, closed, pendingOpen }) => {
    ipc.listEntries = async (_id, page, prefix) => {
      if (prefix === "docs/") throw { key: "error.io", params: {}, detail: "read failed" };
      return { items: [row("root.txt")], total: 1, page };
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, false);
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(archive.archive().id, 1);
    assert.deepEqual(archive.loadedRows().map(({ path }) => path), ["docs/old.txt"]);
    assert.deepEqual([...archive.selectedPaths()], ["docs/old.txt"]);
    assert.deepEqual(closed, [2]);
    const retry = toasts.toasts().find((toast) => toast.key === "archive-refresh-error");
    assert.ok(retry?.action);
    ipc.openArchive = async () => info(3);
    ipc.listEntries = async (_id, page, prefix) => ({ items: [row(`${prefix}new.txt`)], total: 1, page });
    assert.equal(await retry.action.run(), true);
    assert.equal(archive.archiveRefreshStatus(), "idle");
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.equal(toasts.toasts().some((toast) => toast.key === "archive-refresh-error"), false);
  });
});

test("a refresh never restores rows or reports success after the archive is closed", async () => {
  await withArchive(async ({ archive, ipc, closed, requests, pendingOpen }) => {
    const pendingPage = deferred();
    ipc.listEntries = (id, page, prefix) => {
      requests.push({ id, page, prefix });
      return pendingPage.promise;
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    await until(() => requests.length > 0);
    archive.closeArchive();
    pendingPage.resolve({ items: [row("docs/new.txt")], total: 1, page: 0 });
    assert.equal(await refreshing, false);
    assert.equal(archive.archive(), null);
    assert.deepEqual(archive.loadedRows(), []);
    assert.deepEqual(closed.sort(), [1, 2]);
  });
});

test("refresh replaces search results directly without clearing the query or loading the root", async () => {
  await withArchive(async ({ archive, ipc, requests, pendingOpen }) => {
    const searches = [];
    ipc.searchEntries = async (id, page, query) => {
      searches.push({ id, page, query });
      return { items: [row("pictures/new.txt")], total: 1, page };
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.deepEqual(requests, []);
    assert.deepEqual(searches, [{ id: 2, page: 0, query: "new" }]);
    assert.equal(archive.filterText(), " new ");
    assert.deepEqual(archive.currentDirs(), ["docs"]);
  }, { filter: " new " });
});

test("large archive refresh loads only the visible pages and follows scrolling during the request", async () => {
  await withArchive(async ({ archive, ipc, requests, pendingOpen }) => {
    const pendingPage = deferred();
    let visible = { start: 4_490, end: 4_530 };
    ipc.listEntries = async (id, page, prefix) => {
      requests.push({ id, page, prefix });
      if (page === 8) await pendingPage.promise;
      return { items: Array.from({ length: 500 }, (_, offset) => row(`docs/item-${page * 500 + offset}`)), total: 10_000, page };
    };
    const refreshing = archive.refreshCurrentArchive(() => visible);
    pendingOpen.resolve(info(2));
    await until(() => requests.length > 0);
    assert.equal(requests[0].page, 8);
    visible = { start: 7_500, end: 7_540 };
    pendingPage.resolve();
    assert.equal(await refreshing, true);
    assert.equal(archive.rowAt(7_500)?.path, "docs/item-7500");
    assert.ok(archive.loadedRowCount() <= archive.PAGE_SIZE * 2);
    assert.equal(requests.some(({ page }) => page === 0), false);
  }, { total: 10_000 });
});

test("refresh loads the last valid viewport when deletion shortens the list", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    ipc.listEntries = async (_id, page) => ({
      items: page === 0 ? Array.from({ length: 25 }, (_, index) => row(`docs/item-${index}`)) : [],
      total: 25, page,
    });
    const refreshing = archive.refreshCurrentArchive(() => ({ start: 1_000, end: 1_040 }));
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.equal(archive.totalRows(), 25);
    assert.equal(archive.rowAt(24)?.path, "docs/item-24");
  }, { total: 2_000 });
});

test("a newer search replaces a pending refresh search, including its late failure", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen, toasts }) => {
    const pendingSearch = deferred();
    const searches = [];
    ipc.searchEntries = async (id, page, query) => {
      searches.push({ id, query });
      if (query === "old") return pendingSearch.promise;
      return { items: [row("docs/new.txt")], total: 1, page };
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    await until(() => searches.length > 0);
    archive.setFilter("new");
    pendingSearch.reject({ key: "error.io", params: {}, detail: "obsolete error" });
    assert.equal(await refreshing, true);
    assert.equal(archive.filterText(), "new");
    assert.equal(archive.filterPending(), false);
    assert.deepEqual(searches, [{ id: 2, query: "old" }, { id: 2, query: "new" }]);
    assert.equal(toasts.toasts().some((toast) => toast.key === "archive-refresh-error"), false);
  }, { filter: "old" });
});

test("a newer archive supersedes a pending refresh and keeps its own view", async () => {
  await withArchive(async ({ archive, ipc, closed, requests, pendingOpen }) => {
    const pendingPage = deferred();
    ipc.listEntries = (id, page, prefix) => {
      requests.push({ id, page, prefix });
      return pendingPage.promise;
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    await until(() => requests.length > 0);
    ipc.openArchive = async () => ({ ...info(3), path: "/tmp/other.zip", source: "/tmp/other.zip" });
    ipc.listEntries = async (_id, page) => ({ items: [row("other.txt")], total: 1, page });
    assert.equal(await archive.openArchive("/tmp/other.zip"), true);
    pendingPage.resolve({ items: [row("docs/new.txt")], total: 1, page: 0 });
    assert.equal(await refreshing, false);
    assert.equal(archive.archive().id, 3);
    assert.deepEqual(archive.currentDirs(), []);
    assert.equal(archive.rowAt(0)?.path, "other.txt");
    assert.deepEqual(closed.sort(), [1, 2]);
  });
});

test("refresh password retry preserves the browse context and selected filename encoding", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.password_required", params: {}, detail: "" });
    assert.equal(await refreshing, false);
    assert.equal(archive.openPasswordPrompt().encoding, "gbk");
    assert.equal(archive.archive().id, 1);
    ipc.openArchive = async (path, password, encoding) => {
      assert.equal(path, "/tmp/refresh.zip");
      assert.equal(password, "test-only-password");
      assert.equal(encoding, "gbk");
      return info(2);
    };
    assert.equal(await archive.openArchive("/tmp/refresh.zip", "test-only-password", "gbk"), true);
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.equal(archive.archivePasswordBookStatus().session, false, "typing a password does not prove it was cached");
    assert.equal(archive.openPasswordPrompt(), null);
  });
});

test("filename encoding changes reuse atomic refresh", async () => {
  await withArchive(async ({ archive, ipc }) => {
    ipc.openArchive = async (_path, _password, encoding) => {
      assert.equal(encoding, "shift_jis");
      return { ...info(2), encoding_override: encoding };
    };
    assert.equal(await archive.reopenWithEncoding("shift_jis"), true);
    assert.equal(archive.archiveEncoding(), "shift_jis");
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.equal(archive.rowAt(0)?.path, "docs/new.txt");
  });
});

test("retry remains available after dismissing a refresh notification and retains the requested encoding", async () => {
  await withArchive(async ({ archive, ipc, toasts, pendingOpen }) => {
    const reopening = archive.reopenWithEncoding("shift_jis");
    pendingOpen.reject({ key: "error.io", params: {}, detail: "read failed" });
    assert.equal(await reopening, false);
    toasts.removeToastByKey("archive-refresh-error");
    assert.equal(archive.archiveRefreshStatus(), "error");
    assert.equal(archive.archiveEncoding(), "gbk", "the old view remains usable until replacement succeeds");
    ipc.openArchive = async (path, _password, encoding) => {
      assert.equal(path, "/tmp/refresh.zip");
      assert.equal(encoding, "shift_jis");
      return { ...info(2), encoding_override: encoding };
    };
    await archive.retryArchiveBrowse();
    assert.equal(archive.archiveRefreshStatus(), "idle");
    assert.equal(archive.archive().id, 2);
    assert.equal(archive.archiveEncoding(), "shift_jis");
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.equal(archive.rowAt(0)?.path, "docs/new.txt");
  });
});

test("a dismissed refresh retry cannot supersede a newer archive open", async () => {
  await withArchive(async ({ archive, ipc, toasts, pendingOpen }) => {
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.reject({ key: "error.io", params: {}, detail: "read failed" });
    await refreshing;
    const oldRetry = toasts.toasts().find((toast) => toast.key === "archive-refresh-error").action.run;
    const pendingNew = deferred();
    let opens = 0;
    ipc.openArchive = () => { opens += 1; return pendingNew.promise; };
    const opening = archive.openArchive("/tmp/new.zip");
    assert.equal(await oldRetry(), false);
    assert.equal(opens, 1);
    pendingNew.resolve({ ...info(3), path: "/tmp/new.zip", source: "/tmp/new.zip" });
    assert.equal(await opening, true);
    assert.equal(archive.archive().id, 3);
  });
});

test("refresh leaves a missing folder at its nearest surviving parent", async () => {
  await withArchive(async ({ archive, ipc, requests, toasts, pendingOpen }) => {
    ipc.resolveArchiveDirectory = async (id, prefix) => {
      assert.equal(id, 2);
      assert.equal(prefix, "docs/removed/");
      return "docs/";
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.deepEqual(requests, [{ id: 2, page: 0, prefix: "docs/" }]);
    assert.equal(archive.rowAt(0)?.path, "docs/new.txt");
    assert.ok(toasts.toasts().some((toast) => toast.key === "archive-directory-changed"));
  }, { dirs: ["docs", "removed"] });
});

test("refresh preserves an existing empty folder", async () => {
  await withArchive(async ({ archive, ipc, toasts, pendingOpen }) => {
    ipc.listEntries = async (_id, page) => ({ items: [], total: 0, page });
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.deepEqual(archive.currentDirs(), ["docs"]);
    assert.equal(archive.totalRows(), 0);
    assert.equal(toasts.toasts().some((toast) => toast.key === "archive-directory-changed"), false);
  });
});

test("a search keeps its query when its missing browsing folder is resolved", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen }) => {
    ipc.resolveArchiveDirectory = async () => "";
    ipc.searchEntries = async (_id, page, query) => {
      assert.equal(query, "new");
      return { items: [row("new.txt")], total: 1, page };
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    assert.equal(await refreshing, true);
    assert.equal(archive.filterText(), "new");
    assert.deepEqual(archive.currentDirs(), []);
    assert.equal(archive.rowAt(0)?.path, "new.txt");
  }, { filter: "new" });
});

test("navigation supersedes a pending directory resolution", async () => {
  await withArchive(async ({ archive, ipc, pendingOpen, toasts }) => {
    const pendingDirectory = deferred();
    let resolving = false;
    ipc.resolveArchiveDirectory = async (_id, prefix) => {
      if (prefix === "docs/") {
        resolving = true;
        return pendingDirectory.promise;
      }
      return prefix;
    };
    const refreshing = archive.refreshCurrentArchive();
    pendingOpen.resolve(info(2));
    await until(() => resolving);
    await archive.enterDirPath("pictures/");
    pendingDirectory.resolve("");
    assert.equal(await refreshing, true);
    assert.deepEqual(archive.currentDirs(), ["pictures"]);
    assert.equal(archive.rowAt(0)?.path, "pictures/new.txt");
    assert.equal(toasts.toasts().some((toast) => toast.key === "archive-directory-changed"), false);
  });
});

test("navigation to a missing folder displays a real parent instead of a phantom empty folder", async () => {
  await withArchive(async ({ archive, ipc, requests }) => {
    ipc.listEntries = async (id, page, prefix) => {
      requests.push({ id, page, prefix });
      return { items: prefix ? [] : [row("kept.txt")], total: prefix ? 0 : 1, page };
    };
    ipc.resolveArchiveDirectory = async () => "";
    await archive.enterDirPath("removed/");
    assert.deepEqual(archive.currentDirs(), []);
    assert.equal(archive.rowAt(0)?.path, "kept.txt");
    assert.deepEqual(requests.map(({ prefix }) => prefix), ["removed/", ""]);
  });
});

test("virtual rows keep cold and mixed page slots and clamp after the list shrinks", async () => {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const declaration = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "browseVirtualWindow");
  const { outputText } = ts.transpileModule(declaration.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } });
  for (const total of [0, 1, 25, 501]) {
    const window = vm.runInNewContext(`${outputText}\nbrowseVirtualWindow()`, {
      currentArchive: info(), totalRows: () => total, browseScrollTop: 40_000,
      browseViewportHeight: 400, MODERN_ROW_HEIGHT: 40, VIRTUAL_OVERSCAN_ROWS: 5,
    });
    assert.ok(window.start <= window.end);
    assert.ok(window.end <= total);
    assert.ok(window.top <= total * 40);
    assert.equal(window.top + (window.end - window.start) * 40 + window.bottom, total * 40);
  }
  const names = ["browseVirtualWindow", "browseEntries", "browsePaddingTop", "browsePaddingBottom", "toDisplayEntry"];
  const helpers = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const surfaces = ["classicArchiveBrowserSurface", "modernArchiveBrowserSurface"].map((name) => {
    const surface = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
    const rows = surface.body.statements.find((node) => ts.isVariableStatement(node)
      && node.declarationList.declarations.some((item) => item.name.getText(source) === "rows"));
    const result = surface.body.statements.find((node) => ts.isReturnStatement(node)).expression;
    const view = result.properties.find((node) => node.name?.getText(source) === "view").initializer;
    const fields = view.properties.filter((node) => ["rows", "startIndex", "rowsPending", "paddingTop", "paddingBottom"]
      .includes(node.name?.getText(source)));
    assert.equal(fields.length, 5);
    return `function ${name}Rows() { ${rows.getText(source)} return { ${fields.map((node) => node.getText(source)).join(",")} }; }`;
  });
  const runtime = ts.transpileModule([...helpers.map((node) => node.getText(source)), ...surfaces].join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText;
  await withArchive(async ({ archive, ipc, requests }) => {
    const cached = Array.from({ length: 500 }, (_, index) => row(`entry-${index}.txt`));
    archive.installArchivePreview(info(), cached, { total: 10_000 });
    const pendingPages = new Map();
    ipc.listEntries = (id, page, prefix) => {
      requests.push({ id, page, prefix });
      const pending = deferred();
      pendingPages.set(page, pending);
      return pending.promise;
    };
    const context = {
      get currentArchive() { return archive.archive(); },
      totalRows: archive.totalRows, rowAt: archive.rowAt, prefetchAround: archive.prefetchAround,
      archiveBrowseError: archive.archiveBrowseError, filterPending: archive.filterPending, filterText: archive.filterText,
      browseScrollTop: 0, browseViewportHeight: 420, MODERN_ROW_HEIGHT: 42, CLASSIC_ROW_HEIGHT: 29,
      VIRTUAL_OVERSCAN_ROWS: 12, entryType: (entry) => entry.entry_type,
      formatBytes: String, formatModified: () => "", entryAttributeLabel: () => "File",
      tr: (_key, fallback) => fallback, isEntrySelected: () => false,
      isEntryPreviewActive: () => false, isEntryPreviewBusy: () => false,
      entrySelectionLabel: () => "Select", previewEntryActionLabel: () => "Preview", previewActionIcon: () => "eye",
    };
    const app = vm.runInNewContext(`${runtime}\n({browseVirtualWindow, classicArchiveBrowserSurfaceRows, modernArchiveBrowserSurfaceRows})`, context);
    function checkSlots(surface, height, pending) {
      const window = app.browseVirtualWindow(height);
      const view = surface();
      assert.equal(view.startIndex, window.start);
      assert.equal(view.rows.length, window.end - window.start);
      assert.equal(view.paddingTop + view.rows.length * height + view.paddingBottom, archive.totalRows() * height,
        "loaded and missing rows must contribute the same fixed height");
      assert.equal(view.rowsPending, pending);
      view.rows.forEach((entry, slot) => {
        if (entry) {
          assert.equal(entry.virtualIndex, view.startIndex + slot);
          assert.equal(entry.source.path, `entry-${entry.virtualIndex}.txt`);
        }
      });
      return view;
    }
    for (const [surface, height] of [[app.classicArchiveBrowserSurfaceRows, 29], [app.modernArchiveBrowserSurfaceRows, 42]]) {
      context.browseScrollTop = 500 * height;
      const mixed = checkSlots(surface, height, true);
      assert.equal(mixed.rows.filter(Boolean).length, 12, "cached rows must stay in their original slots");
      assert.ok(mixed.rows.some((entry) => entry === null));
      context.browseScrollTop = 10_000 * height;
      const cold = checkSlots(surface, height, true);
      assert.equal(cold.rows.filter(Boolean).length, 0);
    }
    await until(() => pendingPages.has(19) && pendingPages.has(1));
    const loadedPage = Array.from({ length: 500 }, (_, index) => row(`entry-${500 + index}.txt`));
    pendingPages.get(1).resolve({ items: loadedPage, total: 10_000, page: 1 });
    await until(() => archive.rowAt(500) !== null);
    for (const [surface, height] of [[app.classicArchiveBrowserSurfaceRows, 29], [app.modernArchiveBrowserSurfaceRows, 42]]) {
      context.browseScrollTop = 500 * height;
      assert.ok(checkSlots(surface, height, false).rows.every(Boolean));
    }
    pendingPages.get(19).reject({ key: "error.io", params: {}, detail: "page read failed" });
    await until(() => archive.archiveBrowseError() !== null);
    context.browseScrollTop = 10_000 * 42;
    const failed = checkSlots(app.modernArchiveBrowserSurfaceRows, 42, false);
    assert.ok(failed.rows.every((entry) => entry === null));
    const retrying = archive.retryArchiveBrowse();
    await until(() => requests.filter(({ page }) => page === 19).length === 2);
    const retryView = checkSlots(app.modernArchiveBrowserSurfaceRows, 42, true);
    assert.equal(retryView.startIndex, failed.startIndex);
    pendingPages.get(19).resolve({ items: Array.from({ length: 500 }, (_, index) => row(`entry-${9500 + index}.txt`)),
      total: 10_000, page: 19 });
    await retrying;
    assert.ok(checkSlots(app.modernArchiveBrowserSurfaceRows, 42, false).rows.every(Boolean));
    archive.installArchivePreview(info(2), cached.slice(0, 25), { total: 25 });
    assert.ok(checkSlots(app.modernArchiveBrowserSurfaceRows, 42, false).rows.every(Boolean));
    assert.equal(archive.totalRows(), 25);
    for (const [page, pending] of pendingPages) pending.resolve({ items: [row("stale.txt")], total: 10_000, page });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(archive.totalRows(), 25);
    assert.equal(archive.archiveBrowseError(), null);
  });
});

test("the shared browse recovery view exposes an alert and a disabled retry while loading", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Recovery } = await server.ssrLoadModule("/src/components/ArchiveBrowseRecovery.svelte");
    const props = { state: { message: "Could not refresh the archive", busy: false }, retryLabel: "Retry", onRetry() {} };
    const failed = render(Recovery, { props }).body;
    assert.match(failed, /role="alert"/u);
    assert.match(failed, /<button[^>]*>Retry<\/button>/u);
    assert.doesNotMatch(failed, /disabled|style=/u);
    const loading = render(Recovery, { props: { ...props, state: { message: "Refreshing archive contents…", busy: true } } }).body;
    assert.match(loading, /role="status"/u);
    assert.match(loading, /disabled="" aria-busy="true"/u);
    assert.match(loading, /Refreshing archive contents/u);
    assert.doesNotMatch(render(Recovery, { props: { ...props, state: null } }).body, /<button/u);
  } finally { await server.close(); }
});
