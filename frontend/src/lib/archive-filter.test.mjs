import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

const rows = [
  {
    path: "reports/",
    display: "reports",
    entry_type: "dir",
    size: 0,
    compressed: null,
    modified: null,
    crc: null,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "reports/summary.pdf",
    display: "summary.pdf",
    entry_type: "file",
    size: 512,
    compressed: 256,
    modified: null,
    crc: 0x12345678,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "secret.7z",
    display: "secret.7z",
    entry_type: "file",
    size: 1_024,
    compressed: 900,
    modified: null,
    crc: 0x87654321,
    encrypted: true,
    encoding: "utf-8",
  },
];

function archiveInfo(id, name, entryCount = rows.length) {
  return {
    id,
    path: `/tmp/${name}`,
    name,
    format: "zip",
    entry_count: entryCount,
    volumes: null,
    non_utf8_name_count: 0,
    garbled_count: 0,
    suggested_encoding: null,
    encoding_override: null,
  };
}

async function flushPromises() {
  await Promise.resolve();
  await Promise.resolve();
}

test("archive search spans paths and rejects cancelled, failed, and stale results", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const ipcModule = await server.ssrLoadModule("/src/lib/ipc.ts");
    const toastModule = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    archive.installArchivePreview(
      archiveInfo(9, "product-backup.zip"),
      rows,
      {
        previewRows: rows,
        selected: ["secret.7z"],
        selectedSize: 1_024,
      },
    );
    archive.selectAllLoaded();
    assert.deepEqual(
      [...archive.selectedPaths()].sort(),
      rows.map((row) => row.path).sort(),
    );
    assert.equal(archive.selectedSize(), 1_536);

    const manyRows = Array.from({ length: archive.PAGE_SIZE * 2 + 37 }, (_, index) => ({
      ...rows[1],
      path: `item-${index}.bin`,
      display: `item-${index}.bin`,
      size: index + 1,
    }));
    archive.installArchivePreview(
      archiveInfo(10, "large.zip", manyRows.length),
      manyRows,
      { previewRows: manyRows },
    );
    const selectionProgress = [];
    assert.equal(
      await archive.selectAllRows((loaded, total) => selectionProgress.push([loaded, total])),
      "selected",
    );
    assert.equal(archive.selectedPaths().size, manyRows.length);
    assert.equal(archive.allCurrentRowsSelected(), true);
    assert.equal(
      archive.selectedSize(),
      (manyRows.length * (manyRows.length + 1)) / 2,
    );
    assert.deepEqual(selectionProgress.at(-1), [manyRows.length, manyRows.length]);

    archive.installArchivePreview(
      archiveInfo(9, "product-backup.zip"),
      rows,
      { previewRows: rows },
    );
    archive.clearSelection();
    archive.toggleSelect(rows[2]);
    assert.equal(archive.allCurrentRowsSelected(), false);
    assert.deepEqual([...archive.selectedPaths()], ["secret.7z"]);
    assert.equal(archive.selectedSize(), 1_024);

    const originalSetTimeout = globalThis.setTimeout;
    const originalClearTimeout = globalThis.clearTimeout;
    const timers = [];
    globalThis.setTimeout = (callback, delay = 0) => {
      const timer = {
        id: timers.length + 1,
        callback,
        delay: Number(delay),
        cleared: false,
        fired: false,
      };
      timers.push(timer);
      return timer.id;
    };
    globalThis.clearTimeout = (timer) => {
      const pending = timers.find(({ id }) => id === timer);
      if (pending) pending.cleared = true;
    };
    const fireLatestTimer = () => {
      const timer = [...timers]
        .reverse()
        .find(({ cleared, fired }) => !cleared && !fired);
      assert.ok(timer, "expected a pending debounce timer");
      timer.fired = true;
      timer.callback();
      return timer;
    };

    try {
      archive.setFilter("summary");
      assert.equal(archive.filterPending(), true);
      assert.equal(archive.selectedPaths().size, 0);
      assert.equal(archive.loadedRows().length, 0);
      archive.toggleSelect(rows[2]);
      archive.selectAllLoaded();
      archive.prefetchAround(0);
      assert.equal(archive.selectedPaths().size, 0);
      assert.equal(timers[0].delay, 300);

      fireLatestTimer();
      await flushPromises();
      assert.equal(archive.filterPending(), false);
      assert.equal(archive.totalRows(), 1);
      assert.deepEqual(archive.loadedRows().map((row) => row.path), ["reports/summary.pdf"]);
      assert.equal(archive.findLoadedRow("reports/summary.pdf")?.display, "summary.pdf");
      assert.equal(archive.findLoadedRow("missing.txt"), null);
      assert.equal(archive.allRowsLoaded(), false);

      await archive.enterDirPath("/reports/");
      assert.deepEqual(archive.currentDirs(), ["reports"]);
      assert.equal(archive.filterText(), "");
      assert.equal(archive.filterPending(), false);
      assert.deepEqual(archive.loadedRows().map((row) => row.path), ["reports/summary.pdf"]);
      assert.equal(archive.allRowsLoaded(), true);

      archive.setFilter("secret");
      fireLatestTimer();
      await flushPromises();
      assert.deepEqual(archive.loadedRows().map((row) => row.path), ["secret.7z"]);

      archive.setFilter("");
      fireLatestTimer();
      await flushPromises();
      assert.equal(archive.filterText(), "");
      assert.equal(archive.filterPending(), false);
      assert.deepEqual(archive.currentDirs(), ["reports"]);
      assert.deepEqual(archive.loadedRows().map((row) => row.path), ["reports/summary.pdf"]);

      archive.installArchivePreview(archiveInfo(10, "release-notes.zip"), rows);

      const originalSearchEntries = ipcModule.ipc.searchEntries;
      const originalCancelArchiveSearch = ipcModule.ipc.cancelArchiveSearch;
      const originalCloseArchive = ipcModule.ipc.closeArchive;
      const pendingSearches = [];
      const cancelledGenerations = [];
      ipcModule.ipc.searchEntries = (id, page, query, pageSize, generation) =>
        new Promise((resolve, reject) =>
          pendingSearches.push({ id, page, query, pageSize, generation, resolve, reject }),
        );
      ipcModule.ipc.cancelArchiveSearch = async (id, generation) => {
        cancelledGenerations.push({ id, generation });
      };
      ipcModule.ipc.closeArchive = async () => {};
      try {
        archive.setFilter("reports");
        archive.prefetchAround(0);
        assert.equal(pendingSearches.length, 0);
        fireLatestTimer();
        await flushPromises();
        assert.equal(pendingSearches.length, 1);
        assert.deepEqual(
          {
            id: pendingSearches[0].id,
            page: pendingSearches[0].page,
            query: pendingSearches[0].query,
            pageSize: pendingSearches[0].pageSize,
          },
          { id: 10, page: 0, query: "reports", pageSize: archive.PAGE_SIZE },
        );
        assert.equal(Number.isSafeInteger(pendingSearches[0].generation), true);

        archive.setFilter("summary");
        assert.equal(cancelledGenerations.at(-1).id, 10);
        assert.ok(cancelledGenerations.at(-1).generation > pendingSearches[0].generation);
        archive.prefetchAround(0);
        assert.equal(pendingSearches.length, 1);
        pendingSearches[0].resolve({ items: [rows[0]], total: 1, page: 0 });
        await flushPromises();
        assert.equal(archive.filterPending(), true);
        assert.deepEqual(archive.loadedRows(), []);
        assert.equal(archive.archiveBrowseError(), null);

        fireLatestTimer();
        await flushPromises();
        assert.equal(pendingSearches.length, 2);
        assert.equal(pendingSearches[1].query, "summary");
        assert.ok(pendingSearches[1].generation > pendingSearches[0].generation);
        pendingSearches[1].resolve(null);
        await flushPromises();
        assert.equal(archive.filterPending(), false);
        assert.deepEqual(archive.loadedRows(), []);
        assert.equal(archive.archiveBrowseError(), null);

        archive.setFilter("broken");
        fireLatestTimer();
        await flushPromises();
        const searchError = {
          key: "error.io",
          params: { detail: "read failed" },
          detail: "read failed",
        };
        pendingSearches[2].reject(searchError);
        await flushPromises();
        assert.equal(archive.filterPending(), false);
        assert.deepEqual(archive.archiveBrowseError(), searchError);
        assert.deepEqual(archive.loadedRows(), []);
        const browseToast = toastModule
          .toasts()
          .find((toast) => toast.key === "archive-browse-error");
        assert.ok(browseToast);
        assert.equal(JSON.stringify(browseToast).includes("read failed"), false);

        const retry = archive.retryArchiveBrowse();
        assert.equal(pendingSearches.length, 4);
        assert.equal(pendingSearches[3].query, "broken");
        assert.equal(archive.archiveBrowseError(), null);
        pendingSearches[3].resolve({ items: [rows[1]], total: 1, page: 0 });
        await retry;
        assert.deepEqual(archive.loadedRows().map((row) => row.path), ["reports/summary.pdf"]);
        assert.equal(archive.allRowsLoaded(), false);

        archive.installArchivePreview(archiveInfo(11, "private.zip"), rows);
        archive.setFilter("secret");
        fireLatestTimer();
        await flushPromises();
        assert.equal(pendingSearches.length, 5);

        archive.closeArchive();
        pendingSearches[4].resolve({ items: [rows[2]], total: 1, page: 0 });
        await flushPromises();
        assert.equal(archive.archive(), null);
        assert.equal(archive.totalRows(), 0);
        assert.deepEqual(archive.loadedRows(), []);
        assert.equal(archive.archiveBrowseError(), null);

        archive.installArchivePreview(archiveInfo(12, "pagination-race.zip"), rows);
        archive.setFilter("first");
        fireLatestTimer();
        await flushPromises();
        pendingSearches[5].resolve({ items: [rows[0]], total: 1_001, page: 0 });
        await flushPromises();
        assert.equal(archive.rowAt(archive.PAGE_SIZE), null);
        assert.equal(pendingSearches[6].page, 1);

        archive.setFilter("second");
        fireLatestTimer();
        await flushPromises();
        pendingSearches[7].resolve({ items: [rows[1]], total: 1_001, page: 0 });
        await flushPromises();
        assert.equal(archive.rowAt(archive.PAGE_SIZE), null);
        assert.equal(pendingSearches[8].page, 1);
        const loadedSecondPage = archive.loadRowAt(archive.PAGE_SIZE);
        assert.equal(
          pendingSearches.length,
          9,
          "awaiting a row must share the page request already started by the virtual list",
        );

        pendingSearches[6].resolve({ items: [rows[0]], total: 1_001, page: 1 });
        await flushPromises();
        assert.equal(archive.rowAt(archive.PAGE_SIZE), null);
        assert.equal(
          pendingSearches.length,
          9,
          "a stale page must not clear the current generation's in-flight marker",
        );
        pendingSearches[8].resolve({ items: [rows[2]], total: 1_001, page: 1 });
        assert.equal((await loadedSecondPage)?.path, "secret.7z");
        assert.equal(archive.rowAt(archive.PAGE_SIZE)?.path, "secret.7z");
        assert.equal(await archive.loadRowAt(-1), null);
        assert.equal(await archive.loadRowAt(archive.totalRows()), null);
        archive.closeArchive();
      } finally {
        ipcModule.ipc.searchEntries = originalSearchEntries;
        ipcModule.ipc.cancelArchiveSearch = originalCancelArchiveSearch;
        ipcModule.ipc.closeArchive = originalCloseArchive;
      }
    } finally {
      globalThis.setTimeout = originalSetTimeout;
      globalThis.clearTimeout = originalClearTimeout;
    }
  } finally {
    await server.close();
  }
});

