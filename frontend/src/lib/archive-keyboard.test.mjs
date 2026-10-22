import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { createTestServer, englishTranslate } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { archiveKeyboardTarget, archiveTabStopPath } = await server.ssrLoadModule("/src/lib/archive-keyboard.ts");
const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");

function harness(names, overrides = {}) {
  const declarations = selectFunctions(source, names);
  const outputText = compileTestScript(declarations.map((node) => node.getText(source)).join("\n"));
  const context = { archiveKeyboardTarget, MODERN_ROW_HEIGHT: 42, CLASSIC_ROW_HEIGHT: 42,
    archiveKeyboardRequest: 0, taskReviewRequestGeneration: 0, screen: "browse", mode: "modern",
    currentArchive: { id: 1 }, archiveDirs: [], filterText: () => "", taskCenterOpen: false,
    blockingModalVisible: () => false, archiveSelectionBusyReason: () => "", tick: async () => {},
    ...overrides };
  return { ...vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context), context };
}

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
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

test("both archive browsers expose one row Tab stop with selection and open controls outside the Tab sequence", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const rows = Array.from({ length: 3 }, (_, index) => ({
    source: { path: `notes-${index}.txt`, entry_type: "file" }, virtualIndex: index,
    name: `notes-${index}.txt`, location: "reports", type: "file", size: "1 KB", packed: "500 B",
    ratio: "50%", modified: "2026-10-22", crc: "12345678", encoding: "UTF-8", attr: "File",
    selected: index === 1, previewing: false, previewBusy: false, selectionLabel: "Select item",
    previewActionLabel: "Open item", previewActionIcon: "eye",
  }));
  const view = {
    rows, startIndex: 0, totalRows: 3, rowsPending: false, paddingTop: 0, paddingBottom: 0,
    selection: { checked: false, mixed: true, disabled: false, label: "Select all", busy: false, busyLabel: "" },
    selectedSummary: "1 selected", editingStatus: null, conflict: null, structureWarning: null, recovery: null,
    archive: { title: "notes.zip", format: "ZIP", summary: "3 files", dirs: [], canGoUp: false },
    actions: { enabled: {}, previewIcon: "eye" }, encodingWarning: null, filterText: "", filterPending: false,
    filterStatus: "3 files", emptyLabel: "No files", archiveTitle: "notes.zip", archiveFormatSummary: "ZIP",
    archiveOpen: true, openArchiveFirst: "Open an archive", emptyName: "No files", emptyStatus: "",
    preview: { nested: null, busy: false, failed: false, entry: null, canPreview: true, actionIcon: "eye" },
  };
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
    if (name === "ClassicArchiveBrowser") {
      assert.match(body, /aria-colcount="4"/);
      assert.equal([...body.matchAll(/role="columnheader"/g)].length, 4);
      assert.doesNotMatch(body, /12345678/);
    }
  }
});

test("loaded keyboard targets preserve range intent and ignore stale requests or changed selection", async () => {
  for (const change of ["none", "descendant", "selection", "focus", "archive", "navigation", "mode", "modal", "tasks", "newer", "missing"]) {
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
      toDisplayEntry: row => ({ source: row }),
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
    if (change === "navigation") app.context.taskReviewRequestGeneration += 1;
    if (change === "mode") app.context.mode = "classic";
    if (change === "modal") app.context.blockingModalVisible = () => true;
    if (change === "tasks") app.context.taskCenterOpen = true;
    if (change === "newer") app.context.archiveKeyboardRequest += 1;
    waiting.resolve(change === "missing" ? null : { path: "item-506.txt" });
    await pending;
    assert.deepEqual(calls, ["none", "descendant"].includes(change)
      ? [["load", 506], ["anchor", 498], ["select", 506, true], ["focus", 506, "item-506.txt"]]
      : [["load", 506]], change);
  }
});

test("focus restoration reveals overscan rows and rejects later navigation before focusing", async () => {
  for (const mode of ["modern", "classic"]) {
    const height = 42;
    for (const [index, scrollTop, expected] of [[12, 0, 3 * height], [4, 10 * height, 4 * height], [5, 0, 0]]) {
      const calls = [];
      const list = { clientHeight: 10 * height, scrollTop,
        querySelector: selector => ({ focus: () => calls.push(selector) }) };
      const app = harness(["focusArchiveRow"], { mode, document: { querySelector: () => list },
        loadRowAt: async () => ({ path: "target.txt" }), rowAt: () => ({ path: "target.txt" }) });
      await app.focusArchiveRow(index, "target.txt");
      assert.equal(list.scrollTop, expected);
      assert.deepEqual(calls, [`[data-row-index="${index}"]`]);
      calls.length = 0;
      await app.focusArchiveRow(index, "target.txt", () => false);
      assert.deepEqual(calls, []);
      app.context.tick = async () => { app.context.currentArchive = { id: 2 }; };
      await app.focusArchiveRow(index, "target.txt");
      assert.deepEqual(calls, []);
    }
  }
});
