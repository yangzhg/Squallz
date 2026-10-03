import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { parseCss } from "svelte/compiler";

import { createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

async function taskSurface(taskWindowMode, server) {
  const { SettingsSession } = await server.ssrLoadModule("/src/lib/settings-session.svelte.ts");
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const [declaration] = selectFunctions(source, ["taskDialogSurface"]);
  assert.ok(declaration);
  const outputText = compileTestScript(declaration.getText(source));
  const callbacks = Object.fromEntries([
    "taskOutputPath", "taskRevealOutputLabel", "pauseCurrentTask", "resumeCurrentTask",
    "cancelCurrentTask", "copyTaskChecksumResults", "openTaskOutput", "openMacosSfxPublisher",
    "prepareTaskReview", "toggleTaskDetails", "viewTaskResults", "revealTaskOutput",
    "dismissTaskDialog", "submitTaskPasswordRequest", "cancelTaskPasswordRequest", "answerConflictDecision",
  ].map((name) => [name, () => {}]));
  const surface = vm.runInNewContext(`${outputText}\ntaskDialogSurface`, {
    ...callbacks,
    taskWindowMode,
    pendingTaskReviewId: null,
    activePlatform: "macos",
    settingsSession: new SettingsSession({ platform: () => "macos", tr: (_key, fallback) => fallback, emit() {} },
      { paletteOverride: "ocean" }),
    activeTheme: "dark",
    activeDensityChoice: "comfortable",
    customPaletteVariables: () => ({}),
    taskChecksumCopyFeedback: () => null,
    taskChecksumCopyFeedbackTone: () => null,
    checksumCopyPending: () => false,
    taskPasswordQuestion: () => null,
    taskConflictQuestion: () => null,
    jobPasswordPrompt: null, jobPasswordValue: "",
    jobConflictPrompt: null,
    isCurrentTaskPasswordPrompt: () => false,
    jobPasswordSubmissionError: null,
    conflictApplyAll: false,
  })({ id: 42 });
  return { surface, callbacks };
}

test("task surfaces choose window or dialog presentation without changing task actions", async () => {
  const server = await createTestServer();
  try {
    for (const taskWindowMode of [true, false]) {
      const { surface, callbacks } = await taskSurface(taskWindowMode, server);
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
  } finally {
    await server.close();
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

test("waiting task surfaces retain measured progress without rates or processing animation", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
    const { default: TaskCenter } = await server.ssrLoadModule("/src/components/TaskCenter.svelte");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const helpers = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { surface } = await taskSurface(false, server);
    for (const [locale, passwordLabel, conflictLabel, pausedLabel] of [
      ["en-US", "Waiting for a password", "Waiting for a file decision", "Paused"],
      ["zh-CN", "正在等待密码", "正在等待文件冲突处理", "已暂停"],
    ]) {
      await loadLocale(locale);
      for (const [state, interaction, label] of [
        ["running", "password", passwordLabel],
        ["running", "conflict", conflictLabel],
        ["paused", null, pausedLabel],
      ]) {
        const task = { ...extractTask(state), interaction };
        assert.equal(helpers.taskOverallProgressBadge(task), label);
        assert.equal(helpers.taskCurrentProgressBadge(task), label);
        assert.ok(helpers.taskProgressSummary(task).includes(label));
        assert.match(helpers.taskProgressSummary(task), /40%.*40 B \/ 100 B/u);
        assert.match(helpers.taskCurrentProgressSummary(task), /report.txt.*4 B \/ 10 B/u);
        assert.doesNotMatch(helpers.taskProgressSummary(task), /\/s|\/秒/u);
        for (const presentation of ["dialog", "panel", "window"]) {
          const { body } = render(TaskProgressDialog, { props: { ...surface, presentation, task } });
          assert.match(body, /data-task-active="false"/u);
          assert.match(body, /value="40"/u);
          assert.doesNotMatch(body, /\d+ B\/(?:s|秒)/u);
          const unknown = { ...task, total: 0, currentTotal: 0, phase: "archive_open" };
          const pending = render(TaskProgressDialog, { props: { ...surface, presentation, task: unknown } }).body;
          assert.doesNotMatch(pending, /<progress\b|task-current-pending active/u);
          assert.ok(pending.includes(label));
        }
        const row = render(TaskCenter, { props: { tasks: [{ ...task, queueMoveIntent: null }], rootClass: "task-center" } }).body;
        assert.ok(row.includes(helpers.taskProgressSummary(task)));
        const batch = { ...task, done: 1500, total: 2000, spec: { kind: "batch_extract", items: [{ path: "a.zip.001" }, { path: "a.zip.002" }, { path: "b.zip" }] } };
        assert.ok(helpers.taskProgressSummary(batch).includes(label));
        assert.match(helpers.taskProgressSummary(batch), /75%.*1\/2/u);
        for (const presentation of ["dialog", "panel", "window"]) {
          const body = render(TaskProgressDialog, { props: { ...surface, presentation, task: batch } }).body;
          assert.match(body, /75%.*1\/2/u);
          assert.ok(body.includes(label));
          assert.doesNotMatch(body, /\d+ B\/(?:s|秒)/u);
        }
        assert.ok(helpers.taskProgressSummary({ ...task, scanEntries: 37 }).includes(label));
      }
      const running = render(TaskProgressDialog, { props: { ...surface, task: extractTask("running") } }).body;
      assert.match(running, /data-task-active="true"/u);
      assert.match(helpers.taskProgressSummary(extractTask("running")), /10 B\/(?:s|秒)/u);
      for (const state of ["done", "failed", "cancelled"]) {
        assert.doesNotMatch(helpers.taskProgressSummary(extractTask(state)), /\/s|\/秒/u);
      }
      for (const state of ["failed", "cancelled"]) {
        const stopped = { ...extractTask(state), current: "reports/客户交付/Quarterly financial report.pdf" };
        const summary = `${stopped.current} · 4 B / 10 B`;
        assert.equal(helpers.taskCurrentProgressSummary(stopped), summary);
        assert.equal(helpers.taskCurrentProgressPercent(stopped), 40);
        for (const presentation of ["dialog", "panel", "window"]) {
          const body = render(TaskProgressDialog, { props: { ...surface, presentation, task: stopped } }).body;
          assert.ok(body.includes(summary));
          assert.ok(body.includes(helpers.taskCurrentSectionLabel(stopped)));
          assert.match(body, /data-task-active="false"/u);
          assert.doesNotMatch(body, /\d+ B\/(?:s|秒)/u);
          const unknown = { ...stopped, phase: "archive_open", currentDone: 0, currentTotal: 0 };
          const pending = render(TaskProgressDialog, { props: { ...surface, presentation, task: unknown } }).body;
          assert.ok(pending.includes(stopped.current));
          assert.doesNotMatch(pending, /data-task-progress="current-file"|task-current-pending active/u);
        }
        const empty = { ...stopped, current: "", currentDone: 0, currentTotal: 0 };
        assert.equal(helpers.taskCurrentSectionVisible(empty), false);
        assert.equal(helpers.taskCurrentProgressSummary(empty), helpers.taskStateLabel(state));
      }
      const cancelled = { ...extractTask("cancelled"), expanded: true };
      assert.deepEqual(helpers.taskResultDetailRows(cancelled), []);
      for (const presentation of ["dialog", "panel", "window"]) {
        const taskWindowMode = presentation === "window";
        const body = render(TaskProgressDialog, { props: { ...surface, presentation, taskWindowMode, task: cancelled } }).body;
        assert.doesNotMatch(body, /class="task-result-details"/u);
        assert.ok(body.includes(helpers.taskDialogResultSummary(cancelled)));
        assert.ok(body.includes(helpers.taskNextStepDetail(cancelled, taskWindowMode)));
        assert.equal(body.includes(helpers.taskReviewActionLabel(cancelled)), !taskWindowMode);
        assert.doesNotMatch(body, /data-task-active="true"/u);
      }
      const partialBatch = { ...extractTask("done"), expanded: true,
        spec: { kind: "batch_extract", items: [{ path: "damaged.zip" }] },
        result: { archives: 1, extracted: 1, failed: 0, outputs: [
          { archive: "damaged.zip", counts: { failed: 1, skipped: 0 }, problems: ["bad.txt: CRC mismatch"] },
        ] } };
      for (const presentation of ["dialog", "panel", "window"]) {
        const taskWindowMode = presentation === "window";
        const body = render(TaskProgressDialog, { props: { ...surface, presentation, taskWindowMode, task: partialBatch } }).body;
        assert.ok(body.includes(helpers.taskDialogResultSummary(partialBatch)));
        assert.ok(body.includes("bad.txt: CRC mismatch"));
        assert.equal(body.includes(helpers.taskReviewActionLabel(partialBatch)), !taskWindowMode);
      }
    }
  } finally {
    await server.close();
  }
});

test("control failures stay visible and actionable across task surfaces", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
    const { default: TaskCenter } = await server.ssrLoadModule("/src/components/TaskCenter.svelte");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const { surface } = await taskSurface(false, server);
    for (const [locale, labels] of [
      ["en-US", ["Could not pause the task", "Could not resume the task", "Could not cancel the task"]],
      ["zh-CN", ["未能暂停任务", "未能继续任务", "未能取消任务"]],
    ]) {
      await loadLocale(locale);
      for (const [index, actionFailure] of ["pause", "resume", "cancel"].entries()) {
        const task = { ...extractTask(actionFailure === "resume" ? "paused" : "running"), actionFailure };
        for (const presentation of ["dialog", "panel", "window"]) {
          const body = render(TaskProgressDialog, { props: { ...surface, presentation, task } }).body;
          assert.match(body, /class="task-action-error" role="alert"/u);
          assert.ok(body.includes(labels[index]));
          assert.doesNotMatch(body, /<button[^>]*disabled/u, "the user can retry the control");
          const finished = render(TaskProgressDialog, { props: { ...surface, presentation,
            task: { ...task, state: "done" } } }).body;
          assert.ok(!finished.includes(labels[index]), "a completed task never carries an obsolete control failure");
        }
        const body = render(TaskCenter, { props: { tasks: [{ ...task, queueMoveIntent: null }], rootClass: "task-center" } }).body;
        assert.match(body, /class="task-action-error" role="alert"/u);
        assert.ok(body.includes(labels[index]));
      }
    }
  } finally {
    await server.close();
  }
});

test("interrupted status keeps measured progress and inputs without presenting live activity", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
    const { default: TaskCenter } = await server.ssrLoadModule("/src/components/TaskCenter.svelte");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const helpers = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { surface } = await taskSurface(false, server);
    for (const locale of ["en-US", "zh-CN"]) {
      await loadLocale(locale);
      const task = { ...extractTask("running"), statusStale: true };
      const label = helpers.taskOutcomeStateLabel(task);
      const detail = helpers.taskStatusUnavailableMessage();
      assert.notEqual(label, helpers.taskStateLabel("running"));
      for (const presentation of ["dialog", "panel", "window"]) {
        const body = render(TaskProgressDialog, { props: { ...surface, presentation, task } }).body;
        assert.ok(body.includes(label));
        assert.ok(body.includes(detail));
        assert.match(body, /data-task-active="false"/u);
        assert.match(body, /value="40"/u);
        assert.match(body, /report.txt.*4 B \/ 10 B/u);
        assert.doesNotMatch(body, /10 B\/(?:s|秒)/u);
        const pending = render(TaskProgressDialog, { props: { ...surface, presentation,
          task: { ...task, total: 0, currentTotal: 0, phase: "archive_open" } } }).body;
        assert.doesNotMatch(pending, /<progress\b|task-current-pending active/u);
        const waiting = render(TaskProgressDialog, { props: { ...surface, presentation,
          task: { ...task, interaction: "password" },
          passwordQuestion: { name: "reports.zip", detail: "", sessionDetail: "" }, passwordValue: "" } }).body;
        assert.match(waiting, /type="password"/u);
        assert.ok(waiting.includes(detail));
        const done = render(TaskProgressDialog, { props: { ...surface, presentation, task: { ...task, state: "done" } } }).body;
        assert.ok(!done.includes(detail), "confirmed results remain visible during a later outage");
      }
      const row = render(TaskCenter, { props: { tasks: [{ ...task, queueMoveIntent: null }], rootClass: "task-center" } }).body;
      assert.ok(row.includes(detail));
      assert.ok(row.includes(label));
      assert.doesNotMatch(row, /10 B\/(?:s|秒)/u);
    }
  } finally {
    await server.close();
  }
});

