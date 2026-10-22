import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { parse } from "svelte/compiler";
import { compileTestScript, readSvelteScript, selectFunctions, selectMountCallbacks } from "../../tests/source.mjs";

const appUrl = new URL("../App.svelte", import.meta.url);
const app = readSvelteScript(appUrl, "App.ts");

function keyboardEvent(key = "Escape") {
  return {
    key, defaultPrevented: false, stopped: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.stopped = true; },
  };
}

function menuHarness() {
  const [mount] = selectMountCallbacks(app, ["closeQuickActions"]);
  const functions = selectFunctions(app, ["closeQuickActions", "closeEntryContext"]);
  const listeners = {};
  const restored = [];
  class Element {
    contains(target) { return target === this; }
  }
  const context = {
    Node: Element, modeSelectionBlocked: false, activePopover: null, entryContext: null,
    quickActionPopover: new Element(), quickActionButton: new Element(),
    entryContextMenu: new Element(), entryContextReturnFocus: new Element(),
    nativeMenuConnection: null, activePlatform: "macos", workflowCalls: 0, shortcutCalls: 0,
    archiveEditorVisible: () => false,
    appActionForShortcut: () => { context.shortcutCalls += 1; return null; },
    handleWorkflowEscape: () => { context.workflowCalls += 1; return true; },
    restoreModalFocus: (...args) => restored.push(args),
    document: { addEventListener: (name, callback) => { listeners[name] = callback; }, removeEventListener() {} },
  };
  vm.createContext(context);
  vm.runInContext(compileTestScript(functions.map((node) => node.getText(app)).join("\n")
    + `\nconst mount = ${mount.getText(app)}; mount();`), context);
  return { context, listeners, restored, Element };
}

test("App Escape closes only the foremost menu before any workspace action", () => {
  for (const menu of ["quick", "entry"]) {
    const run = menuHarness();
    if (menu === "quick") run.context.activePopover = "quickActions";
    else run.context.entryContext = { name: "archive.txt" };
    const event = keyboardEvent();
    run.listeners.keydown(event);
    assert.equal(event.defaultPrevented, true);
    assert.equal(run.context.workflowCalls, 0, "closing a menu cannot also clear the archive filter");
    assert.equal(menu === "quick" ? run.context.activePopover : run.context.entryContext, null);
    assert.equal(run.restored.length, 1);
    run.listeners.keydown(keyboardEvent());
    assert.equal(run.context.workflowCalls, 1, "the next Escape may reach the workspace");
  }
});

test("App ignores a key already consumed by the focused overlay", () => {
  const run = menuHarness();
  run.context.activePopover = "quickActions";
  const event = keyboardEvent();
  event.preventDefault();
  run.listeners.keydown(event);
  assert.equal(run.context.activePopover, "quickActions");
  assert.equal(run.context.shortcutCalls, 0);
  assert.equal(run.context.workflowCalls, 0);
  assert.equal(run.restored.length, 0);
});

test("outside pointer dismissal leaves focus to the clicked control", () => {
  for (const menu of ["quick", "entry"]) {
    const run = menuHarness();
    if (menu === "quick") run.context.activePopover = "quickActions";
    else run.context.entryContext = { name: "archive.txt" };
    const inside = menu === "quick" ? run.context.quickActionPopover : run.context.entryContextMenu;
    run.listeners.pointerdown({ target: inside });
    assert.notEqual(menu === "quick" ? run.context.activePopover : run.context.entryContext, null);
    run.listeners.pointerdown({ target: new run.Element() });
    assert.equal(menu === "quick" ? run.context.activePopover : run.context.entryContext, null);
    assert.equal(run.restored.length, 0);
  }
});

function nativeDialogHarness(dialog = {}) {
  const functions = selectFunctions(app, ["withNativeDialog", "openNativeDialog", "saveNativeDialog", "getDialogModule", "blockingModalVisible"]);
  const questionEffect = app.statements.find((node) => ts.isExpressionStatement(node)
    && node.getText(app).startsWith("$effect(") && node.getText(app).includes("const questionTaskId"));
  assert.ok(questionEffect);
  const context = {
    openDialogModulePromise: null,
    require: (name) => { assert.equal(name, "@tauri-apps/plugin-dialog"); return dialog; },
    nativeDialogCount: 0, taskWindowMode: false, macosSfxPublisherTask: null, editorVisible: false,
    archiveEditorVisible: () => context.editorVisible,
    taskDialogVisible: () => context.taskDialogTaskId !== null,
    taskDialogTaskId: null, taskDialogDismissedId: null,
    jobPasswordPrompt: { id: 21 }, jobConflictPrompt: null, rememberedFocus: 0,
    rememberTaskWorkspaceFocus: () => { context.rememberedFocus += 1; },
    recordNativeDialogRequest() {}, $effect: (callback) => callback(),
  };
  vm.createContext(context);
  const api = vm.runInContext(compileTestScript(functions.map((node) => node.getText(app)).join("\n"))
    + "\n({ openNativeDialog, saveNativeDialog, getDialogModule, blockingModalVisible })", context);
  return { api, context, showQuestion: () => vm.runInContext(compileTestScript(questionEffect.getText(app)), context) };
}

