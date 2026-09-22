import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parseCss } from "svelte/compiler";
import ts from "typescript";

import { createTestServer } from "../../tests/runtime.mjs";

function taskSurface(taskWindowMode) {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const declaration = source.statements.find((node) =>
    ts.isFunctionDeclaration(node) && node.name?.text === "taskDialogSurface");
  assert.ok(declaration);
  const { outputText } = ts.transpileModule(declaration.getText(source), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  const callbacks = Object.fromEntries([
    "taskOutputPath", "taskRevealOutputLabel", "pauseCurrentTask", "resumeCurrentTask",
    "cancelCurrentTask", "copyTaskChecksumResults", "openTaskOutput", "openMacosSfxPublisher",
    "reviewFailedTask", "toggleTaskDetails", "viewTaskResults", "revealTaskOutput",
    "dismissTaskDialog", "submitPasswordRequest", "cancelPasswordRequest", "answerConflictDecision",
  ].map((name) => [name, () => {}]));
  const surface = vm.runInNewContext(`${outputText}\ntaskDialogSurface`, {
    ...callbacks,
    taskWindowMode,
    activePlatform: "macos",
    activePalette: "ocean",
    activeTheme: "dark",
    activeDensityChoice: "comfortable",
    customPaletteVariables: () => ({}),
    taskChecksumCopyFeedback: () => null,
    taskChecksumCopyFeedbackTone: () => null,
    taskPasswordQuestion: () => null,
    taskConflictQuestion: () => null,
    jobPasswordValue: "",
    passwordSubmissionError: null,
    conflictApplyAll: false,
  })({ id: 42 });
  return { surface, callbacks };
}

test("task surfaces choose window or dialog presentation without changing task actions", () => {
  for (const taskWindowMode of [true, false]) {
    const { surface, callbacks } = taskSurface(taskWindowMode);
    assert.equal(surface.presentation, taskWindowMode ? "window" : "dialog");
    assert.ok(surface.rootClass.split(" ").includes(
      taskWindowMode ? "task-window-surface" : "task-modal-overlay",
    ));
    assert.match(surface.rootClass, /platform-macos palette-ocean theme-dark/u);
    assert.equal(surface.onPause, callbacks.pauseCurrentTask);
    assert.equal(surface.onCancel, callbacks.cancelCurrentTask);
    assert.equal(surface.onOpenOutput, callbacks.openTaskOutput);
    assert.equal(surface.onDismiss, callbacks.dismissTaskDialog);
  }
});

function extractTask(state) {
  return {
    id: 42, title: "Extract reports.zip", state,
    spec: { kind: "extract", path: "/Archives/reports.zip", dest: "/Exports/reports" },
    done: 40, total: 100, current: "report.txt", currentDone: 4, currentTotal: 10,
    scanEntries: null, speed: 10, phase: null, interruptible: true, pausable: true,
    error: state === "failed" ? { key: "error.io", params: {}, detail: "Read failed" } : null,
    result: state === "done" ? { counts: { created: 1, skipped: 0, failed: 0 }, problems: [] } : null,
    revealPath: state === "done" ? "/Exports/reports" : null,
    interaction: null, ownedByRequester: true, controlIntent: null, expanded: false,
  };
}

test("window task views render one task heading and retain progress, results and input", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule(
      "/src/components/TaskProgressDialog.svelte",
    );
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    await loadLocale("en-US");
    const { surface } = taskSurface(true);
    for (const state of ["running", "paused", "done", "failed"]) {
      const { body } = render(TaskProgressDialog, { props: {
        ...surface, presentation: "window", task: extractTask(state),
      } });
      assert.match(body, /data-task-presentation="window"/u);
      assert.match(body, /role="region"/u);
      assert.doesNotMatch(body, /aria-modal|task-modal-eyebrow/u);
      assert.equal((body.match(/<h2\b/gu) ?? []).length, 1);
      assert.match(body, /<progress\b/u);
      if (state === "running") assert.match(body, />Pause<|>Pause<!--/u);
      if (state === "paused") assert.match(body, />Resume<|>Resume<!--/u);
      if (state === "done" || state === "failed") assert.match(body, /task-result-callout/u);
    }
    const { body } = render(TaskProgressDialog, { props: {
      ...surface, presentation: "window", task: extractTask("running"),
      passwordQuestion: { name: "reports.zip", detail: "Encrypted archive", sessionDetail: "This task only" },
    } });
    assert.match(body, /type="password"/u);
    assert.match(body, /Unlock and continue/u);
  } finally {
    await server.close();
  }
});

test("main-window task dialogs keep modal semantics and their task heading context", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule(
      "/src/components/TaskProgressDialog.svelte",
    );
    const { surface } = taskSurface(false);
    const { body } = render(TaskProgressDialog, { props: { ...surface, task: extractTask("running") } });
    assert.match(body, /role="dialog" aria-modal="true"/u);
    assert.match(body, /task-modal-eyebrow/u);
  } finally {
    await server.close();
  }
});

test("deferred task views retain their window surface while the component loads", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialogHost } = await server.ssrLoadModule(
      "/src/components/TaskProgressDialogHost.svelte",
    );
    const { surface } = taskSurface(true);
    const { body } = render(TaskProgressDialogHost, { props: {
      surface: { ...surface, task: extractTask("running") },
      loadingTitle: "Loading task view", loadingBody: "Preparing task controls",
      failureTitle: "Task view unavailable", failureBody: "Retry the view",
      retryLabel: "Retry", backLabel: "Back to tasks",
    } });
    assert.match(body, /class="task-window-surface [^"]*task-surface-shell"/u);
    assert.match(body, /data-task-presentation="window"/u);
    assert.match(body, /role="status" aria-live="polite" aria-busy="true"/u);
    assert.match(body, /Loading task view/u);
    assert.doesNotMatch(body, /aria-modal="true"/u);
    await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
  } finally {
    await server.close();
  }
});

test("window task surfaces fill their host and keep scrollable content inside the window", () => {
  const css = readFileSync(new URL("../design.css", import.meta.url), "utf8");
  const rules = parseCss(css).children.filter((node) => node.type === "Rule");
  function declarations(selector) {
    const rule = rules.find((entry) => entry.prelude.children.some((item) =>
      css.slice(item.start, item.end).trim() === selector));
    assert.ok(rule, selector);
    return Object.fromEntries(rule.block.children.filter((node) => node.type === "Declaration")
      .map((node) => [node.property, node.value]));
  }
  const window = declarations(".task-window-root");
  assert.equal(window.height, "100dvh");
  assert.equal(window.overflow, "hidden");
  const card = declarations(".task-window-surface .task-modal-card");
  assert.equal(card.width, "100%");
  assert.equal(card.height, "100%");
  assert.equal(card["max-height"], "none");
  assert.equal(card.border, "0");
  assert.equal(card["border-radius"], "0");
  assert.equal(card["box-shadow"], "none");
  assert.match(card.padding, /^var\(--/u);
  assert.equal(declarations(".task-modal-body").overflow, "auto");
  assert.equal(declarations(".task-modal-overlay").position, "fixed");
});