test("a later row selection invalidates an in-flight full selection", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const ipcModule = await server.ssrLoadModule("/src/lib/ipc.ts");
    const originalListEntries = ipcModule.ipc.listEntries;
    const originalCancelArchiveSearch = ipcModule.ipc.cancelArchiveSearch;
    const originalCloseArchive = ipcModule.ipc.closeArchive;
    const allRows = Array.from({ length: archive.PAGE_SIZE * 2 + 1 }, (_, index) => ({
      ...rows[1],
      path: `race-${index}.bin`,
      display: `race-${index}.bin`,
      size: index + 1,
    }));
    const pendingPages = [];

    archive.installArchivePreview(
      archiveInfo(14, "selection-race.zip", allRows.length),
      allRows.slice(0, archive.PAGE_SIZE),
      {
        total: allRows.length,
        pages: new Map([[0, allRows.slice(0, archive.PAGE_SIZE)]]),
      },
    );
    ipcModule.ipc.cancelArchiveSearch = async () => {};
    ipcModule.ipc.closeArchive = async () => {};
    ipcModule.ipc.listEntries = (_id, page) =>
      new Promise((resolve) => pendingPages.push({ page, resolve }));

    try {
      const selecting = archive.selectAllRows();
      await flushPromises();
      assert.deepEqual(
        pendingPages.map(({ page }) => page),
        [1, 2],
      );

      archive.clearSelection();
      archive.toggleSelect(allRows[0]);
      pendingPages[0].resolve({
        items: allRows.slice(archive.PAGE_SIZE, archive.PAGE_SIZE * 2),
        total: allRows.length,
        page: 1,
      });
      pendingPages[1].resolve({
        items: allRows.slice(archive.PAGE_SIZE * 2),
        total: allRows.length,
        page: 2,
      });

      assert.equal(await selecting, "stale");
      assert.deepEqual([...archive.selectedPaths()], [allRows[0].path]);
      assert.equal(archive.allCurrentRowsSelected(), false);
    } finally {
      archive.closeArchive();
      await flushPromises();
      ipcModule.ipc.listEntries = originalListEntries;
      ipcModule.ipc.cancelArchiveSearch = originalCancelArchiveSearch;
      ipcModule.ipc.closeArchive = originalCloseArchive;
    }
  } finally {
    await server.close();
  }
});

