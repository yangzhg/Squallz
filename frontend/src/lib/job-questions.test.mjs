import assert from "node:assert/strict";
import test from "node:test";
import { webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

function snapshot(id, version, kind = "password") {
  const prompt = kind === "password"
    ? { id, version, name: `archive-${id}.zip`, wrong: false }
    : { id, version, existing_path: "/output/report.txt", existing_size: 8,
        existing_modified: null, incoming_path: "report.txt", incoming_size: 12,
        incoming_modified: null };
  return {
    id, version, spec: { kind: "test", path: `/archives/archive-${id}.zip`, password: null },
    output_password_required: false,
    origin: "app", owned_by_requester: true, state: "running", queue_position: null,
    queue_wait_reason: null, cpu_threads: 1, stream_buffer_limit_bytes: null,
    progress: { done: 0, total: 100, current: "", current_done: 0, current_total: 0,
      speed: 0, phase: null, interruptible: true }, error: null, result: null,
    interaction: kind, question: { kind, prompt },
  };
}

async function until(predicate) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.fail("Expected task question state was not reached");
}

test("reconnecting restores every pending question without replaying prompt events", async () => {
  const server = await createTestServer();
  const previousWindow = globalThis.window;
  globalThis.window = { crypto: webcrypto };
  let dispose;
  try {
    const { mockIPC, mockWindows } = await server.ssrLoadModule("@tauri-apps/api/mocks");
    const { emit } = await server.ssrLoadModule("@tauri-apps/api/event");
    mockWindows("main");
    mockIPC(() => {}, { shouldMockEvents: true });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const records = [snapshot(1, 10), snapshot(2, 11), snapshot(3, 12, "conflict")];
    let revision = 12;
    let polls = 0;
    ipc.jobSnapshots = async () => {
      polls += 1;
      return structuredClone({ revision, reset: true, upserts: records, removed: [] });
    };
    ipc.jobSnapshot = async (id) => structuredClone(records.find((item) => item.id === id) ?? null);
    const answers = [];
    let releaseAnswer;
    ipc.answerPassword = (...args) => {
      answers.push(args);
      return new Promise((resolve) => { releaseAnswer = resolve; });
    };
    dispose = await jobs.initJobEvents();
    await until(() => jobs.tasks().length === 3);
    assert.equal(jobs.pendingPassword().id, 1);
    assert.equal(jobs.pendingConflict().id, 3);
    assert.ok(jobs.tasks().every((task) => task.question !== null));

    const initialPrompt = jobs.pendingPassword();
    records[0].version = ++revision;
    records[0].progress.done = 10;
    await emit("job://ask-password", records[0].question.prompt);
    await until(() => jobs.tasks()[0].done === 10);
    assert.equal(jobs.pendingPassword(), initialPrompt, "progress preserves the active form");

    jobs.answerPassword("answer");
    assert.deepEqual(answers, [[1, 10, "answer"]]);
    assert.equal(jobs.pendingPassword().id, 2, "the second waiting job remains available");
    const beforePoll = polls;
    await until(() => polls > beforePoll);
    assert.equal(jobs.pendingPassword().id, 2, "an in-flight answer is not reopened by an older snapshot");

    records[0].question = { kind: "password", prompt: { id: 1, version: ++revision,
      name: "archive-1.zip", wrong: true } };
    records[0].version = revision;
    releaseAnswer();
    await until(() => jobs.pendingPassword()?.wrong === true);
    assert.equal(jobs.pendingPassword().version, revision);

    ipc.answerPassword = async () => { throw new Error("transport unavailable"); };
    jobs.answerPassword("retry");
    await until(() => jobs.pendingPassword()?.id === 1);
    assert.equal(jobs.pendingPassword().wrong, true, "a failed submission restores the unanswered question");

    records[0].state = "cancelled";
    records[0].version = ++revision;
    records[0].question = null;
    records[0].interaction = null;
    await emit("job://ask-password", initialPrompt);
    await until(() => jobs.tasks()[0].state === "cancelled");
    assert.equal(jobs.pendingPassword().id, 2, "a late prompt event cannot reopen a cancelled task");

    records.splice(1, 2);
    revision += 1;
    await until(() => jobs.tasks().length === 1);
    assert.equal(jobs.pendingPassword(), null);
    assert.equal(jobs.pendingConflict(), null);
  } finally {
    dispose?.();
    await server.close();
    globalThis.window = previousWindow;
  }
});

