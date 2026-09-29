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
      assert.equal(jobs.tasks().find((task) => task.id === id).questionFailure, "cancel");
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
    assert.equal(task.questionFailure, null);
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
      assert.equal(jobs.tasks().find((task) => task.id === id).questionFailure, "answer");
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
    assert.equal(jobs.tasks()[0].questionFailure, null);
    reject(new Error("late answer failure"));
    await until(() => jobs.tasks()[0].answeredQuestionVersion === 0);
    assert.equal(jobs.pendingPassword().version, 12);
    assert.equal(jobs.tasks()[0].questionFailure, null, "an old failure cannot annotate a newer prompt");

    jobs.answerPassword("temporary-secret");
    await emit("job://state", { id: 1, version: 13, state: "done", error: null });
    await until(() => jobs.tasks()[0].state === "done");
    reject(new Error("late answer failure"));
    await until(() => jobs.tasks()[0].answeredQuestionVersion === 0);
    assert.equal(jobs.pendingPassword(), null);
  });
});

test("question actions report success and leave the prompt only after acknowledgement", async () => {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["submitPasswordRequest", "cancelPasswordRequest", "answerConflictDecision"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
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
        archivePasswordPrompt: null, jobPasswordValue: "temporary-secret", passwordSubmissionAttempted: false,
        conflictApplyAll: true, jobQuestionReturnScreen: () => "extract", taskPasswordReady: (value) => value.length > 0,
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
      const request = handlers[action]("overwrite", true);
      assert.deepEqual(calls, [], "an unacknowledged answer cannot report success");
      if (action !== "answerConflictDecision") assert.equal(context.jobPasswordValue, "");
      if (outcome === "next-question") context.jobPasswordPrompt = { id: 2, version: 11 };
      finish(outcome !== "failure");
      await request;
      if (outcome === "failure") assert.deepEqual(calls, []);
      else if (outcome === "next-question") assert.deepEqual(calls.map(([kind]) => kind), ["notice"]);
      else assert.deepEqual(calls.map(([kind]) => kind), ["notice", "screen", "center"]);
    }
  }
});
