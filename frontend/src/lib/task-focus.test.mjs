import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

function harness() {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const actions = ["openTaskCenter", "closeTaskCenter", "dismissTaskDialog", "returnTaskQuestionToCenter", "handleWorkflowEscape"];
  const names = [...actions, "rememberTaskWorkspaceFocus", "restoreTaskWorkspaceFocus"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const questionEffect = source.statements.find((node) => ts.isExpressionStatement(node)
    && node.getText(source).startsWith("$effect(") && node.getText(source).includes("const questionTaskId"));
  assert.ok(questionEffect);
  const compile = (code) => ts.transpileModule(code, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const pendingTicks = [];
  const document = { body: {}, activeElement: null, querySelector: () => trigger };
  class Element {
    isConnected = true;
    disabled = false;
    inert = false;
    taskSurface = false;
    closest(selector) {
      return (selector === "[inert]" ? this.inert : this.taskSurface) ? this : null;
    }
    matches() { return this.disabled; }
    focus() { if (this.isConnected && !this.disabled && !this.inert) document.activeElement = this; }
  }
  const input = new Element();
  const trigger = new Element();
  const createButton = new Element();
  const modal = new Element();
  modal.taskSurface = true;
  document.activeElement = input;
  const context = {
    document, HTMLElement: Element, screen: "browse", taskWindowMode: false, modeSelectionBlocked: false,
    taskCenterReturnFocus: null, taskCenterOpen: false, taskCenterSelectedTaskId: null, taskCenterFocusTaskId: null,
    taskDialogTaskId: null, taskDialogDismissedId: null, taskReviewRequestGeneration: 0, activePopover: null,
    jobPasswordPrompt: null, jobConflictPrompt: { id: 7, version: 1 }, otherModal: false,
    tick: () => new Promise((resolve) => pendingTicks.push(resolve)),
    appActionEnabled: () => true, createPrimaryAction: () => createButton,
    isTaskActiveState: (state) => ["running", "queued", "paused"].includes(state),
    closeNativeTaskWindow: async () => true,
    taskDialogVisible: () => context.taskDialogTaskId !== null && context.taskDialogDismissedId !== context.taskDialogTaskId,
    blockingModalVisible: () => context.otherModal || context.taskDialogVisible(),
    $effect: (callback) => callback(),
    previewBusy: () => false, nestedPreview: null, entryPreview: null, entryPreviewFailure: null,
    search: "reports", filterText: () => context.search, clearArchiveFilter: () => { context.search = ""; },
    passwordCancelled: false, cancelPasswordRequest: () => { context.passwordCancelled = true; },
  };
  vm.createContext(context);
  const api = vm.runInContext(compile(declarations.map((node) => node.getText(source)).join("\n"))
    + `\n({${actions.join(",")}})`, context);
  return {
    api, context, input, trigger, createButton, modal,
    showQuestion: () => vm.runInContext(compile(questionEffect.getText(source)), context),
    flush: async () => { for (const resolve of pendingTicks.splice(0)) resolve(); await Promise.resolve(); },
  };
}

test("closing an interrupted task restores the workspace control through both exit paths", async () => {
  for (const exit of ["dialog", "center"]) {
    const run = harness();
    run.showQuestion();
    run.modal.focus();
    run.context.jobConflictPrompt = { id: 8, version: 2 };
    run.showQuestion();
    run.context.jobConflictPrompt = null;
    if (exit === "dialog") await run.api.dismissTaskDialog({ id: 8, state: "cancelled" });
    else {
      run.api.returnTaskQuestionToCenter(8);
      run.api.closeTaskCenter();
    }
    await run.flush();
    assert.equal(run.context.document.activeElement, run.input, "consecutive questions retain the original workspace focus");
    assert.equal(run.context.taskCenterReturnFocus, null);
    assert.equal(run.context.screen, "browse");
  }
});

test("a question interrupting the task center preserves its original return target", async () => {
  for (const focus of ["center", "workspace"]) {
    const run = harness();
    run.api.openTaskCenter(run.trigger);
    (focus === "center" ? run.modal : run.input).focus();
    run.showQuestion();
    run.modal.focus();
    run.context.jobConflictPrompt = null;
    await run.api.dismissTaskDialog({ id: 7, state: "done" });
    await run.flush();
    assert.equal(run.context.taskCenterOpen, true);
    assert.equal(run.context.document.activeElement, run.modal, "dialog dismissal leaves task-center focus to its mounted view");
    run.api.closeTaskCenter();
    await run.flush();
    assert.equal(run.context.document.activeElement, focus === "center" ? run.trigger : run.input);
  }
});

test("closing a completed question also closes its selected task details", async () => {
  for (const state of ["done", "failed", "cancelled"]) {
    const run = harness();
    run.api.openTaskCenter(run.trigger);
    run.context.taskCenterSelectedTaskId = 7;
    run.modal.focus();
    run.showQuestion();
    run.context.jobConflictPrompt = null;
    await run.api.dismissTaskDialog({ id: 7, state });
    assert.equal(run.context.taskDialogVisible(), false, "the close button must dismiss the foreground dialog");
    assert.equal(run.context.taskCenterOpen, false);
    await run.flush();
    assert.equal(run.context.document.activeElement, run.trigger);
  }
});

test("deferred focus restoration does not steal focus from newer navigation or task views", async () => {
  for (const change of ["screen", "center", "dialog", "first-run"]) {
    const run = harness();
    run.api.openTaskCenter(run.trigger);
    run.api.closeTaskCenter();
    run.modal.focus();
    if (change === "screen") run.context.screen = "extract";
    if (change === "center") run.context.taskCenterOpen = true;
    if (change === "dialog") run.context.otherModal = true;
    if (change === "first-run") run.context.modeSelectionBlocked = true;
    await run.flush();
    assert.equal(run.context.document.activeElement, run.modal);
  }
});

test("unavailable workspace controls fall back to a visible primary action", async () => {
  for (const screen of ["browse", "create"]) {
    for (const unavailable of ["removed", "disabled", "inert"]) {
      const run = harness();
      run.context.screen = screen;
      run.api.openTaskCenter(run.input);
      if (unavailable === "removed") run.input.isConnected = false;
      if (unavailable === "disabled") run.input.disabled = true;
      if (unavailable === "inert") run.input.inert = true;
      run.modal.focus();
      run.api.closeTaskCenter();
      await run.flush();
      assert.equal(run.context.document.activeElement, screen === "create" ? run.createButton : run.trigger);
    }
  }
});

test("Escape in task views cannot clear the workspace search or cancel its password request", () => {
  for (const screen of ["browse", "password"]) {
    for (const foreground of ["question", "center"]) {
      const run = harness();
      run.context.screen = screen;
      if (foreground === "question") run.showQuestion();
      else run.api.openTaskCenter(run.trigger);
      run.modal.focus();
      assert.equal(run.api.handleWorkflowEscape(), false);
      assert.equal(run.context.search, "reports");
      assert.equal(run.context.passwordCancelled, false);
    }
    const run = harness();
    run.context.screen = screen;
    assert.equal(run.api.handleWorkflowEscape(), true, "the workspace still handles Escape when it owns focus");
    assert.equal(run.context.search, screen === "browse" ? "" : "reports");
    assert.equal(run.context.passwordCancelled, screen === "password");
  }
});
