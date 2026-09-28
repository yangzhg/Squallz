import assert from "node:assert/strict";
import test from "node:test";
import { webcrypto } from "node:crypto";
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
    assert.equal(jobs.tasks()[0].question, null, "an in-flight answer is not reopened by an older snapshot");

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

test("snapshot questions enforce ownership, terminal state and answer identity", async () => {
  const server = await createTestServer();
  try {
    const { snapshotQuestion } = await server.ssrLoadModule("/src/lib/job-snapshot.ts");
    const waiting = snapshot(1, 5);
    assert.equal(snapshotQuestion(waiting, 0), waiting.question);
    assert.equal(snapshotQuestion({ ...waiting, owned_by_requester: false }, 0), null);
    assert.equal(snapshotQuestion({ ...waiting, state: "done" }, 0), null);
    assert.equal(snapshotQuestion(waiting, 5), null);
    assert.equal(snapshotQuestion(waiting, 4), waiting.question);
    assert.equal(snapshotQuestion({ ...waiting, question: null }, 0), null);
  } finally {
    await server.close();
  }
});