test("pause and resume events do not restore an old transfer rate", async () => {
  await withQuestionFeed(async ({ jobs, records, emit }) => {
    const record = records[0];
    record.interaction = null;
    record.question = null;
    record.version = 12;
    record.progress = { ...record.progress, done: 40, speed: 64 };
    await emit("job://ask-password", { id: 1 });
    await until(() => jobs.tasks()[0].version === 12);
    const task = jobs.tasks()[0];
    assert.equal(task.speed, 64);
    await emit("job://state", { id: 1, version: 13, state: "paused" });
    assert.equal(task.speed, 0);
    assert.equal(task.done, 40);
    await emit("job://state", { id: 1, version: 14, state: "running" });
    assert.equal(task.speed, 0);
    await emit("job://progress", { id: 1, version: 15, ...record.progress, done: 50, speed: 80 });
    assert.equal(task.speed, 80);
    assert.equal(task.done, 50);
    await emit("job://state", { id: 1, version: 16, state: "cancelled" });
    assert.equal(task.speed, 0);
  });
});

test("snapshot connection failures preserve tasks and recover after a successful poll", async () => {
  await withQuestionFeed(async ({ jobs, ipc, server }) => {
    const { taskWindowRecoveryState } = await server.ssrLoadModule("/src/lib/task-window.ts");
    assert.equal(taskWindowRecoveryState("loading").status, "reconnecting");
    assert.equal(jobs.jobSnapshotStatus(), "ready");
    const prompt = jobs.pendingPassword();
    ipc.jobSnapshots = async () => { throw new Error("connection unavailable"); };
    await until(() => jobs.jobSnapshotStatus() === "unavailable");
    assert.equal(taskWindowRecoveryState(jobs.jobSnapshotStatus()).status, "reconnect-error");
    assert.equal(jobs.tasks().length, 2);
    assert.equal(jobs.pendingPassword(), prompt);
    ipc.jobSnapshots = async () => ({ revision: 11, reset: false, upserts: [], removed: [] });
    await until(() => jobs.jobSnapshotStatus() === "ready");
    assert.equal(taskWindowRecoveryState(jobs.jobSnapshotStatus()).status, "task-unavailable");
    assert.equal(jobs.pendingPassword(), prompt);
  });
});

test("unavailable status retains progress and questions without reviving an old rate on reconnect", async () => {
  await withQuestionFeed(async ({ jobs, ipc, records, emit, server }) => {
    const helpers = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    Object.assign(records[1], { version: 12, question: null, interaction: null });
    Object.assign(records[1].progress, { done: 40, speed: 64 });
    await emit("job://ask-conflict", { id: 2 });
    await until(() => jobs.tasks()[1].version === 12);
    const task = jobs.tasks()[1];
    const question = jobs.pendingPassword();
    assert.equal(task.speed, 64);
    ipc.jobSnapshots = async () => { throw new Error("status unavailable"); };
    await until(() => jobs.jobSnapshotStatus() === "unavailable");
    assert.equal(task.statusStale, true);
    assert.equal(task.done, 40);
    assert.equal(task.speed, 0);
    assert.equal(jobs.pendingPassword(), question);
    assert.equal(helpers.taskProgressActive(task), false);
    await emit("job://progress", { id: 2, version: 13, ...records[1].progress, done: 50, speed: 80 });
    assert.equal(task.done, 50);
    assert.equal(task.speed, 0, "progress events do not conceal the interrupted status feed");
    ipc.jobSnapshots = async () => ({ revision: 13, reset: false, upserts: [], removed: [] });
    await until(() => jobs.jobSnapshotStatus() === "ready");
    assert.equal(jobs.tasks()[1], task);
    assert.equal(jobs.pendingPassword(), question);
    assert.equal(task.statusStale, false);
    assert.equal(task.speed, 0, "an empty delta does not make an old speed current");
    await emit("job://progress", { id: 2, version: 14, ...records[1].progress, done: 60, speed: 96 });
    assert.equal(task.speed, 96);
    assert.equal(helpers.taskProgressActive(task), true);
    ipc.jobSnapshots = async () => { throw new Error("status unavailable"); };
    await until(() => jobs.jobSnapshotStatus() === "unavailable");
    await emit("job://state", { id: 2, version: 15, state: "done", result: { ok: true } });
    assert.equal(helpers.taskOutcomeStateLabel(task), helpers.taskStateLabel("done"));
    assert.equal(task.statusStale, false, "a terminal event still confirms the result");
  });
});

