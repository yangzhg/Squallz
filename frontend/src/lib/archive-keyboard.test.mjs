import { deferred } from "../../tests/async.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer, englishTranslate } from "../../tests/runtime.mjs";
import { evaluateFunctions, readSvelteScript } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { archiveKeyboardTarget, archiveTabStopPath, focusVirtualArchiveRow, handleArchiveNavigationKey }
  = await server.ssrLoadModule("/src/lib/archive-keyboard.ts");
const { displayArchiveEntry } = await server.ssrLoadModule("/src/lib/archive-browser.ts");
const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");

function harness(names, overrides = {}) {
  const context = { archiveKeyboardTarget, focusVirtualArchiveRow, handleArchiveNavigationKey,
    MODERN_ROW_HEIGHT: 42, CLASSIC_ROW_HEIGHT: 42, archiveBrowseGeneration: () => 0, modeSelectionBlocked: false,
    archiveKeyboardRequest: 0, taskReviewRequestGeneration: 0, screen: "browse", mode: "modern",
    currentArchive: { id: 1 }, archiveDirs: [], filterText: () => "", taskCenterOpen: false,
    blockingModalVisible: () => false, archiveSelectionBusyReason: () => "", tick: async () => {},
    ...overrides };
  return { ...evaluateFunctions(source, names, context), context };
}

test("archive keyboard movement stays in bounds and pages by the visible row count", () => {
  for (const [key, index, expected] of [["ArrowUp", 0, 0], ["ArrowDown", 10, 11],
    ["Home", 500, 0], ["End", 0, 1000], ["PageUp", 5, 0], ["PageDown", 498, 506],
    ["PageDown", 997, 1000], ["ArrowDown", 1000, 1000]]) {
    assert.equal(archiveKeyboardTarget(key, index, 1001, 8.9), expected);
  }
  assert.equal(archiveKeyboardTarget("PageDown", 0, 1001, 0), 1);
  assert.equal(archiveKeyboardTarget("Enter", 1, 1001, 8), null);
  assert.equal(archiveKeyboardTarget("Home", 0, 0, 8), null);
  assert.equal(archiveKeyboardTarget("End", undefined, 1001, 8), null);
});

test("row navigation accepts movement from row controls while preserving activation and native shortcuts", () => {
  const calls = [];
  const app = harness(["onEntryKeydown"], { moveArchiveRow: (event) => calls.push([event.key, Boolean(event.shiftKey)]) });
  const origin = {};
  for (const key of ["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
    for (const shiftKey of [false, true]) {
      let prevented = false;
      app.onEntryKeydown({ key, shiftKey, target: origin, currentTarget: origin,
        preventDefault() { prevented = true; } }, {});
      assert.equal(prevented, true);
      assert.deepEqual(calls.at(-1), [key, shiftKey]);
    }
  }
  app.onEntryKeydown({ key: "ArrowDown", target: {}, currentTarget: origin, preventDefault() {} }, {});
  assert.deepEqual(calls.at(-1), ["ArrowDown", false]);
  const count = calls.length;
  for (const options of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }, { isComposing: true }]) {
    app.onEntryKeydown({ key: "ArrowDown", target: origin, currentTarget: origin, ...options,
      preventDefault: () => assert.fail("native shortcut was consumed") }, {});
  }
  assert.equal(calls.length, count);
  for (const key of [" ", "Enter"]) {
    app.onEntryKeydown({ key, target: {}, currentTarget: origin,
      preventDefault: () => assert.fail("nested control activation was consumed") }, {});
  }
});

test("Shift+Space toggles the focused entry without opening it or repeating a held selection", () => {
  const calls = [];
  const app = harness(["onEntryKeydown"], {
    toggleEntrySelection: (entry) => calls.push(["select", entry.source.path]),
    previewDisplayEntry: (entry) => calls.push(["preview", entry.source.path]),
  });
  const origin = {};
  const entry = { source: { path: "notes.txt" } };
  for (const [shiftKey, repeat] of [[true, false], [true, true], [false, false]]) {
    app.onEntryKeydown({ key: " ", shiftKey, repeat, target: origin, currentTarget: origin, preventDefault() {} }, entry);
  }
  assert.deepEqual(calls, [["select", "notes.txt"], ["preview", "notes.txt"]]);
});

