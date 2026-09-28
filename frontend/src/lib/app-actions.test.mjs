import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test, { after, before } from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

let server;
let actions;
before(async () => {
  server = await createTestServer();
  actions = await server.ssrLoadModule("/src/lib/app-actions.ts");
});
after(async () => server?.close());

const browsing = {
  blocked: false, taskWindow: false, opening: false, archive: true, writable: true,
  browsing: true, selectionBusy: false, hasSelection: true, canRename: true,
  canPreview: true, canSelectAll: true, hasParent: true, textSelection: false,
  taskCenterAvailable: true,
};

test("refresh and unfinished selection disable entry actions while navigation stays available", () => {
  const available = actions.appActionAvailability({ ...browsing, selectionBusy: true, canSelectAll: false });
  for (const action of ["extract_all", "extract_selection", "add_files", "new_folder", "rename_entry", "move_entries", "delete_entries", "copy_entries", "preview_entry", "select_all"]) {
    assert.equal(available[action], false, action);
  }
  assert.equal(available.go_up, true);
  assert.equal(available.search_archive, true);
  assert.equal(available.open_archive, true);
});

test("read-only archives retain reading actions and task windows cannot act on a main-window archive", () => {
  const available = actions.appActionAvailability({ ...browsing, writable: false });
  for (const action of ["add_files", "new_folder", "rename_entry", "move_entries", "delete_entries"]) assert.equal(available[action], false);
  for (const action of ["extract_all", "extract_selection", "test_archive", "copy_entries", "preview_entry"]) assert.equal(available[action], true);
  for (const override of [{ taskWindow: true }, { blocked: true, taskCenterAvailable: false }]) {
    const isolated = actions.appActionAvailability({ ...browsing, ...override });
    assert.deepEqual(Object.keys(isolated).filter((id) => isolated[id]), ["select_all"]);
  }
});

test("an action received after selection or modal state changes is rejected before execution", async () => {
  const definitions = JSON.parse(await readFile(new URL("./app-actions.json", import.meta.url), "utf8"));
  const available = actions.appActionAvailability(browsing);
  assert.deepEqual(Object.keys(available).sort(), definitions.map(({ id }) => id).sort());
  let submitted = 0;
  const handlers = { delete_entries: async () => { submitted += 1; } };
  assert.equal(await actions.dispatchAppAction("delete_entries", available, handlers), true);
  assert.equal(submitted, 1);
  const blocked = actions.appActionAvailability({ ...browsing, blocked: true });
  assert.equal(await actions.dispatchAppAction("delete_entries", blocked, handlers), false);
  assert.equal(await actions.dispatchAppAction("unknown_action", available, handlers), false);
  assert.equal(submitted, 1);
});

function key(key, modifiers = {}) {
  return { key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, isComposing: false, defaultPrevented: false, ...modifiers };
}

test("global shortcuts use the platform modifier and do not consume composition or modified row keys", () => {
  const shortcut = actions.appActionForShortcut;
  assert.equal(shortcut(key("e", { metaKey: true }), "macos"), "extract_all");
  assert.equal(shortcut(key("E", { ctrlKey: true, shiftKey: true }), "windows"), "extract_selection");
  assert.equal(shortcut(key("ArrowUp", { altKey: true }), "linux"), "go_up");
  assert.equal(shortcut(key("ArrowUp", { metaKey: true }), "macos"), "go_up");
  for (const modifiers of [{ ctrlKey: true }, { metaKey: true, altKey: true }, { metaKey: true, repeat: true }, { metaKey: true, isComposing: true }, { metaKey: true, defaultPrevented: true }]) {
    assert.equal(shortcut(key("e", modifiers), "macos"), null);
  }
  assert.equal(shortcut(key("Backspace", { metaKey: true }), "macos"), null);
  assert.equal(shortcut(key("F2"), "windows"), null);
});

test("checkbox focus permits archive select-all without taking text editing away from fields", () => {
  assert.equal(actions.isTextEditingTarget({ tagName: "INPUT", type: "checkbox" }), false);
  assert.equal(actions.isTextEditingTarget({ tagName: "INPUT", type: "search" }), true);
  assert.equal(actions.isTextEditingTarget({ tagName: "TEXTAREA" }), true);
  assert.equal(actions.isTextEditingTarget({ tagName: "DIV", isContentEditable: true }), true);
  const available = actions.appActionAvailability({ ...browsing, selectionBusy: true, canSelectAll: false, textSelection: true });
  assert.equal(available.select_all, true);
});

async function entryHandlers(overrides = {}) {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const names = new Set(["onEntryKeydown", "runEntryContextAction"]);
  const functions = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.has(node.name?.text));
  const { outputText } = ts.transpileModule(functions.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\n({ onEntryKeydown, runEntryContextAction })`, {
    archiveSelectionBusyReason: () => "",
    tick: () => Promise.resolve(),
    ...overrides,
  });
}

test("row handling leaves modified E and M to the shared shortcut dispatcher", async () => {
  const dispatched = [];
  const handlers = await entryHandlers({
    selectedPaths: () => new Set(["file.txt"]),
    runAppAction: async (action) => dispatched.push(action),
  });
  const row = {};
  const entry = { source: { path: "file.txt" } };
  for (const event of [key("e", { metaKey: true }), key("m", { ctrlKey: true }), key("Backspace", { metaKey: true })]) {
    handlers.onEntryKeydown({ ...event, target: row, currentTarget: row, preventDefault: () => assert.fail("modified row key was consumed") }, entry);
  }
  handlers.onEntryKeydown({ ...key("e"), target: row, currentTarget: row, preventDefault() {} }, entry);
  handlers.onEntryKeydown({ ...key("F2"), target: row, currentTarget: row, preventDefault() {} }, entry);
  await new Promise(setImmediate);
  assert.deepEqual(dispatched, ["extract_selection", "rename_entry"]);
});

test("context preview keeps the clicked entry while group operations keep their selection", async () => {
  const dispatched = [];
  const context = {
    entryContext: { path: "selected-group/clicked.txt", isDir: false },
    closeEntryContext: () => { context.entryContext = null; },
    runAppAction: async (action, target) => { dispatched.push({ action, target }); },
  };
  const handlers = await entryHandlers(context);
  await handlers.runEntryContextAction("preview");
  assert.equal(dispatched[0].action, "preview_entry");
  assert.equal(dispatched[0].target.path, "selected-group/clicked.txt");
  await handlers.runEntryContextAction("delete");
  assert.equal(dispatched[1].action, "delete_entries");
  assert.equal(dispatched[1].target, undefined);
});