test("snapshot polling continues when native event listeners fail to initialize", async () => {
  const server = await createTestServer();
  const previousWindow = globalThis.window;
  globalThis.window = { crypto: webcrypto };
  let dispose;
  try {
    const { mockIPC, mockWindows } = await server.ssrLoadModule("@tauri-apps/api/mocks");
    mockWindows("main");
    mockIPC(() => { throw new Error("event listener unavailable"); });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const record = { ...snapshot(1, 10), question: null, interaction: null };
    ipc.jobSnapshots = async () => ({ revision: record.version, reset: true, upserts: [structuredClone(record)], removed: [] });
    dispose = await jobs.initJobEvents();
    await until(() => jobs.tasks().length === 1);
    assert.equal(jobs.jobSnapshotStatus(), "ready");
    assert.equal(jobs.tasks()[0].statusStale, false);
    record.state = "paused";
    record.version += 1;
    await until(() => jobs.tasks()[0].state === "paused");
    assert.equal(jobs.tasks()[0].version, 11, "the fallback remains a live authoritative feed");
  } finally {
    dispose?.();
    await server.close();
    globalThis.window = previousWindow;
  }
});

test("snapshot questions enforce ownership and terminal state while preserving form identity", async () => {
  const server = await createTestServer();
  try {
    const { snapshotQuestion } = await server.ssrLoadModule("/src/lib/job-snapshot.ts");
    const waiting = snapshot(1, 5);
    assert.equal(snapshotQuestion(waiting), waiting.question);
    assert.equal(snapshotQuestion({ ...waiting, owned_by_requester: false }), null);
    assert.equal(snapshotQuestion({ ...waiting, state: "done" }), null);
    assert.equal(snapshotQuestion(structuredClone(waiting), waiting.question), waiting.question);
    assert.equal(snapshotQuestion({ ...waiting, question: null }, waiting.question), null);
  } finally {
    await server.close();
  }
});

async function withQuestionFeed(run) {
  const server = await createTestServer();
  const previousWindow = globalThis.window;
  globalThis.window = { crypto: webcrypto };
  let dispose;
  try {
    const { mockIPC, mockWindows } = await server.ssrLoadModule("@tauri-apps/api/mocks");
    const { emit } = await server.ssrLoadModule("@tauri-apps/api/event");
    mockWindows("main");
    mockIPC(() => {}, { shouldMockEvents: true });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const records = [snapshot(1, 10), snapshot(2, 11, "conflict")];
    let polls = 0;
    ipc.jobSnapshots = async (revision) => {
      polls += 1;
      return { revision: 11, reset: revision === null,
        upserts: revision === null ? structuredClone(records) : [], removed: [] };
    };
    ipc.jobSnapshot = async (id) => structuredClone(records.find((record) => record.id === id));
    dispose = await jobs.initJobEvents();
    await until(() => jobs.tasks().length === 2);
    await run({ jobs, ipc, records, emit, server, polls: () => polls });
  } finally {
    dispose?.();
    await server.close();
    globalThis.window = previousWindow;
  }
}

test("resuming into the queue releases controls through both events and snapshots", async () => {
  await withQuestionFeed(async ({ jobs, ipc, records, emit }) => {
    ipc.resumeJob = async () => {};
    for (const [index, delivery] of ["event", "snapshot"].entries()) {
      const id = index + 1;
      await emit("job://state", { id, version: 20, state: "paused" });
      jobs.resumeTask(id);
      const task = jobs.tasks().find((item) => item.id === id);
      assert.equal(task.controlIntent, "resume");
      if (delivery === "event") {
        await emit("job://state", { id, version: 21, state: "queued" });
      } else {
        Object.assign(records[index], { version: 21, state: "queued", interaction: null, question: null });
        await emit("job://ask-password", { id });
        await until(() => task.version === 21);
      }
      assert.equal(task.state, "queued");
      assert.equal(task.controlIntent, null, "a resumed task may still wait for a worker");
      let cancelled = false;
      ipc.cancelJob = async () => { cancelled = true; };
      jobs.cancelTask(id);
      assert.equal(cancelled, true);
    }
  });
});

