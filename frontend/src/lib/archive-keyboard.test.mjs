import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { archiveKeyboardTarget } = await server.ssrLoadModule("/src/lib/archive-keyboard.ts");
const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");

function harness(names, overrides = {}) {
  const declarations = selectFunctions(source, names);
  const outputText = compileTestScript(declarations.map((node) => node.getText(source)).join("\n"));
  const context = { archiveKeyboardTarget, MODERN_ROW_HEIGHT: 42, CLASSIC_ROW_HEIGHT: 29,
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

test("row navigation consumes plain movement and Shift ranges without intercepting controls or native shortcuts", () => {
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
  const count = calls.length;
  for (const options of [{ target: {} }, { metaKey: true }, { ctrlKey: true }, { altKey: true }, { isComposing: true }]) {
    app.onEntryKeydown({ key: "ArrowDown", target: origin, currentTarget: origin, ...options,
      preventDefault: () => assert.fail("native or nested control key was consumed") }, {});
  }
  assert.equal(calls.length, count);
});

test("loaded keyboard targets preserve range intent and ignore stale requests or changed selection", async () => {
  for (const change of ["none", "selection", "focus", "archive", "navigation", "mode", "modal", "tasks", "newer", "missing"]) {
    const waiting = deferred();
    const calls = [];
    const origin = { isConnected: true, closest: () => ({ clientHeight: 336 }) };
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
    assert.deepEqual(calls, change === "none"
      ? [["load", 506], ["anchor", 498], ["select", 506, true], ["focus", 506, "item-506.txt"]]
      : [["load", 506]], change);
  }
});

test("focus restoration reveals overscan rows and rejects later navigation before focusing", async () => {
  for (const mode of ["modern", "classic"]) {
    const height = mode === "modern" ? 42 : 29;
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