test("horizontal row keys reveal a clipped filename without adding a second Tab stop", () => {
  const label = { scrollWidth: 1100, clientWidth: 420, scrollLeft: 0 };
  class Row { querySelector() { return label; } }
  const row = new Row();
  const app = harness(["onEntryKeydown"], { HTMLElement: Row });
  const key = (key, expected, consumed = true) => {
    let prevented = false;
    app.onEntryKeydown({ key, currentTarget: row, target: row,
      preventDefault() { prevented = true; } }, {});
    assert.equal(label.scrollLeft, expected);
    assert.equal(prevented, consumed);
  };
  key("ArrowRight", 420);
  key("ArrowRight", 680);
  key("ArrowRight", 680, false);
  key("ArrowLeft", 260);
  key("ArrowLeft", 0);
  key("ArrowLeft", 0, false);
  label.scrollWidth = label.clientWidth;
  key("ArrowRight", 0, false);
});

test("the row Tab stop follows focus, then selection, and remains available after virtual scrolling", () => {
  const rows = [null, { source: { path: "a.txt" }, selected: false },
    { source: { path: "b.txt" }, selected: true }, { source: { path: "c.txt" }, selected: false }];
  assert.equal(archiveTabStopPath(rows, "c.txt"), "c.txt");
  assert.equal(archiveTabStopPath(rows, null), "b.txt");
  assert.equal(archiveTabStopPath(rows.slice(3), "b.txt"), "c.txt");
  assert.equal(archiveTabStopPath([null, null], "b.txt"), null);
  assert.equal(archiveTabStopPath([], null), null);
});

function archiveBrowserView() {
  const rows = Array.from({ length: 3 }, (_, index) => ({
    source: { path: `notes-${index}.txt`, entry_type: "file" }, virtualIndex: index,
    name: `notes-${index}.txt`, location: { path: "reports", label: "reports" }, type: "file", size: "1 KB", packed: "500 B",
    ratio: "50%", modified: "2026-10-22", crc: "12345678", encoding: "UTF-8", attr: "File",
    selected: index === 1, previewing: false, previewBusy: false, selectionLabel: "Select item",
    previewActionLabel: "Open item", previewActionIcon: "eye",
  }));
  return {
    rows, startIndex: 0, totalRows: 3, rowsPending: false, paddingTop: 0, paddingBottom: 0,
    sort: { key: "size", direction: "desc" },
    selection: { checked: false, mixed: true, disabled: false, label: "Select all", busy: false, busyLabel: "" },
    selectedSummary: "1 selected", editingStatus: null, conflict: null, structureWarning: null, recovery: null,
    archive: { title: "notes.zip", format: "ZIP", summary: "3 files", dirs: [], canGoUp: false },
    actions: { enabled: { extract_all: true, extract_selection: true }, previewIcon: "eye",
      extractAllLabel: "Extract all to…", extractSelectedLabel: "Extract selected (1)…" },
    encodingWarning: null, filterText: "", filterPending: false,
    filterStatus: "3 files", emptyLabel: "No files", archiveTitle: "notes.zip", archiveFormatSummary: "ZIP",
    archiveOpen: true, openArchiveFirst: "Open an archive", emptyName: "No files", emptyStatus: "",
    preview: { nested: null, busy: false, failed: false, entry: null, canPreview: true, actionIcon: "eye" },
  };
}

