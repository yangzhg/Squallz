import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { parseCss } from "svelte/compiler";
import { createTestServer } from "../../tests/runtime.mjs";

const rows = Array.from({ length: 8 }, (_, index) => ({
  path: `资料-${index}.txt`,
  display: `资料-${index}.txt`,
  entry_type: "file",
  size: index + 1,
  compressed: null,
  modified: null,
  crc: null,
  encrypted: false,
  encoding: "utf-8",
}));

function info(total) {
  return {
    id: 17, path: "/tmp/selection.zip", source: "/tmp/selection.zip",
    name: "selection.zip", format: "zip", entry_count: total,
    volumes: null, non_utf8_name_count: 0, garbled_count: 0,
    suggested_encoding: null, encoding_override: null,
  };
}

async function loadSelectionHandlers(archive, overrides = {}) {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const names = new Set([
    "selectEntry", "selectOnlyEntry", "showEntryContextAt", "runArchiveSelection", "toggleEntrySelection",
    "submitDeleteSelectedJob", "selectedDeletePatterns", "closeEntryContext",
    "selectedRenameSource", "canRenameSelection", "hasArchiveSelection", "hasArchiveOpen", "submitRenameSelectedJob",
  ]);
  const declarations = source.statements.filter(
    (node) => ts.isFunctionDeclaration(node) && names.has(node.name?.text),
  );
  const { outputText } = ts.transpileModule(
    declarations.map((node) => node.getText(source)).join("\n"),
    { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } },
  );
  return vm.runInNewContext(`${outputText}\n({ selectEntry, selectOnlyEntry, showEntryContextAt, toggleEntrySelection, submitDeleteSelectedJob, canRenameSelection, submitRenameSelectedJob, context: () => entryContext })`, {
    ...archive,
    archiveSelectionBusyReason: () => "",
    entryPreviewForPath: () => null,
    clearEntryPreviewState: () => {},
    recordValidationEvent: () => {},
    closeQuickActions: () => {},
    blockSelectionScopedAction: () => false,
    archiveMutationDisabledReason: () => "",
    currentArchive: archive.archive(),
    archiveTitle: () => "selection.zip",
    recordOperation: () => {},
    showNotice: () => {},
    tr: (_key, fallback) => fallback,
    selectionProgressLabel: (loaded, total) => `${loaded}/${total}`,
    archiveSelectionProgress: null,
    entryContext: null,
    window: { innerWidth: 1280, innerHeight: 800 },
    ...overrides,
  });
}

function displayEntry(index) {
  return { name: rows[index].display, source: rows[index], virtualIndex: index };
}

test("opening a context menu selects only an unselected target and preserves a selected group", async () => {
  const server = await createTestServer();
  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    archive.installArchivePreview(info(rows.length), rows, { previewRows: rows });
    const submitted = [];
    const handlers = await loadSelectionHandlers(archive, {
      submitCurrentArchiveJob: async (spec) => { submitted.push(spec); return true; },
    });
    await handlers.selectEntry(displayEntry(1));
    handlers.showEntryContextAt(200, 200, displayEntry(4));
    assert.deepEqual([...archive.selectedPaths()], [rows[4].path]);
    assert.equal(archive.selectedSize(), rows[4].size);
    await handlers.submitDeleteSelectedJob();
    assert.deepEqual(Array.from(submitted[0].delete), [rows[4].path]);

    await handlers.selectEntry(displayEntry(6), { metaKey: true });
    handlers.showEntryContextAt(200, 200, displayEntry(4));
    assert.deepEqual([...archive.selectedPaths()], [rows[4].path, rows[6].path]);
    assert.equal(archive.selectedSize(), rows[4].size + rows[6].size);
    await handlers.submitDeleteSelectedJob();
    assert.deepEqual(Array.from(submitted[1].delete), [rows[4].path, rows[6].path]);
  } finally {
    await server.close();
  }
});

test("single-entry rename accepts a file or folder but rejects mixed selections", async () => {
  await withArchive(async (archive) => {
    const folder = { ...rows[0], path: "folder/", display: "folder", entry_type: "dir", size: 0 };
    archive.installArchivePreview(info(rows.length + 1), [...rows, folder]);
    const submitted = [];
    const notices = [];
    const handlers = await loadSelectionHandlers(archive, {
      showNotice: (message) => notices.push(message),
      submitCurrentArchiveJob: async (spec) => { submitted.push(spec); return true; },
    });
    archive.selectRow(rows[1], 1);
    assert.equal(handlers.canRenameSelection(), true);
    archive.toggleSelect(folder, rows.length);
    assert.equal(handlers.canRenameSelection(), false);
    await handlers.submitRenameSelectedJob();
    assert.deepEqual(submitted, []);
    assert.deepEqual(notices, ["Select exactly one file or folder before renaming"]);
    archive.selectRow(folder, rows.length);
    assert.equal(handlers.canRenameSelection(), true);
    archive.clearSelection();
    assert.equal(handlers.canRenameSelection(), false);
  });
});