test("late control failures cannot clear a newer request or report a completed request as failed", async () => {
  await withQuestionFeed(async ({ jobs, ipc, emit, server }) => {
    const notices = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    try {
      const failures = [];
      ipc.pauseJob = () => new Promise((_, reject) => { failures.push(reject); });
      const first = jobs.pauseTask(1);
      await emit("job://state", { id: 1, version: 20, state: "paused" });
      await emit("job://state", { id: 1, version: 21, state: "running" });
      const next = jobs.pauseTask(1);
      const task = jobs.tasks()[0];
      assert.equal(task.controlIntent, "pause");
      failures[0](new Error("late response failure"));
      await first;
      assert.equal(task.controlIntent, "pause", "the new pause remains pending");
      assert.equal(notices.toasts().length, 0);
      await emit("job://state", { id: 1, version: 22, state: "paused" });
      failures[1](new Error("response lost after confirmation"));
      await next;
      assert.equal(task.controlIntent, null);
      assert.equal(notices.toasts().length, 0, "the authoritative state already confirmed the request");

      let rejectResume;
      ipc.resumeJob = () => new Promise((_, reject) => { rejectResume = reject; });
      const resume = jobs.resumeTask(1);
      ipc.cancelJob = async () => {};
      jobs.cancelTask(1);
      rejectResume(new Error("late resume failure"));
      await resume;
      assert.equal(task.controlIntent, "cancel");
      assert.equal(notices.toasts().length, 0, "cancellation supersedes the old request");

      const failedPause = jobs.pauseTask(2);
      failures[2](new Error("current request failed"));
      assert.equal(await failedPause, false);
      assert.equal(jobs.tasks()[1].controlIntent, null, "a current failure releases controls for retry");
      assert.equal(jobs.tasks()[1].actionFailure, "pause");
      assert.equal(notices.toasts().length, 0, "the error is presented with its task, not behind a modal");
      let retryAccepted;
      ipc.pauseJob = () => new Promise((resolve) => { retryAccepted = resolve; });
      const retry = jobs.pauseTask(2);
      assert.equal(jobs.tasks()[1].actionFailure, null, "retry clears the old inline error immediately");
      retryAccepted();
      assert.equal(await retry, true);
      await emit("job://state", { id: 2, version: 20, state: "paused" });
      assert.equal(jobs.tasks()[1].controlIntent, null);
    } finally {
      for (const toast of [...notices.toasts()]) notices.dismissToast(toast.id);
    }
  });
});

test("task control notices wait for acceptance and do not outlive the pending action", async () => {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const actions = [["pauseCurrentTask", "pauseTask", "pause"], ["resumeCurrentTask", "resumeTask", "resume"], ["cancelCurrentTask", "cancelTask", "cancel"]];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && actions.some(([name]) => node.name?.text === name));
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  for (const [handler, control, intent] of actions) {
    for (const outcome of ["accepted", "failed", "confirmed", "superseded"]) {
      let finish;
      const notices = [];
      const task = { id: 1, controlIntent: intent };
      const context = { [control]: () => new Promise((resolve) => { finish = resolve; }),
        showNotice: (message) => notices.push(message), tr: (_key, fallback) => fallback };
      const run = vm.runInNewContext(`${outputText}\n${handler}`, context);
      const request = run(task);
      assert.deepEqual(notices, [], "the IPC request is still pending");
      if (outcome === "confirmed") task.controlIntent = null;
      if (outcome === "superseded") task.controlIntent = "another action";
      finish(outcome !== "failed");
      await request;
      assert.equal(notices.length, outcome === "accepted" ? 1 : 0);
    }
  }
});

