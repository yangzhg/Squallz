import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { webcrypto } from "node:crypto";

import { createTestServer } from "../../tests/runtime.mjs";

const spec = { kind: "test", path: "/archives/reports.zip", password: null };
function snapshot(id, version, state = "running", done = 40) {
  return { id, version, spec, state, output_password_required: false, origin: "app",
    owned_by_requester: true, interaction: null, question: null, queue_position: null,
    queue_wait_reason: null, cpu_threads: 1, stream_buffer_limit_bytes: null,
    error: null, result: state === "done" ? jobResult("test") : null,
    progress: { done, total: 100, current: `entry-${done}.txt`, current_done: done,
      current_total: 100, speed: state === "running" ? 64 : 0,
      phase: "archive_test", interruptible: true } };
}
function live(record) {
  const { spec: _description, ...update } = record;
  return update;
}

async function withJobEvents(run) {
  const server = await createTestServer();
  const previousWindow = globalThis.window;
  globalThis.window = { crypto: webcrypto };
  let dispose;
  let finishPoll;
  let notices;
  try {
    const { mockIPC, mockWindows } = await server.ssrLoadModule("@tauri-apps/api/mocks");
    const { emit } = await server.ssrLoadModule("@tauri-apps/api/event");
    mockWindows("main");
    const nativeCalls = [];
    mockIPC((command, args) => { nativeCalls.push({ command, args }); }, { shouldMockEvents: true });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    notices = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    await loadLocale("en-US");
    ipc.jobSnapshots = () => new Promise((resolve) => { finishPoll = resolve; });
    dispose = await jobs.initJobEvents();
    await run({ server, jobs, ipc, notices, nativeCalls, mockIPC,
      publish: (record) => emit("job://snapshot", live(record)),
      recover: async (delta) => { finishPoll(delta); await new Promise((resolve) => setTimeout(resolve, 0)); } });
  } finally {
    dispose?.();
    finishPoll?.({ revision: 0, reset: true, upserts: [], removed: [] });
    while (notices?.toasts().length) notices.dismissToast(notices.toasts()[0].id);
    globalThis.window = previousWindow;
    await server.close();
  }
}

const createSpec = {
  kind: "compress", inputs: ["/input/report.txt"], dest: "/output/archive.zip", level: 6,
  password: null, encrypt_names: false, split_size: null, split_mode: "generic",
  excludes: [], content_policy: "keep_all_files", sqz_inner_format: null, sfx_target: null,
  replace_existing: false, replacement_guard: null, completion: "open_in_squallz",
  post_success: "trash_source", test_after_create: false,
};

test("presented task results suppress duplicate notices while completion, recovery and history still run", () =>
  withJobEvents(async ({ server, jobs, ipc, notices, publish, nativeCalls }) => {
    const { operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    await server.ssrLoadModule("@tauri-apps/plugin-opener");
    const opened = [];
    let recoveryRefreshes = 0;
    jobs.setCreateCompletionHandler((path) => { opened.push(path); return true; });
    jobs.setSourceCleanupRecoveryRefreshHandler(() => { recoveryRefreshes += 1; });
    ipc.submitJob = async () => ({ ...snapshot(71, 1), spec: createSpec });
    await jobs.submitJob(createSpec);
    jobs.setPresentedTaskId(71);
    const completed = { ...snapshot(71, 2, "done"), result: jobResult("compress", {
      source_cleanup: { status: "partial", moved: 1, kept: 1, recovery_required: 1 },
    }) };
    await publish(completed);
    assert.equal(notices.toasts().length, 0);
    assert.deepEqual(opened, ["/output/archive.zip"]);
    assert.equal(recoveryRefreshes, 1);
    assert.equal(operationHistory().length, 1);
    assert.equal(jobs.tasks().find((task) => task.id === 71).revealPath, "/output/archive.zip");
    jobs.setPresentedTaskId(null);
    await publish(completed);
    assert.equal(notices.toasts().length, 0, "closing details does not replay a consumed result");
    assert.equal(recoveryRefreshes, 1);
    assert.equal(operationHistory().length, 1);

    ipc.submitJob = async () => snapshot(72, 1);
    await jobs.submitJob(spec);
    jobs.setPresentedTaskId(72);
    await publish(snapshot(72, 2, "cancelled"));
    assert.equal(notices.toasts().length, 0);

    const extractSpec = { kind: "extract", path: "/input/report.zip", dest: "/output", password: null,
      expected_destination: null, expected_input_guard: null, selection: null, overwrite: "ask",
      symlinks: "skip", smart: false, encoding: null, verify_sfx: true, best_effort: false };
    ipc.submitJob = async () => ({ ...snapshot(73, 1), spec: extractSpec });
    await jobs.submitJob(extractSpec);
    jobs.setPresentedTaskId(73);
    jobs.setRevealAfterExtractPreference(true);
    await publish({ ...snapshot(73, 2, "done"), result: jobResult("extract") });
    const revealCalls = () => nativeCalls.filter((call) => call.command === "plugin:opener|reveal_item_in_dir");
    for (let attempt = 0; attempt < 50 && revealCalls().length === 0; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 2));
    }
    assert.deepEqual(revealCalls(), [{ command: "plugin:opener|reveal_item_in_dir", args: { paths: ["/output"] } }]);
    assert.equal(notices.toasts().length, 0);
    assert.equal(operationHistory().length, 2);
  }));