test("replacing or dismissing an archive open cancels its exact backend request", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const ipcModule = await server.ssrLoadModule("/src/lib/ipc.ts");
    const originalOpenArchive = ipcModule.ipc.openArchive;
    const originalCancelArchiveOpen = ipcModule.ipc.cancelArchiveOpen;
    const originalCloseArchive = ipcModule.ipc.closeArchive;
    const pendingOpens = [];
    const cancelledRequests = [];
    const closedArchives = [];

    ipcModule.ipc.openArchive = (path, password, encoding, requestId) =>
      new Promise((resolve, reject) => {
        pendingOpens.push({ path, password, encoding, requestId, resolve, reject });
      });
    ipcModule.ipc.cancelArchiveOpen = async (requestId) => {
      cancelledRequests.push(requestId);
    };
    ipcModule.ipc.closeArchive = async (id) => {
      closedArchives.push(id);
    };

    try {
      const first = archive.openArchive("first.zip");
      await flushPromises();
      assert.equal(pendingOpens.length, 1);
      assert.ok(pendingOpens[0].requestId);

      const second = archive.openArchive("second.zip");
      await flushPromises();
      assert.equal(pendingOpens.length, 2);
      assert.notEqual(pendingOpens[0].requestId, pendingOpens[1].requestId);
      assert.deepEqual(cancelledRequests, [pendingOpens[0].requestId]);

      pendingOpens[0].resolve(archiveInfo(41, "first.zip"));
      assert.equal(await first, false);
      assert.deepEqual(closedArchives, [41]);

      archive.cancelPendingArchiveOpen();
      await flushPromises();
      assert.deepEqual(cancelledRequests, [
        pendingOpens[0].requestId,
        pendingOpens[1].requestId,
      ]);
      pendingOpens[1].reject({
        key: "error.cancelled",
        params: {},
        detail: "",
      });
      assert.equal(await second, false);
      assert.equal(archive.archive(), null);
    } finally {
      ipcModule.ipc.openArchive = originalOpenArchive;
      ipcModule.ipc.cancelArchiveOpen = originalCancelArchiveOpen;
      ipcModule.ipc.closeArchive = originalCloseArchive;
      archive.closeArchive();
    }
  } finally {
    await server.close();
  }
});