test("a context menu cannot advertise a different target during pending selection", async () => {
  await withArchive(async (archive) => {
    archive.installArchivePreview(info(rows.length), rows, { previewRows: rows });
    archive.selectRow(rows[1], 1);
    let busy = "Selecting 500 of 1003…";
    const notices = [];
    const handlers = await loadSelectionHandlers(archive, {
      archiveSelectionBusyReason: () => busy,
      showNotice: (message) => notices.push(message),
    });
    handlers.showEntryContextAt(200, 200, displayEntry(4));
    assert.equal(handlers.context(), null);
    assert.deepEqual([...archive.selectedPaths()], [rows[1].path]);
    assert.deepEqual(notices, [busy]);
    busy = "";
    handlers.showEntryContextAt(200, 200, displayEntry(4));
    assert.equal(handlers.context().path, rows[4].path);
    assert.deepEqual([...archive.selectedPaths()], [rows[4].path]);
  });
});

test("archive preview rows use the same directory order as range selection", async () => {
  const server = await createTestServer();
  try {
    const { readRuntimePreviews } = await server.ssrLoadModule("/src/lib/dev-preview-data.ts");
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const preview = readRuntimePreviews(new URLSearchParams("previewArchive=1"), archive.PAGE_SIZE).archive;
    assert.ok(preview);
    assert.equal(preview.info.entry_count, 7);
    assert.equal(preview.total, 6);
    assert.equal(preview.rows.some((row) => row.path === "reports/Launch plan.pdf"), false);
    archive.installArchivePreview(preview.info, preview.rows, {
      previewRows: preview.previewRows, total: preview.total,
    });
    archive.selectRow(preview.rows[2], 2);
    assert.equal(await archive.selectRangeTo(preview.rows[5], 5), "selected");
    assert.deepEqual([...archive.selectedPaths()], preview.rows.slice(2, 6).map((row) => row.path));
  } finally {
    await server.close();
  }
});

async function withArchive(run) {
  const server = await createTestServer();
  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const original = { ...ipc };
    ipc.cancelArchiveSearch = async () => {};
    ipc.closeArchive = async () => {};
    try {
      await run(archive, ipc);
    } finally {
      archive.closeArchive();
      Object.assign(ipc, original);
    }
  } finally {
    await server.close();
  }
}

function manyRows(total) {
  return Array.from({ length: total }, (_, index) => ({
    ...rows[0], path: `reports/item-${index}.txt`, display: `item-${index}.txt`, size: index + 1,
  }));
}

function installPages(archive, entries, pageNumbers, options = {}) {
  const pages = new Map(pageNumbers.map((page) => [
    page, entries.slice(page * archive.PAGE_SIZE, (page + 1) * archive.PAGE_SIZE),
  ]));
  archive.installArchivePreview(info(entries.length), pages.get(0) ?? [], {
    total: entries.length, pages, dirs: ["reports"], ...options,
  });
}

test("range selection fetches only missing intersecting pages with bounded concurrency", async () => {
  await withArchive(async (archive, ipc) => {
    const entries = manyRows(3_205);
    installPages(archive, entries, [0, 6]);
    const cachedCount = archive.loadedRowCount();
    const requested = [];
    let inFlight = 0;
    let peak = 0;
    ipc.listEntries = async (id, page, prefix, filter, pageSize) => {
      requested.push(page);
      assert.deepEqual([id, prefix, filter, pageSize], [17, "reports/", null, 500]);
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await new Promise(setImmediate);
      inFlight -= 1;
      return { page, total: entries.length, items: entries.slice(page * pageSize, (page + 1) * pageSize) };
    };
    archive.selectRow(entries[497], 497);
    const progress = [];
    assert.equal(await archive.selectRangeTo(entries[3_102], 3_102, false,
      (loaded, total) => progress.push([loaded, total])), "selected");
    assert.deepEqual(requested, [1, 2, 3, 4, 5]);
    assert.ok(peak > 1 && peak <= 4, `expected bounded concurrent reads, got ${peak}`);
    assert.deepEqual([...archive.selectedPaths()], entries.slice(497, 3_103).map((row) => row.path));
    assert.equal(archive.selectedSize(), entries.slice(497, 3_103).reduce((sum, row) => sum + row.size, 0));
    assert.deepEqual(progress[0], [0, 2_606]);
    assert.deepEqual(progress.at(-1), [2_606, 2_606]);
    assert.equal(archive.loadedRowCount(), cachedCount, "selection must not retain every page");
    assert.equal(archive.allCurrentRowsSelected(), false);
    assert.equal(await archive.selectAllRows(), "selected");
    assert.equal(archive.allCurrentRowsSelected(), true);
    assert.equal(archive.selectedPaths().size, entries.length);
    assert.equal(archive.loadedRowCount(), cachedCount);
  });
});