test("both archive browsers expose one row Tab stop with selection and open controls outside the Tab sequence", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const view = archiveBrowserView();
  for (const name of ["ModernArchiveBrowser", "ClassicArchiveBrowser"]) {
    const { default: Browser } = await server.ssrLoadModule(`/src/components/${name}.svelte`);
    const { body } = render(Browser, { props: { view, tr: englishTranslate } });
    const renderedRows = [...body.matchAll(/<div[^>]*data-row-index="(\d+)"[^>]*>/g)];
    assert.equal(renderedRows.length, 3, name);
    assert.deepEqual(renderedRows.filter(([tag]) => /tabindex="0"/.test(tag)).map(([, index]) => Number(index)), [1], name);
    const rowGroup = body.slice(body.indexOf('data-virtual-list="'));
    const controls = [...rowGroup.matchAll(/<button[^>]*(?:row-select-toggle|row-preview-button)[^>]*>/g)];
    assert.equal(controls.length, 6, name);
    assert.ok(controls.every(([tag]) => tag.includes('tabindex="-1"')), name);
    const locations = [...rowGroup.matchAll(/<button[^>]*class="archive-entry-location"[^>]*>/g)];
    assert.equal(locations.length, 3, name);
    assert.deepEqual(locations.map(([tag]) => tag.includes('tabindex="0"')), [false, true, false], name);
    assert.ok(locations.every(([tag]) => tag.includes('aria-label="Open containing folder: reports"')), name);
    if (name === "ClassicArchiveBrowser") {
      assert.match(body, /aria-colcount="4"/);
      assert.equal([...body.matchAll(/role="columnheader"/g)].length, 4);
      assert.doesNotMatch(body, /12345678/);
    }
  }
});

test("both archive headers offer the same sortable columns and announce the active order", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const view = archiveBrowserView();
  for (const name of ["ModernArchiveBrowser", "ClassicArchiveBrowser"]) {
    const { default: Browser } = await server.ssrLoadModule(`/src/components/${name}.svelte`);
    for (const [key, direction, aria] of [["size", "desc", "descending"], ["modified", "asc", "ascending"], [null, "asc", null]]) {
      view.sort = { key, direction };
      const { body } = render(Browser, { props: { view, tr: englishTranslate } });
      const head = body.slice(body.indexOf('role="row"'), body.indexOf('role="rowgroup"'));
      assert.deepEqual([...head.matchAll(/class="archive-sort-button"[^>]*>\s*(Name|Size|Modified)/g)].map(match => match[1]),
        ["Name", "Size", "Modified"], name);
      const sorted = [...head.matchAll(/aria-sort="([^"]+)"[^>]*>[\s\S]*?class="archive-sort-button"[^>]*>\s*(\w+)/g)]
        .map(match => [match[1], match[2].toLowerCase()]);
      assert.deepEqual(sorted, aria ? [[aria, key]] : [], name);
    }
  }
});

test("modern archive extraction gives the primary action the current scope", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: Browser } = await server.ssrLoadModule("/src/components/ModernArchiveBrowser.svelte");
  const view = archiveBrowserView();
  for (const [mixed, checked] of [[false, false], [true, false], [false, true]]) {
    view.selection = { ...view.selection, mixed, checked };
    const { body } = render(Browser, { props: { view, tr: englishTranslate } });
    const actions = body.slice(body.indexOf('class="summary-actions"'), body.indexOf('class="archive-tools"'));
    const labels = [...actions.matchAll(/<button\b([^>]+)>([\s\S]*?)<\/button>/g)]
      .map(([, attributes, contents]) => [attributes.includes("primary"), contents.replace(/<[^>]*>/g, "").trim()])
      .filter(([, label]) => label.startsWith("Extract"));
    const all = view.actions.extractAllLabel;
    assert.deepEqual(labels, mixed || checked ? [[true, view.actions.extractSelectedLabel], [false, all]] : [[true, all]]);
  }
});

test("opening a search result folder leaves feedback and focus with the latest visit", async () => {
  const pending = [];
  const notices = [];
  let focused = 0;
  const app = harness(["openSearchResultFolder", "openArchiveDirectoryEntry"], {
    archiveDirs: ["$&"], filterText: () => "notes", rowAt: () => ({ path: "$&/notes.txt" }),
    blockSelectionScopedAction: () => false, clearEntryPreviewState() {}, document: { body: null, activeElement: null },
    enterDirPath: (path) => { assert.equal(path, "$&"); const visit = deferred(); pending.push(visit); return visit.promise; },
    archiveBrowseGeneration: () => pending.length, archiveBrowseError: () => null, browseScrollTop: 30,
    tr: englishTranslate, recordValidationEvent() {}, showNotice: (text) => notices.push(text),
    modeSelectionBlocked: false, totalRows: () => 1, focusArchiveRow: () => { focused++; },
  });
  const entry = { source: { path: "$&/notes.txt" }, virtualIndex: 2, location: { path: "$&", label: "$&" } };
  const first = app.openSearchResultFolder(entry);
  const second = app.openSearchResultFolder(entry);
  pending[1].resolve();
  await second;
  pending[0].resolve();
  await first;
  assert.deepEqual(notices, ["Opened folder: $&"]);
  assert.equal(focused, 1);
});

