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
  blocked: false, taskWindow: false, opening: false, addingFiles: false, archive: true, writable: true,
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

test("a pending add request disables the shared add action while archive navigation stays available", async () => {
  const available = actions.appActionAvailability({ ...browsing, addingFiles: true });
  assert.equal(available.add_files, false);
  assert.equal(available.open_archive, true);
  assert.equal(available.go_up, true);
  assert.equal(await actions.dispatchAppAction("add_files", available, {
    add_files: () => assert.fail("a duplicate add action was dispatched"),
  }), false);
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

async function appHandlers(overrides = {}, names = ["onEntryKeydown", "runEntryContextAction"]) {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const functions = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const { outputText } = ts.transpileModule(functions.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  const context = {
    archiveSelectionBusyReason: () => "",
    tick: () => Promise.resolve(),
    ...overrides,
  };
  return { ...vm.runInNewContext(`${outputText}\n({ ${names.join(", ")} })`, context), context };
}

test("archive editing checks the source and registered capabilities while reading and recovery stay available", async () => {
  const state = { selected: false, preparing: false, selectionBusy: "", refresh: "", registry: "ready" };
  const archive = { read_only: true, format: "zip", path: "/Samples/nested.zip" };
  const formats = [{ id: "zip", can_update: true }, { id: "rar", can_update: false }];
  const calls = [];
  const notices = [];
  const availability = () => actions.appActionAvailability({
    ...browsing, writable: !app.archiveMutationDisabledReason(), hasSelection: state.selected,
    canRename: app.canRenameSelection(), canPreview: state.selected && !state.preparing,
    selectionBusy: Boolean(state.selectionBusy),
  });
  const app = await appHandlers({
    currentArchive: archive,
    formatRegistryStatus: () => state.registry,
    allFormats: () => formats,
    hasArchiveOpen: () => true,
    hasArchiveSelection: () => state.selected && !state.selectionBusy,
    selectedRenameSource: () => state.selected ? "inner-readme.txt" : null,
    selectedPaths: () => new Set(state.selected ? ["inner-readme.txt"] : []),
    appActionEnabled: (action) => availability()[action],
    archiveRefreshStatusLabel: () => state.refresh,
    archiveSelectionBusyReason: () => state.selectionBusy,
    selectedPreviewPolicy: () => ({ disabledReason: state.selected ? "" : "Select one entry" }),
    previewBusy: () => state.preparing,
    tr: (_key, fallback) => fallback,
    showNotice: (message) => notices.push(message),
    blockSelectionScopedAction: () => false,
    submitJob: () => assert.fail("an unsupported update was submitted"),
    runAppAction: (action) => actions.dispatchAppAction(action, availability(), {
      [action]: () => { calls.push(action); },
    }),
  }, ["classicCommandAction", "classicCommandDisabled", "classicCommandDisabledTitle", "handleClassicCommand", "previewSelectedDisabledReason",
    "archiveSourceMutationDisabledReason", "archiveMutationDisabledReason", "archiveEditingStatus", "canRenameSelection",
    "renameSelectedDisabledReason", "moveSelectedDisabledReason", "deleteSelectedDisabledReason", "openArchiveEditor", "submitDeleteSelectedJob"]);
  assert.equal(app.classicCommandDisabled("View"), true);
  assert.equal(app.classicCommandDisabledTitle("View"), "Select one entry");
  state.selected = true;
  for (const label of ["View", "Extract To", "Test", "Convert"]) {
    assert.equal(app.classicCommandDisabled(label), false, label);
    assert.equal(app.classicCommandDisabledTitle(label), "", label);
  }
  app.handleClassicCommand("View");
  app.handleClassicCommand("Extract To");
  assert.deepEqual(calls, ["preview_entry", "extract_selection"]);
  state.preparing = true;
  assert.equal(app.classicCommandDisabled("View"), true);
  assert.equal(app.classicCommandDisabledTitle("View"), "Preparing item");
  state.selectionBusy = "Refreshing selection";
  assert.equal(app.classicCommandDisabledTitle("View"), state.selectionBusy);
  state.selectionBusy = "";
  state.preparing = false;
  for (const label of ["Add", "Delete", "Rename", "Move", "New Folder"]) {
    assert.equal(app.classicCommandDisabled(label), true, label);
    assert.equal(app.classicCommandDisabledTitle(label), "Nested archives are read-only. Extract or convert to save changes.", label);
  }
  assert.equal(app.classicCommandDisabled("Protect"), true);

  archive.read_only = false;
  archive.format = "rar";
  assert.equal(app.archiveSourceMutationDisabledReason(), "");
  assert.match(app.archiveMutationDisabledReason(), /does not support editing entries/u);
  assert.equal(app.archiveEditingStatus().retryLabel, null);
  for (const label of ["Add", "Delete", "Rename", "Move", "New Folder"]) {
    assert.equal(app.classicCommandDisabled(label), true, label);
    assert.match(app.classicCommandDisabledTitle(label), /does not support editing entries/u, label);
    app.handleClassicCommand(label);
  }
  assert.deepEqual(calls, ["preview_entry", "extract_selection"]);
  await app.submitDeleteSelectedJob();
  app.openArchiveEditor("new-folder");
  assert.equal(notices.length, 2);
  assert.ok(notices.every((message) => message.includes("does not support editing entries")));
  for (const label of ["Protect", "View", "Extract To", "Test", "Convert"]) {
    assert.equal(app.classicCommandDisabled(label), false, label);
  }

  archive.format = "zip";
  archive.path = "/Samples/application.jar";
  for (const registry of ["loading", "error"]) {
    state.registry = registry;
    assert.notEqual(app.archiveMutationDisabledReason(), "");
    assert.equal(app.archiveEditingStatus().pending, registry === "loading");
    assert.equal(Boolean(app.archiveEditingStatus().retryLabel), registry === "error");
    for (const label of ["Add", "Delete", "Rename", "Move", "New Folder"]) assert.equal(app.classicCommandDisabled(label), true, label);
    for (const label of ["Protect", "Extract To", "Test", "Convert"]) assert.equal(app.classicCommandDisabled(label), false, label);
  }
  state.registry = "ready";
  for (const label of ["Add", "Delete", "Rename", "Move", "New Folder"]) assert.equal(app.classicCommandDisabled(label), false, label);
  assert.equal(app.archiveEditingStatus(), null);
  state.selected = false;
  assert.match(app.classicCommandDisabledTitle("Rename"), /Select exactly one/u);
  state.selected = true;
  state.selectionBusy = "Selecting entries";
  assert.equal(app.canRenameSelection(), false);
  state.selectionBusy = "";

  archive.format = "tar.gzip";
  assert.match(app.archiveMutationDisabledReason(), /Editing is unavailable/u);
  assert.equal(app.archiveEditingStatus().retryLabel, null);
  state.refresh = "Refreshing archive";
  assert.equal(app.archiveMutationDisabledReason(), state.refresh);
  assert.equal(app.archiveEditingStatus(), null);
  assert.equal(app.classicCommandDisabledTitle("Protect"), state.refresh);
  state.refresh = "";
  archive.read_only = true;
  state.registry = "error";
  assert.match(app.archiveMutationDisabledReason(), /^Nested archives/u);
  assert.equal(app.archiveEditingStatus().retryLabel, null);

  for (const scenario of [
    { name: "successful retry", target: "archive search" },
    { name: "failed retry", target: "new retry button", failed: true },
    { name: "user changed focus", target: null },
    { name: "browse departure and return", target: null },
    { name: "mode departure and return", target: null },
    { name: "startup load", target: null },
  ]) {
    const focused = [];
    const scope = { isConnected: true };
    const document = { body: {}, activeElement: null, querySelector: () => null };
    class Element {
      constructor(name, status = null) { this.name = name; this.status = status; }
      closest(selector) {
        assert.equal(selector, ".archive-editing-status");
        return this.status;
      }
      focus(options) {
        assert.equal(options.preventScroll, true);
        focused.push(this.name);
        document.activeElement = this;
      }
    }
    const origin = new Element("old retry button", { parentElement: scope });
    const search = new Element("archive search");
    const retryButton = new Element("new retry button");
    document.activeElement = scenario.name === "startup load" ? document.body : origin;
    let resolve;
    let reject;
    const loaded = new Promise((yes, no) => { resolve = yes; reject = no; });
    const retry = await appHandlers({
      document, HTMLElement: Element, currentArchive: { id: 17 }, archiveOpenGeneration: 0,
      screen: "browse", mode: "modern", modeSelectionBlocked: false,
      blockingModalVisible: () => false, archiveSearchInput: search, loadFormats: () => loaded,
    }, ["retryArchiveFormats"]);
    const pending = retry.retryArchiveFormats();
    document.activeElement = document.body;
    if (scenario.name === "user changed focus") {
      document.activeElement = new Element("chosen action");
    } else if (scenario.name === "browse departure and return") {
      retry.context.screen = "recovery";
      scope.isConnected = false;
      retry.context.screen = "browse";
    } else if (scenario.name === "mode departure and return") {
      retry.context.mode = "classic";
      scope.isConnected = false;
      retry.context.mode = "modern";
    }
    if (scenario.failed) {
      document.querySelector = () => retryButton;
      reject(new Error("editing check failed"));
    } else {
      resolve();
    }
    await pending;
    assert.deepEqual(focused, scenario.target ? [scenario.target] : [], scenario.name);
  }
});

test("row handling leaves modified E and M to the shared shortcut dispatcher", async () => {
  const dispatched = [];
  const handlers = await appHandlers({
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
  const handlers = await appHandlers(context);
  await handlers.runEntryContextAction("preview");
  assert.equal(dispatched[0].action, "preview_entry");
  assert.equal(dispatched[0].target.path, "selected-group/clicked.txt");
  await handlers.runEntryContextAction("delete");
  assert.equal(dispatched[1].action, "delete_entries");
  assert.equal(dispatched[1].target, undefined);
});