test("range selection uses search-result ordering instead of directory pages", async () => {
  await withArchive(async (archive, ipc) => {
    const entries = manyRows(1_010).map((row, index) => ({ ...row, path: `${index % 2 ? "reports" : "other"}/${row.display}` }));
    installPages(archive, entries, [0, 2], { filter: "item" });
    const requested = [];
    ipc.listEntries = async () => { assert.fail("search selection must not list a directory"); };
    ipc.searchEntries = async (id, page, query, size, generation) => {
      requested.push({ page, generation });
      assert.deepEqual([id, query, size], [17, "item", 500]);
      return { page, total: entries.length, items: entries.slice(page * size, (page + 1) * size) };
    };
    archive.selectRow(entries[1_005], 1_005);
    assert.equal(await archive.selectRangeTo(entries[498], 498), "selected");
    assert.deepEqual(requested.map(({ page }) => page), [1]);
    assert.deepEqual([...archive.selectedPaths()], entries.slice(498, 1_006).map((row) => row.path));
  });
});

test("failed, cancelled, short, or changed pages never publish a partial selection", async () => {
  await withArchive(async (archive, ipc) => {
    const entries = manyRows(1_010);
    for (const [response, expected] of [
      [async () => { throw new Error("page unavailable"); }, "failed"],
      [async () => null, "stale"],
      [async () => ({ page: 1, total: 1_011, items: entries.slice(500, 1_000) }), "stale"],
      [async () => ({ page: 1, total: 1_010, items: entries.slice(500, 999) }), "failed"],
    ]) {
      installPages(archive, entries, [0, 2]);
      archive.selectRow(entries[498], 498);
      ipc.listEntries = response;
      assert.equal(await archive.selectRangeTo(entries[1_005], 1_005), expected);
      assert.deepEqual([...archive.selectedPaths()], [entries[498].path]);
      assert.equal(archive.selectedSize(), entries[498].size);
      assert.equal(archive.allCurrentRowsSelected(), false);
    }
    ipc.listEntries = async (_id, page) => ({ page, total: entries.length, items: entries.slice(500, 1_000) });
    assert.equal(await archive.selectRangeTo(entries[1_005], 1_005), "selected");
  });
});

test("later row, search, directory, archive, or range changes invalidate pending selection", async () => {
  await withArchive(async (archive, ipc) => {
    const entries = manyRows(1_010);
    for (const change of ["row", "search", "directory", "archive", "range"]) {
      installPages(archive, entries, [0, 2]);
      archive.selectRow(entries[498], 498);
      let release;
      let requested;
      const started = new Promise((resolve) => { requested = resolve; });
      ipc.listEntries = async (_id, page) => {
        if (page === 0) return { page, total: 2, items: entries.slice(0, 2) };
        requested();
        return new Promise((resolve) => { release = resolve; });
      };
      const pending = archive.selectRangeTo(entries[1_005], 1_005);
      await started;
      if (change === "row") archive.selectRow(entries[2], 2);
      if (change === "search") archive.setFilter("different");
      if (change === "directory") await archive.enterDirPath("other/");
      if (change === "archive") archive.installArchivePreview({ ...info(rows.length), id: 99 }, rows, { previewRows: rows });
      if (change === "range") assert.equal(await archive.selectRangeTo(entries[499], 499), "selected");
      const selection = [...archive.selectedPaths()];
      const size = archive.selectedSize();
      release({ page: 1, total: entries.length, items: entries.slice(500, 1_000) });
      assert.equal(await pending, "stale", change);
      assert.deepEqual([...archive.selectedPaths()], selection, change);
      assert.equal(archive.selectedSize(), size, change);
      if (["search", "directory", "archive"].includes(change)) assert.equal(selection.length, 0, change);
      archive.closeArchive();
    }
  });
});