test("opening a folder from a row keeps navigation inside its contents without taking later focus", async () => {
  for (const scenario of ["row", "empty", "outside", "focus moved"]) {
    const origin = { closest: () => scenario === "outside" ? null : {} };
    const document = { body: {}, activeElement: origin };
    const listing = deferred();
    const focus = [];
    const app = harness(["openArchiveDirectoryEntry"], {
      document, archiveDirs: ["folder"], pathBaseName: () => "folder", clearEntryPreviewState() {},
      enterDirPath: () => listing.promise, archiveBrowseError: () => null, browseScrollTop: 0,
      recordValidationEvent() {}, showNotice() {}, tr: englishTranslate,
      totalRows: () => scenario === "empty" ? 0 : 2, focusArchiveRow: (index) => focus.push(index),
      archiveSearchInput: { focus: () => focus.push("search") },
    });
    const open = app.openArchiveDirectoryEntry("folder/");
    document.activeElement = scenario === "focus moved" ? {} : document.body;
    listing.resolve();
    await open;
    assert.deepEqual(focus, scenario === "row" ? [0] : scenario === "empty" ? ["search"] : [], scenario);
  }
});

test("Escape clears archive search only after input method editing finishes", () => {
  const calls = [];
  const app = harness(["onArchiveFilterKeydown"], { filterText: () => "报告", clearArchiveFilter: () => calls.push("clear") });
  const escape = { key: "Escape", preventDefault: () => calls.push("prevent"), stopPropagation: () => calls.push("stop") };
  for (const state of [{ isComposing: true }, { keyCode: 229 }, { defaultPrevented: true }]) {
    app.onArchiveFilterKeydown({ ...escape, ...state });
  }
  assert.deepEqual(calls, []);
  app.onArchiveFilterKeydown(escape);
  assert.deepEqual(calls, ["prevent", "stop", "clear"]);
});

test("Down from a completed search selects the first match and hands keyboard navigation to its row", () => {
  const origin = {};
  const document = { activeElement: origin };
  const first = { path: "reports/summary.txt", display: "summary.txt", entry_type: "file", encoding: "utf-8", size: 20 };
  const calls = [];
  let canFocus;
  const app = harness(["onArchiveFilterKeydown"], {
    document, filterText: () => "summary", filterPending: () => false, archiveBrowseError: () => null,
    rowAt: (index) => index === 0 ? first : null, displayArchiveEntry, tr: englishTranslate,
    selectOnlyEntry: (entry) => calls.push(["select", entry.virtualIndex, entry.source.path]),
    focusArchiveRow(index, path, guard) { calls.push(["focus", index, path]); canFocus = guard; },
  });
  const event = { key: "ArrowDown", currentTarget: origin,
    preventDefault: () => calls.push("prevent"), stopPropagation: () => calls.push("stop") };
  for (const flags of [{ isComposing: true }, { keyCode: 229 }, { defaultPrevented: true },
    { shiftKey: true }, { ctrlKey: true }, { altKey: true }, { metaKey: true }]) {
    app.onArchiveFilterKeydown({ ...event, ...flags });
  }
  for (const overrides of [{ filterText: () => " " }, { filterPending: () => true },
    { rowAt: () => null }, { archiveBrowseError: () => ({}) }, { archiveSelectionBusyReason: () => "Refreshing" }]) {
    const saved = Object.fromEntries(Object.keys(overrides).map((key) => [key, app.context[key]]));
    Object.assign(app.context, overrides);
    app.onArchiveFilterKeydown(event);
    Object.assign(app.context, saved);
  }
  assert.deepEqual(calls, []);
  app.onArchiveFilterKeydown(event);
  assert.deepEqual(calls, ["prevent", "stop", ["select", 0, first.path], ["focus", 0, first.path]]);
  assert.equal(canFocus(), true);
  document.activeElement = {};
  assert.equal(canFocus(), false);
});