test("window task views render one task heading and retain progress, results and input", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule(
      "/src/components/TaskProgressDialog.svelte",
    );
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    await loadLocale("en-US");
    const { surface } = await taskSurface(true, server);
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

test("all task surfaces show folder finalization without finished byte bars", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
    const { default: TaskCenter } = await server.ssrLoadModule("/src/components/TaskCenter.svelte");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const helpers = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { surface } = await taskSurface(false, server);
    for (const [locale, phase, currentLabel] of [
      ["en-US", "Restoring folder information", "Current folder"],
      ["zh-CN", "正在恢复文件夹信息", "当前文件夹"],
    ]) {
      await loadLocale(locale);
      for (const kind of ["extract", "batch_extract"]) {
        const task = { ...extractTask("running"), phase: "extract_metadata", done: 100, total: 100,
          current: "reports/客户交付", currentDone: 10, currentTotal: 10,
          spec: kind === "extract" ? extractTask("running").spec : { kind, items: [{ path: "first.zip" }, { path: "second.zip" }] },
        };
        assert.equal(helpers.taskOverallProgressBadge(task), phase);
        assert.equal(helpers.taskOverallProgressIndeterminate(task), true);
        assert.equal(helpers.taskCurrentSectionLabel(task), currentLabel);
        assert.equal(helpers.hasTaskCurrentProgress(task), false);
        assert.doesNotMatch(helpers.taskProgressSummary(task), /100%|\/s|100 B/u);
        for (const presentation of ["dialog", "panel", "window"]) {
          const { body } = render(TaskProgressDialog, { props: { ...surface, presentation, task } });
          assert.ok(body.includes(phase));
          assert.ok(body.includes(currentLabel));
          assert.match(body, /reports\/客户交付/u);
          assert.equal((body.match(/<progress\b/gu) ?? []).length, 1);
          assert.doesNotMatch(body.match(/<progress\b[^>]*>/u)?.[0] ?? "", /\bvalue=/u);
          assert.doesNotMatch(body, /100%|100 B|disabled/u);
        }
        const paused = { ...task, state: "paused" };
        assert.equal(helpers.taskOverallProgressIndeterminate(paused), true);
        assert.doesNotMatch(helpers.taskOverallProgressBadge(paused), /100%/u);
        const done = { ...task, state: "done" };
        assert.equal(helpers.taskOverallProgressBadge(done), "100%");
        assert.equal(helpers.taskOverallProgressIndeterminate(done), false);
        const next = { ...task, phase: "extract_entries", done: 50 };
        assert.equal(helpers.taskOverallProgressBadge(next), "50%");
        assert.equal(helpers.hasTaskCurrentProgress(next), true);
        if (kind === "batch_extract") assert.doesNotMatch(helpers.taskProgressSummary(next), / B|\/s/u);
        for (const row of [task, paused, next]) {
          const { body } = render(TaskCenter, { props: {
            tasks: [{ ...row, queueMoveIntent: null }], rootClass: "task-center",
          } });
          assert.ok(body.includes(helpers.taskProgressSummary(row)));
          if (row.phase === "extract_metadata") assert.doesNotMatch(body, /<progress\b|\d+%|disabled/u);
          else assert.match(body, /value="50"/u);
        }
      }
    }
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
    const { surface } = await taskSurface(false, server);
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
    const { surface } = await taskSurface(true, server);
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

test("cancelling and terminal task surfaces do not ask for input from a stale interaction", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: TaskProgressDialog } = await server.ssrLoadModule("/src/components/TaskProgressDialog.svelte");
    const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const { taskCenterCounts } = await server.ssrLoadModule("/src/lib/task-center.ts");
    const { surface } = await taskSurface(false, server);
    for (const locale of ["en-US", "zh-CN"]) {
      await loadLocale(locale);
      for (const presentation of ["dialog", "panel", "window"]) {
        for (const interaction of ["password", "conflict"]) {
          const cancelling = { ...extractTask("running"), interaction, controlIntent: "cancel" };
          const pending = render(TaskProgressDialog, { props: { ...surface, presentation, task: cancelling } }).body;
          assert.ok(pending.includes(tFallback("gui.task.cancelling")));
          assert.doesNotMatch(pending, /task-interaction-callout/);
          assert.ok(!pending.includes(tFallback("gui.task_center.needs_input")));
          assert.equal(taskCenterCounts([cancelling]).attention, 0);
          const terminal = { ...cancelling, state: "cancelled", controlIntent: null };
          const finished = render(TaskProgressDialog, { props: { ...surface, presentation, task: terminal } }).body;
          assert.ok(finished.includes(tFallback("gui.task.state.cancelled")));
          assert.doesNotMatch(finished, /task-interaction-callout/);
          assert.ok(!finished.includes(tFallback("gui.task_center.needs_input")));
          assert.equal(taskCenterCounts([terminal]).attention, 0);
          const failedAnswer = { ...extractTask("running"), interaction, actionFailure: "answer" };
          const response = render(TaskProgressDialog, { props: {
            ...surface, presentation, task: failedAnswer,
            passwordQuestion: interaction === "password" ? { name: "reports.zip", detail: "", sessionDetail: "" } : null,
            conflictQuestion: interaction === "conflict" ? { path: "reports.txt", existing: "8 B", incoming: "12 B" } : null,
          } }).body;
          assert.ok(response.includes(tFallback("gui.task.answer_failed_detail")));
          assert.match(response, /class="task-question-error" role="alert"/);
          if (interaction === "conflict") {
            for (const label of locale === "zh-CN"
              ? ["1 个条目已存在", "已有文件", "压缩包内", "将本次决定应用到剩余冲突", "取消解压", "跳过", "覆盖", "保留两者"]
              : ["1 item already exists", "Existing file", "In archive", "Apply this decision to remaining conflicts", "Cancel extraction", "Skip", "Replace", "Keep Both"]
            ) assert.ok(response.includes(label), `${presentation} conflict request includes ${label}`);
            assert.equal((response.match(/class="task-question-card"/g) ?? []).length, 1);
          }
        }
      }
    }
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