test("opening task details consumes visible and queued result notices without clearing unrelated errors", () =>
  withJobEvents(async ({ server, jobs, ipc, notices }) => {
    const { platformFileManagerName } = await server.ssrLoadModule("/src/lib/platform-labels.ts");
    const { t } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    ipc.submitJob = async () => ({ ...snapshot(81, 1, "done"),
      spec: { ...createSpec, completion: "none" },
      result: jobResult("compress", { source_cleanup: { status: "completed", moved: 1 } }),
    });
    jobs.setPresentedTaskId(80);
    await jobs.submitJob(createSpec);
    assert.deepEqual(notices.toasts().map((toast) => toast.key), ["task:81:result", "task:81:source-cleanup"]);
    assert.equal(notices.toasts()[0].action.label,
      t("gui.task.show_in_file_manager", { fileManager: platformFileManagerName() }));
    jobs.setPresentedTaskId(81);
    assert.equal(notices.toasts().length, 0);
    jobs.setPresentedTaskId(null);

    for (let id = 1; id <= 3; id += 1) notices.pushToast({ kind: "danger", title: `urgent ${id}` });
    ipc.submitJob = async () => ({ ...snapshot(82, 1, "done"), spec: { ...createSpec, completion: "none" },
      result: jobResult("compress", { source_cleanup: { status: "partial", kept: 1 } }),
    });
    await jobs.submitJob(createSpec);
    notices.pushToast({ kind: "warning", title: "Could not open output", key: "task:82:output-error" });
    ipc.submitJob = async () => snapshot(83, 1, "cancelled");
    await jobs.submitJob(spec);
    jobs.setPresentedTaskId(82);
    const remaining = [];
    while (notices.toasts().length) {
      remaining.push(notices.toasts()[0]);
      notices.dismissToast(notices.toasts()[0].id);
    }
    assert.ok(remaining.some((toast) => toast.key === "task:82:output-error"));
    assert.ok(remaining.some((toast) => toast.key === "task:83:result"));
    assert.ok(remaining.every((toast) => !["task:82:result", "task:82:source-cleanup"].includes(toast.key)));
  }));

test("a foreground completion action failure remains visible after result notices are consumed", () =>
  withJobEvents(async ({ server, jobs, ipc, notices, mockIPC }) => {
    await server.ssrLoadModule("@tauri-apps/plugin-opener");
    const { t } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    mockIPC((command) => {
      if (command === "plugin:opener|reveal_item_in_dir") throw new Error("File manager unavailable");
    }, { shouldMockEvents: true });
    const revealSpec = { ...createSpec, completion: "reveal_output", post_success: "keep_source" };
    ipc.submitJob = async () => ({ ...snapshot(91, 1, "done"), spec: revealSpec, result: jobResult("compress") });
    jobs.setPresentedTaskId(91);
    await jobs.submitJob(revealSpec);
    for (let attempt = 0; attempt < 50 && notices.toasts().length === 0; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 2));
    }
    jobs.setPresentedTaskId(91);
    assert.equal(notices.toasts().length, 1);
    assert.equal(notices.toasts()[0].title, t("gui.toast.reveal_failed"));
    assert.equal(notices.toasts()[0].key, undefined);
  }));