test("rejected cancellation restores password and conflict inputs without a new snapshot revision", async () => {
  await withQuestionFeed(async ({ jobs, ipc, polls }) => {
    for (const [id, pending] of [[1, jobs.pendingPassword], [2, jobs.pendingConflict]]) {
      const original = pending();
      let reject;
      let requests = 0;
      ipc.cancelJob = () => { requests += 1; return new Promise((_, fail) => { reject = fail; }); };
      jobs.cancelTask(id);
      assert.equal(pending(), null);
      reject(new Error("service unavailable"));
      await until(() => jobs.tasks().find((task) => task.id === id).controlIntent === null);
      assert.equal(pending(), original, "the still-unanswered prompt must remain recoverable");
      assert.equal(jobs.tasks().find((task) => task.id === id).actionFailure, "cancel");
      const before = polls();
      await until(() => polls() > before);
      assert.equal(pending(), original, "an empty delta cannot recover a discarded prompt");
      assert.equal(requests, 1);
    }
  });
});

test("cancellation keeps inputs unavailable through snapshot refreshes and ignores duplicate control requests", async () => {
  await withQuestionFeed(async ({ jobs, ipc, records, emit, server }) => {
    let release;
    let requests = 0;
    ipc.cancelJob = () => { requests += 1; return new Promise((resolve) => { release = resolve; }); };
    jobs.cancelTask(1);
    jobs.cancelTask(1);
    assert.equal(requests, 1);
    records[0].version = 12;
    records[0].progress.done = 25;
    await emit("job://ask-password", records[0].question.prompt);
    await until(() => jobs.tasks()[0].done === 25);
    assert.equal(jobs.pendingPassword(), null, "polling cannot reopen input during cancellation");
    assert.equal(jobs.tasks()[0].controlIntent, "cancel");
    release();
    await emit("job://state", { id: 1, version: 13, state: "cancelled", error: null });
    await until(() => jobs.tasks()[0].state === "cancelled");
    const task = jobs.tasks()[0];
    assert.equal(task.question, null);
    assert.equal(task.interaction, null, "a terminal event immediately removes Needs input");
    assert.equal(task.controlIntent, null);
    assert.equal(task.actionFailure, null);
    const { taskCenterCounts } = await server.ssrLoadModule("/src/lib/task-center.ts");
    assert.equal(taskCenterCounts([task]).attention, 0);
  });
});

test("failed answers survive a failed refresh and never revive a cleared or newer question", async () => {
  await withQuestionFeed(async ({ jobs, ipc, records, emit }) => {
    ipc.jobSnapshot = async () => { throw new Error("refresh unavailable"); };
    for (const [id, pending, answer, method] of [
      [1, jobs.pendingPassword, () => jobs.answerPassword("temporary-secret"), "answerPassword"],
      [2, jobs.pendingConflict, () => jobs.answerConflict("overwrite", true), "answerConflict"],
    ]) {
      const original = pending();
      ipc[method] = async () => { throw new Error("answer unavailable"); };
      answer();
      assert.equal(pending(), null);
      await until(() => jobs.tasks().find((task) => task.id === id).answeredQuestionVersion === 0);
      assert.equal(pending(), original, "restore input even when the follow-up read fails");
      assert.equal(jobs.tasks().find((task) => task.id === id).actionFailure, "answer");
      assert.doesNotMatch(JSON.stringify(jobs.tasks()), /temporary-secret/);
    }

    let reject;
    ipc.answerPassword = () => new Promise((_, fail) => { reject = fail; });
    jobs.answerPassword("temporary-secret");
    records[0].version = 12;
    records[0].question = { kind: "password", prompt: { id: 1, version: 12, name: "archive-1.zip", wrong: true } };
    ipc.jobSnapshot = async (id) => structuredClone(records.find((record) => record.id === id));
    await emit("job://ask-password", records[0].question.prompt);
    await until(() => jobs.pendingPassword()?.version === 12);
    assert.equal(jobs.tasks()[0].actionFailure, null);
    reject(new Error("late answer failure"));
    await until(() => jobs.tasks()[0].answeredQuestionVersion === 0);
    assert.equal(jobs.pendingPassword().version, 12);
    assert.equal(jobs.tasks()[0].actionFailure, null, "an old failure cannot annotate a newer prompt");

    jobs.answerPassword("temporary-secret");
    await emit("job://state", { id: 1, version: 13, state: "done", error: null });
    await until(() => jobs.tasks()[0].state === "done");
    reject(new Error("late answer failure"));
    await until(() => jobs.tasks()[0].answeredQuestionVersion === 0);
    assert.equal(jobs.pendingPassword(), null);
  });
});