test("single rows, checkboxes, full selection, and navigation share range anchors", async () => {
  await withArchive(async (archive) => {
    const entries = [{ ...rows[0], path: "folder/", display: "folder", entry_type: "dir", size: 0 }, ...rows];
    archive.installArchivePreview(info(entries.length), entries, { previewRows: entries });
    const handlers = await loadSelectionHandlers(archive);
    handlers.toggleEntrySelection(displayEntry(1));
    handlers.toggleEntrySelection(displayEntry(4), { shiftKey: true });
    await new Promise(setImmediate);
    assert.deepEqual([...archive.selectedPaths()], rows.slice(1, 5).map((row) => row.path));
    assert.equal(await archive.selectAllRows(), "selected");
    assert.equal(archive.allCurrentRowsSelected(), true);
    assert.equal(await archive.selectRangeTo(entries[0], 0), "selected");
    assert.deepEqual([...archive.selectedPaths()], ["folder/"]);
    assert.equal(archive.selectedSize(), 0);
    archive.clearSelection();
    assert.equal(await archive.selectRangeTo(entries[3], 3), "selected");
    assert.deepEqual([...archive.selectedPaths()], [entries[3].path]);
    await archive.enterDirPath("folder/");
    assert.equal(archive.selectedPaths().size, 0);
    await archive.goUp();
    assert.equal(await archive.selectRangeTo(entries[1], 1), "selected");
    assert.deepEqual([...archive.selectedPaths()], [entries[1].path]);
  });
});

test("a synchronously completed selection cannot replace a newer row's anchor", async () => {
  await withArchive(async (archive) => {
    archive.installArchivePreview(info(rows.length), rows, { previewRows: rows });
    for (const operation of [() => archive.selectAllRows(), () => archive.selectRangeTo(rows[4], 4)]) {
      archive.selectRow(rows[1], 1);
      const pending = operation();
      archive.selectRow(rows[6], 6);
      await pending;
      assert.deepEqual([...archive.selectedPaths()], [rows[6].path]);
      assert.equal(await archive.selectRangeTo(rows[7], 7), "selected");
      assert.deepEqual([...archive.selectedPaths()], [rows[6].path, rows[7].path]);
    }
  });
});

test("Shift-click extends from the selection anchor and can shrink or add a range", async () => {
  const server = await createTestServer();
  try {
    const archive = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    archive.installArchivePreview(info(rows.length), rows, { previewRows: rows });
    const handlers = await loadSelectionHandlers(archive);
    await handlers.selectEntry(displayEntry(2));
    await handlers.selectEntry(displayEntry(5), { shiftKey: true });
    assert.deepEqual([...archive.selectedPaths()], rows.slice(2, 6).map((row) => row.path));
    await handlers.selectEntry(displayEntry(3), { shiftKey: true });
    assert.deepEqual([...archive.selectedPaths()], rows.slice(2, 4).map((row) => row.path));
    await handlers.selectEntry(displayEntry(7), { ctrlKey: true });
    await handlers.selectEntry(displayEntry(5), { shiftKey: true, ctrlKey: true });
    assert.deepEqual([...archive.selectedPaths()].sort(), [2, 3, 5, 6, 7].map((i) => rows[i].path).sort());
    assert.equal(archive.selectedSize(), [2, 3, 5, 6, 7].reduce((sum, i) => sum + rows[i].size, 0));
  } finally {
    await server.close();
  }
});

test("archive selection styles retain usable rows and avoid browser text selection", async () => {
  const css = await readFile(new URL("../design.css", import.meta.url), "utf8");
  const rules = parseCss(css).children.filter((node) => node.type === "Rule");
  const declarations = (selector) => Object.fromEntries(rules
    .filter((rule) => rule.prelude.children.some((item) => css.slice(item.start, item.end).trim() === selector))
    .flatMap((rule) => rule.block.children.filter((node) => node.type === "Declaration")
      .map((node) => [node.property, node.value])));
  for (const selector of [".modern-row", ".classic-row"]) {
    assert.equal(declarations(selector)["user-select"], "none");
    assert.equal(declarations(selector)["-webkit-user-select"], "none");
  }
  const workspace = declarations(".archive-workspace");
  assert.equal(workspace["overflow-y"], "auto");
  assert.equal(workspace["overflow-x"], "hidden");
  assert.match(workspace["grid-template-rows"], /minmax\(var\(--archive-list-min-height\), 1fr\)/u);
  const hint = declarations(".archive-selection-hint");
  for (const property of ["padding", "color", "background", "font-size", "line-height"]) {
    assert.match(hint[property], /var\(--/u);
  }
});