test("submission snapshots preserve progress and final states when live updates precede registration", () =>
  withJobEvents(async ({ jobs, ipc, publish }) => {
    for (const [index, state] of ["done", "failed", "cancelled", "running", "paused"].entries()) {
      const id = index + 1;
      ipc.submitJob = async () => {
        await publish(snapshot(id, 2, "running", 20));
        await publish(snapshot(id, 3, state));
        return snapshot(id, 3, state);
      };
      await jobs.submitJob(spec);
      const task = jobs.tasks().find((item) => item.id === id);
      assert.equal(task.state, state);
      assert.equal(task.version, 3);
      assert.equal(task.current, "entry-40.txt");
      assert.equal(task.done, 40);
      assert.equal(task.currentDone, 40);
      assert.equal(task.currentTotal, 100);
      assert.equal(task.speed, state === "running" ? 64 : 0);
      assert.equal(task.localEffects, true);
    }
    ipc.submitJob = async () => {
      await publish(snapshot(6, 6, "cancelled", 80));
      await publish(snapshot(6, 2, "running", 20));
      return snapshot(6, 6, "cancelled", 80);
    };
    await jobs.submitJob(spec);
    const latest = jobs.tasks().find((item) => item.id === 6);
    assert.equal(latest.state, "cancelled");
    assert.equal(latest.version, 6);
    assert.equal(latest.current, "entry-80.txt");
    assert.equal(latest.speed, 0);
    await publish(snapshot(6, 7, "running", 100));
    assert.equal(latest.current, "entry-80.txt", "terminal tasks reject later nonterminal updates");
  }));