test("question actions preserve the workspace and report success only after acknowledgement", async () => {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["submitTaskPasswordRequest", "cancelTaskPasswordRequest", "answerConflictDecision"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && [...names, "isCurrentTaskPasswordPrompt"].includes(node.name?.text));
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  for (const action of names) {
    for (const outcome of ["failure", "accepted", "next-question"]) {
      const calls = [];
      let finish;
      const context = {
        jobPasswordPrompt: action === "answerConflictDecision" ? null : { id: 1, version: 10 },
        jobConflictPrompt: action === "answerConflictDecision" ? { id: 1, version: 10 } : null,
        archivePasswordPrompt: null, jobPasswordValue: "temporary-secret", jobPasswordSubmissionAttempted: false,
        conflictApplyAll: true, taskPasswordReady: (value) => value.length > 0,
        normalizeTaskConflictAnswer: (decision, applyAll) => ({ decision, applyAll }),
        setScreen: (screen) => calls.push(["screen", screen]),
        returnTaskQuestionToCenter: (id) => calls.push(["center", id]),
        showNotice: (message) => calls.push(["notice", message]), tr: (_key, fallback) => fallback,
      };
      const answer = () => {
        context.jobPasswordPrompt = null;
        context.jobConflictPrompt = null;
        return new Promise((resolve) => { finish = resolve; });
      };
      context.answerJobPassword = answer;
      context.answerJobConflict = answer;
      const handlers = vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context);
      const request = action === "answerConflictDecision"
        ? handlers[action]("overwrite", true)
        : handlers[action](context.jobPasswordPrompt);
      assert.deepEqual(calls, [], "an unacknowledged answer cannot report success");
      if (action !== "answerConflictDecision") assert.equal(context.jobPasswordValue, "");
      if (outcome === "next-question") context.jobPasswordPrompt = { id: 2, version: 11 };
      finish(outcome !== "failure");
      await request;
      if (outcome === "failure") assert.deepEqual(calls, []);
      else if (outcome === "next-question") assert.deepEqual(calls.map(([kind]) => kind), ["notice"]);
      else assert.deepEqual(calls.map(([kind]) => kind), ["notice", "center"]);
    }
  }
});

test("job questions open the shared task surface without navigating or discarding a draft", () => {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const effects = source.statements.filter((node) =>
    ts.isExpressionStatement(node) && ts.isCallExpression(node.expression) &&
    node.expression.expression.getText(source) === "$effect"
  );
  const routing = effects.find((node) => node.getText(source).includes("setScreen(\"password\")"));
  const dialog = effects.find((node) => node.getText(source).includes("const questionTaskId"));
  assert.ok(routing && dialog);
  const { outputText } = ts.transpileModule([routing, dialog].map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  for (const screen of ["browse", "create", "recovery"]) {
    for (const kind of ["password", "conflict"]) {
      const draft = { output: "unfinished-archive.zip" };
      const context = {
        screen, pendingCreateSubmission: draft,
        jobPasswordPrompt: kind === "password" ? { id: 8 } : null,
        jobConflictPrompt: kind === "conflict" ? { id: 9 } : null,
        archivePasswordPrompt: null, previewPasswordPrompt: null,
        taskWindowMode: false, taskDialogTaskId: null, taskDialogDismissedId: 8,
        $effect: (effect) => effect(),
        setScreen: (next) => { context.screen = next; context.pendingCreateSubmission = null; },
      };
      vm.runInNewContext(outputText, context);
      assert.equal(context.screen, screen);
      assert.equal(context.pendingCreateSubmission, draft);
      assert.equal(context.taskDialogTaskId, kind === "password" ? 8 : 9);
      assert.equal(context.taskDialogDismissedId, null);
      for (const prompt of ["archivePasswordPrompt", "previewPasswordPrompt"]) {
        context[prompt] = { name: "opening.zip" };
        vm.runInNewContext(outputText, context);
        assert.equal(context.screen, "password", "archive opening retains its password workspace beneath the task dialog");
        context[prompt] = null;
      }
    }
  }
});
