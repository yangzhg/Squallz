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
    ipc.archivePasswordStatus = async () => ({ available: true, saved: false });
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
    assert.equal(archive.archiveHasSessionPassword(), true);
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

test("virtual rows stay within the updated list when its size shrinks below the scroll offset", async () => {
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
});