test("one snapshot reducer rejects reordered and duplicate delivery and runs local terminal effects once", () =>
  withJobEvents(async ({ server, jobs, ipc, publish, notices }) => {
    const { operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    let nextId = 0;
    ipc.submitJob = async () => snapshot(++nextId, 1, "queued", 0);
    for (const state of ["done", "failed", "cancelled"]) {
      const id = await jobs.submitJob(spec);
      const task = jobs.tasks().find((item) => item.id === id);
      await publish(snapshot(id, 3, "running", 30));
      await publish(snapshot(id, 2, "running", 20));
      assert.equal(task.state, "running");
      assert.equal(task.version, 3);
      await publish(snapshot(id, 5, state, 80));
      const historyCount = operationHistory().length;
      const toastCount = notices.toasts().length;
      await publish(snapshot(id, 4, "running", 60));
      await publish(snapshot(id, 5, state, 80));
      await publish(snapshot(id, 6, "running", 100));
      assert.equal(task.current, "entry-80.txt", "a full terminal snapshot already contains its last measurement");
      assert.equal(task.done, 80);
      assert.equal(task.currentDone, 80);
      assert.equal(task.state, state);
      assert.equal(task.version, 5);
      assert.equal(task.speed, 0);
      assert.equal(operationHistory().length, historyCount);
      assert.equal(notices.toasts().length, toastCount);
    }
    const id = await jobs.submitJob(spec);
    const task = jobs.tasks().find((item) => item.id === id);
    await publish(snapshot(id, 4, "paused", 30));
    await publish(snapshot(id, 3, "running", 20));
    assert.equal(task.current, "entry-30.txt");
    assert.equal(task.speed, 0);
    await publish(snapshot(id, 6, "running", 60));
    await publish(snapshot(id, 5, "running", 50));
    assert.equal(task.state, "running");
    assert.equal(task.version, 6);
    assert.equal(task.done, 60);
    assert.equal(task.speed, 64);
  }));

test("recovery descriptions and local promotion survive a terminal event before submit returns", () =>
  withJobEvents(async ({ server, jobs, ipc, publish, recover, notices }) => {
    const { operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    ipc.submitJob = async () => {
      await recover({ revision: 8, reset: true, upserts: [snapshot(7, 8, "running", 60)], removed: [] });
      await publish(snapshot(7, 9, "done", 80));
      assert.equal(jobs.tasks()[0].localEffects, false);
      assert.equal(notices.toasts().length, 0);
      assert.equal(operationHistory().length, 0);
      return snapshot(7, 8, "running", 60); // The event overtook the command response.
    };
    await jobs.submitJob(spec);
    const task = jobs.tasks()[0];
    assert.equal(task.version, 9);
    assert.equal(task.state, "done");
    assert.equal(task.current, "entry-80.txt");
    assert.equal(task.localEffects, true);
    assert.equal(task.historyRecorded, true);
    const historyCount = operationHistory().length;
    const toastCount = notices.toasts().length;
    assert.equal(historyCount, 1);
    assert.ok(toastCount > 0);
    await publish(snapshot(7, 9, "done", 80));
    assert.equal(operationHistory().length, historyCount);
    assert.equal(notices.toasts().length, toastCount);
  }));

test("unknown descriptionless live updates recover through the authoritative delta without buffering", () =>
  withJobEvents(async ({ jobs, publish, recover }) => {
    await publish(snapshot(21, 4, "running", 40));
    assert.equal(jobs.tasks().length, 0);
    await recover({ revision: 5, reset: true, upserts: [snapshot(21, 5, "paused", 50)], removed: [] });
    assert.equal(jobs.tasks().length, 1);
    assert.equal(jobs.tasks()[0].state, "paused");
    await publish(snapshot(21, 3, "running", 30));
    assert.equal(jobs.tasks()[0].done, 50);
    await publish(snapshot(21, 6, "running", 60));
    assert.equal(jobs.tasks()[0].done, 60);
    assert.deepEqual(JSON.parse(JSON.stringify(jobs.tasks()[0].spec)), spec);
  }));

test("an older in-flight reset cannot remove a task registered or updated after its revision", () =>
  withJobEvents(async ({ server, jobs, ipc, publish, recover }) => {
    const { operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    ipc.submitJob = async () => snapshot(31, 5, "running", 50);
    await jobs.submitJob(spec);
    const task = jobs.tasks()[0];
    await recover({ revision: 4, reset: true, upserts: [], removed: [] });
    assert.equal(jobs.tasks()[0], task);
    assert.equal(task.localEffects, true);
    await publish(snapshot(31, 6, "done", 100));
    assert.equal(task.state, "done");
    assert.equal(operationHistory().length, 1);
  }));

test("invalid live results preserve the last state and retry the same completion revision", () =>
  withJobEvents(async ({ jobs, ipc, publish, notices }) => {
    ipc.submitJob = async () => snapshot(41, 1, "running", 40);
    await jobs.submitJob(spec);
    const task = jobs.tasks()[0];
    const invalid = snapshot(41, 2, "done", 100);
    delete invalid.result.entries_tested;
    await publish(invalid);
    assert.equal(task.version, 1);
    assert.equal(task.state, "running");
    assert.equal(task.statusStale, true);
    assert.equal(notices.toasts().length, 0);
    await publish(snapshot(41, 2, "done", 100));
    assert.equal(task.version, 2);
    assert.equal(task.state, "done");
    assert.equal(task.result.entries_tested, 1);
    assert.equal(task.statusStale, false);
  }));

test("a malformed recovery delta cannot remove or partially advance visible tasks", () =>
  withJobEvents(async ({ jobs, ipc, recover }) => {
    ipc.submitJob = async () => snapshot(51, 1, "running", 40);
    await jobs.submitJob(spec);
    const invalid = snapshot(52, 2, "done", 100);
    invalid.result = jobResult("checksum");
    await recover({ revision: 2, reset: true, removed: [51], upserts: [snapshot(53, 2), invalid] });
    assert.deepEqual(jobs.tasks().map((task) => task.id), [51]);
    assert.equal(jobs.tasks()[0].version, 1);
    assert.equal(jobs.tasks()[0].statusStale, true);
  }));

test("snapshot versions reject stale updates and terminal regressions", async () => {
  const server = await createTestServer();
  try {
    const { isTerminalSnapshotState, localSubmissionPromotion, shouldApplyFullSnapshot } =
      await server.ssrLoadModule("/src/lib/job-snapshot.ts");
    assert.equal(isTerminalSnapshotState("done"), true);
    assert.equal(isTerminalSnapshotState("cancelled"), true);
    assert.equal(isTerminalSnapshotState("running"), false);
    assert.equal(shouldApplyFullSnapshot(5, "queued", 6, "running"), true);
    assert.equal(shouldApplyFullSnapshot(6, "running", 5, "running"), false);
    assert.equal(shouldApplyFullSnapshot(8, "done", 8, "running"), false);
    assert.deepEqual(localSubmissionPromotion("running", false), { resetHistory: true, replayTerminal: false });
    assert.deepEqual(localSubmissionPromotion("done", false), { resetHistory: true, replayTerminal: true });
    assert.deepEqual(localSubmissionPromotion("done", true), { resetHistory: false, replayTerminal: false });
  } finally { await server.close(); }
});