test("native open and save dialogs defer task questions until every dialog settles", async () => {
  const run = nativeDialogHarness();
  let finishOpen;
  let finishSave;
  const open = run.api.openNativeDialog("open", () => new Promise((resolve) => { finishOpen = resolve; }), {});
  const save = run.api.saveNativeDialog("save", () => new Promise((resolve) => { finishSave = resolve; }), {});
  assert.equal(run.context.nativeDialogCount, 2);
  assert.equal(run.api.blockingModalVisible(), true);
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, null);
  finishOpen(null);
  await open;
  assert.equal(run.context.nativeDialogCount, 1);
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, null);
  finishSave("output.sqz");
  assert.equal(await save, "output.sqz");
  assert.equal(run.context.nativeDialogCount, 0);
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, 21);
  assert.equal(run.context.rememberedFocus, 1);
});

test("native dialog failure releases its blocker and other modal forms defer questions", async () => {
  const run = nativeDialogHarness();
  const failure = new Error("native dialog unavailable");
  await assert.rejects(run.api.openNativeDialog("open", () => { throw failure; }, {}), (error) => error === failure);
  assert.equal(run.context.nativeDialogCount, 0);
  assert.equal(run.api.blockingModalVisible(), false);
  run.context.editorVisible = true;
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, null);
  run.context.editorVisible = false;
  run.context.macosSfxPublisherTask = { id: 22 };
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, null);
  run.context.macosSfxPublisherTask = null;
  run.showQuestion();
  assert.equal(run.context.taskDialogTaskId, 21);
});