test("loaded keyboard targets preserve range intent and ignore stale requests or changed selection", async () => {
  for (const change of ["none", "descendant", "selection", "focus", "archive", "sort", "navigation", "mode", "modal", "tasks", "newer", "missing"]) {
    const waiting = deferred();
    const calls = [];
    const descendant = {};
    const origin = { isConnected: true, closest: () => ({ clientHeight: 336 }),
      contains: (target) => target === origin || target === descendant };
    const document = { activeElement: origin, body: {} };
    let selection = new Set();
    const app = harness(["moveArchiveRow"], { document, totalRows: () => 1001,
      selectedPaths: () => selection,
      selectRow: (row, index) => { calls.push(["anchor", index]); selection = new Set([row.path]); },
      loadRowAt: (index) => { calls.push(["load", index]); return waiting.promise; },
      displayArchiveEntry, tr: englishTranslate,
      selectEntry: async (entry, event) => {
        calls.push(["select", entry.virtualIndex, event.shiftKey]);
        selection = new Set([...selection, entry.source.path]);
      },
      focusArchiveRow: async (index, path, isCurrent) => { if (isCurrent()) calls.push(["focus", index, path]); },
    });
    const pending = app.moveArchiveRow({ key: "PageDown", shiftKey: true, currentTarget: origin },
      { virtualIndex: 498, source: { path: "item-498.txt" } });
    if (change === "descendant") document.activeElement = descendant;
    if (change === "selection") selection = new Set(["other.txt"]);
    if (change === "focus") document.activeElement = {};
    if (change === "archive") app.context.currentArchive = { id: 2 };
    if (change === "sort") app.context.archiveBrowseGeneration = () => 1;
    if (change === "navigation") app.context.taskReviewRequestGeneration += 1;
    if (change === "mode") app.context.mode = "classic";
    if (change === "modal") app.context.blockingModalVisible = () => true;
    if (change === "tasks") app.context.taskCenterOpen = true;
    if (change === "newer") app.context.archiveKeyboardRequest += 1;
    waiting.resolve(change === "missing" ? null : {
      path: "item-506.txt", display: "item-506.txt", entry_type: "file", encoding: "utf-8", size: 10,
    });
    await pending;
    assert.deepEqual(calls, ["none", "descendant"].includes(change)
      ? [["load", 506], ["anchor", 498], ["select", 506, true], ["focus", 506, "item-506.txt"]]
      : [["load", 506]], change);
  }
});

test("focus restoration reveals overscan rows and rejects later navigation before focusing", async (t) => {
  t.after(() => { delete globalThis.document; });
  for (const mode of ["modern", "classic"]) {
    const height = 42;
    for (const [index, scrollTop, expected] of [[12, 0, 3 * height], [4, 10 * height, 4 * height], [5, 0, 0]]) {
      const calls = [];
      const list = { clientHeight: 10 * height, scrollTop,
        querySelector: selector => ({ focus: () => calls.push(selector) }) };
      globalThis.document = { querySelector(selector) {
        assert.equal(selector, `[data-virtual-list="${mode}"]`);
        return list;
      } };
      const app = harness(["focusArchiveRow"], { mode, browseScrollTop: scrollTop, browseViewportHeight: 10 * height,
        loadRowAt: async () => ({ path: "target.txt" }) });
      await app.focusArchiveRow(index, "target.txt");
      assert.equal(list.scrollTop, expected);
      assert.deepEqual(calls, [`[data-row-index="${index}"]`]);
      calls.length = 0;
      await app.focusArchiveRow(index, "target.txt", () => false);
      assert.deepEqual(calls, []);
      const superseded = app.focusArchiveRow(index, "target.txt");
      app.context.currentArchive = { id: 2 };
      await superseded;
      const loading = deferred();
      app.context.loadRowAt = async () => { loading.resolve(); return { path: "target.txt" }; };
      const sorted = app.focusArchiveRow(index, "target.txt");
      await loading.promise;
      app.context.archiveBrowseGeneration = () => 1;
      await sorted;
      assert.deepEqual(calls, []);
    }
  }
});