test("shared overwrite confirmation keeps task questions deferred across creation, conversion and late conflicts", async () => {
  const source = ts.createSourceFile("archive-output-authorization.ts",
    readFileSync(new URL("./archive-output-authorization.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
  const [declaration] = selectFunctions(source, ["authorizeArchiveOutput"]);
  const authorize = vm.runInNewContext(compileTestScript(declaration.getText(source))
    + "\nauthorizeArchiveOutput", { exports: {} });
  for (const message of ["gui.create.replace_existing", "gui.output.replace_existing", "gui.create.replace_changed", "gui.convert.replace_changed"]) {
    for (const outcome of ["approved", "cancelled", "failed"]) {
      let finish;
      let entered;
      const started = new Promise((resolve) => { entered = resolve; });
      const failure = new Error("Confirmation unavailable");
      const original = {
        open() {}, save() {},
        confirm: (_body, options) => {
          assert.equal(options.title, `${message}.title`);
          entered();
          return new Promise((resolve, reject) => { finish = { resolve, reject }; });
        },
      };
      const run = nativeDialogHarness(original);
      const dialogs = await run.api.getDialogModule();
      assert.equal(await run.api.getDialogModule(), dialogs, "all consumers share one wrapped module");
      assert.equal(dialogs.open, original.open);
      assert.equal(dialogs.save, original.save);
      const authorization = authorize({
        path: "/output/archive.zip", inspect: async () => ({ conflict: true, guard: "observed-output" }),
        isCurrent: () => true, confirm: async () => dialogs.confirm, tr: (key) => key, message,
      });
      await started;
      assert.equal(run.context.nativeDialogCount, 1, "the shared confirmation registers exactly one native dialog");
      assert.equal(run.api.blockingModalVisible(), true);
      run.showQuestion();
      assert.equal(run.context.taskDialogTaskId, null);
      if (outcome === "failed") {
        finish.reject(failure);
        await assert.rejects(authorization, (error) => error === failure);
      } else {
        finish.resolve(outcome === "approved");
        const result = await authorization;
        if (outcome === "approved") {
          assert.equal(result.replaceExisting, true);
          assert.equal(result.replacementGuard, "observed-output");
        } else assert.equal(result, null);
      }
      assert.equal(run.context.nativeDialogCount, 0);
      run.showQuestion();
      assert.equal(run.context.taskDialogTaskId, 21, "settled confirmation releases the pending task question");
    }
  }
});

test("task Escape consumes the event without cancelling active work, and dismisses completed results", () => {
  const source = readSvelteScript(new URL("../components/TaskProgressDialog.svelte", import.meta.url), "TaskProgressDialog.ts");
  const [handler] = selectFunctions(source, ["onTaskCardKeydown"]);
  const modelText = readFileSync(new URL("./task-model.ts", import.meta.url), "utf8");
  const model = ts.createSourceFile("task-model.ts", modelText, ts.ScriptTarget.Latest, true);
  const [activeState] = selectFunctions(model, ["isTaskActiveState"]);
  for (const presentation of ["dialog", "window", "panel"]) {
    for (const state of ["submitting", "queued", "running", "paused", "done", "failed", "cancelled"]) {
      let dismissed = 0;
      let cancelled = 0;
      const context = vm.createContext({
        presentation, task: { id: 21, state }, taskCard: {}, exports: {},
        onDismiss: () => { dismissed += 1; }, onCancel: () => { cancelled += 1; }, trapModalFocus() {},
      });
      const onKeydown = vm.runInContext(compileTestScript(activeState.getText(model) + "\n" + handler.getText(source))
        + "\nonTaskCardKeydown", context);
      const event = keyboardEvent();
      onKeydown(event);
      assert.equal(event.defaultPrevented, true);
      assert.equal(event.stopped, true);
      assert.equal(cancelled, 0);
      assert.equal(dismissed, presentation === "panel" || ["done", "failed", "cancelled"].includes(state) ? 1 : 0);
      onKeydown(event);
      assert.equal(dismissed, presentation === "panel" || ["done", "failed", "cancelled"].includes(state) ? 1 : 0, "consumed Escape is not handled twice");
    }
  }
});

function fallbackBackExpression() {
  const component = readFileSync(appUrl, "utf8");
  let expression;
  function visit(node) {
    if (!node || typeof node !== "object") return;
    if (node.type === "Component" && node.name === "TaskProgressDialogHost") {
      expression = node.attributes.find((attribute) => attribute.name === "onUnavailableBack")?.value.expression;
    }
    for (const value of Object.values(node)) {
      if (Array.isArray(value)) value.forEach(visit);
      else if (value && typeof value === "object") visit(value);
    }
  }
  visit(parse(component, { modern: true }).fragment);
  assert.ok(expression);
  return component.slice(expression.start, expression.end);
}

test("unavailable task view exits through task navigation while preserving the pending job", () => {
  const host = readSvelteScript(new URL("../components/TaskProgressDialogHost.svelte", import.meta.url), "TaskProgressDialogHost.ts");
  const [handler] = selectFunctions(host, ["onFallbackKeydown"]);
  const navigation = selectFunctions(app, ["returnToTaskCenter", "returnTaskQuestionToCenter"]);
  const back = fallbackBackExpression();
  for (const presentation of ["dialog", "panel", "window"]) {
    const task = { id: 21, state: "running", interaction: "password" };
    const context = vm.createContext({
      task, surface: { task, onDismiss: () => { throw new Error("fallback must not dismiss or cancel its task"); } },
      presentation, taskWindowMode: presentation === "window", fallbackCard: {},
      taskDialogTaskId: task.id, taskDialogDismissedId: null,
      taskCenterOpen: presentation === "panel", taskCenterSelectedTaskId: task.id, taskCenterFocusTaskId: null,
      tools: { leave() {} }, cancelTaskReview() {}, trapModalFocus() {},
    });
    const onKeydown = vm.runInContext(compileTestScript(navigation.map((node) => node.getText(app)).join("\n")
      + `\nconst onUnavailableBack = ${back};\n${handler.getText(host)}`)
      + "\nonFallbackKeydown", context);
    const event = keyboardEvent();
    onKeydown(event);
    assert.equal(event.defaultPrevented, true);
    assert.equal(event.stopped, true);
    assert.deepEqual(task, { id: 21, state: "running", interaction: "password" });
    if (presentation === "dialog") {
      assert.equal(context.taskDialogTaskId, null);
      assert.equal(context.taskDialogDismissedId, task.id);
      assert.equal(context.taskCenterOpen, true);
      assert.equal(context.taskCenterSelectedTaskId, task.id);
    } else if (presentation === "panel") {
      assert.equal(context.taskCenterOpen, true);
      assert.equal(context.taskCenterSelectedTaskId, null);
    } else assert.equal(context.taskDialogTaskId, task.id);
  }
});
