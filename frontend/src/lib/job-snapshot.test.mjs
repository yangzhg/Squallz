import assert from "node:assert/strict";
import test from "node:test";
import { webcrypto } from "node:crypto";

import { createTestServer } from "../../tests/runtime.mjs";

test("buffered job events preserve the latest progress and state before submission returns", async () => {
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
    mockIPC(() => {}, { shouldMockEvents: true });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    notices = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    await loadLocale("en-US");
    ipc.jobSnapshots = () => new Promise((resolve) => { finishPoll = resolve; });
    dispose = await jobs.initJobEvents();
    const progress = (id, version = 2, current = "reports/final.txt") => ({ id, version,
      done: 40, total: 100, current, current_done: 4, current_total: 10,
      speed: 64, phase: "extract_entries", interruptible: true });
    const states = ["done", "failed", "cancelled", "running", "paused"];
    for (const [index, state] of states.entries()) {
      const id = index + 1;
      ipc.submitJob = async () => {
        await emit("job://progress", progress(id, state === "running" ? 3 : 2));
        await emit("job://state", { id, version: state === "running" ? 2 : 3, state,
          result: state === "done" ? { ok: true } : null });
        return id;
      };
      await jobs.submitJob({ kind: "test", path: "/archives/reports.zip", password: null });
      const task = jobs.tasks().find((item) => item.id === id);
      assert.equal(task.state, state);
      assert.equal(task.version, 3);
      assert.equal(task.current, "reports/final.txt");
      assert.equal(task.done, 40);
      assert.equal(task.currentDone, 4);
      assert.equal(task.currentTotal, 10);
      assert.equal(task.speed, state === "running" ? 64 : 0);
    }
    ipc.submitJob = async () => {
      await emit("job://progress", progress(6, 5));
      await emit("job://progress", progress(6, 4, "reports/older.txt"));
      await emit("job://state", { id: 6, version: 6, state: "cancelled" });
      await emit("job://state", { id: 6, version: 2, state: "running" });
      return 6;
    };
    await jobs.submitJob({ kind: "test", path: "/archives/reports.zip", password: null });
    const latest = jobs.tasks().find((item) => item.id === 6);
    assert.equal(latest.state, "cancelled");
    assert.equal(latest.version, 6);
    assert.equal(latest.current, "reports/final.txt");
    assert.equal(latest.speed, 0);
    await emit("job://progress", progress(6, 7, "late.txt"));
    assert.equal(latest.current, "reports/final.txt", "terminal tasks still reject later progress");

    const spec = { kind: "test", path: "/archives/reports.zip", password: null };
    ipc.jobSnapshot = async () => ({ id: 7, version: 9, spec, state: "failed",
      output_password_required: false, origin: "app", owned_by_requester: true,
      interaction: null, question: null, queue_position: null, queue_wait_reason: null,
      cpu_threads: 1, stream_buffer_limit_bytes: null, error: null, result: null,
      progress: { done: 80, total: 100, current: "snapshot.txt", current_done: 8,
        current_total: 10, speed: 0, phase: "archive_test", interruptible: true } });
    ipc.submitJob = async () => {
      await emit("job://progress", progress(7));
      await emit("job://state", { id: 7, version: 3, state: "cancelled" });
      await emit("job://ask-password", { id: 7 });
      for (let attempt = 0; attempt < 100 && !jobs.tasks().some((task) => task.id === 7); attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.ok(jobs.tasks().some((task) => task.id === 7), "the snapshot arrives before registration");
      return 7;
    };
    await jobs.submitJob(spec);
    const restored = jobs.tasks().find((task) => task.id === 7);
    assert.equal(restored.version, 9);
    assert.equal(restored.state, "failed");
    assert.equal(restored.current, "snapshot.txt", "older buffered events cannot replace authoritative progress");
    assert.equal(restored.currentDone, 8);
    assert.equal(restored.localEffects, true);
    assert.equal(restored.question, null);
  } finally {
    dispose?.();
    finishPoll?.({ revision: 0, reset: true, upserts: [], removed: [] });
    while (notices?.toasts().length) notices.dismissToast(notices.toasts()[0].id);
    globalThis.window = previousWindow;
    await server.close();
  }
});

test("snapshot versions reject stale updates and terminal regressions", async () => {
  const server = await createTestServer();

  try {
    const {
      isTerminalSnapshotState,
      localSubmissionPromotion,
      shouldApplyFullSnapshot,
      shouldApplySnapshotProgress,
      shouldApplySnapshotState,
    } = await server.ssrLoadModule("/src/lib/job-snapshot.ts");

    assert.equal(isTerminalSnapshotState("done"), true);
    assert.equal(isTerminalSnapshotState("cancelled"), true);
    assert.equal(isTerminalSnapshotState("running"), false);

    assert.equal(shouldApplySnapshotState(4, "running", 5, "paused"), true);
    assert.equal(shouldApplySnapshotState(5, "running", 5, "paused"), false);
    assert.equal(shouldApplySnapshotState(8, "done", 9, "running"), false);
    assert.equal(shouldApplySnapshotState(8, "done", 9, "done"), true);

    assert.equal(shouldApplySnapshotProgress(4, "running", 5), true);
    assert.equal(shouldApplySnapshotProgress(5, "running", 5), false);
    assert.equal(shouldApplySnapshotProgress(8, "failed", 9), false);

    assert.equal(shouldApplyFullSnapshot(5, "queued", 5, "running"), true);
    assert.equal(shouldApplyFullSnapshot(6, "running", 5, "running"), false);
    assert.equal(shouldApplyFullSnapshot(8, "done", 8, "running"), false);

    assert.deepEqual(localSubmissionPromotion("running", false), {
      resetHistory: true,
      replayTerminal: false,
    });
    assert.deepEqual(localSubmissionPromotion("done", false), {
      resetHistory: true,
      replayTerminal: true,
    });
    assert.deepEqual(localSubmissionPromotion("done", true), {
      resetHistory: false,
      replayTerminal: false,
    });

    let replayState = "queued";
    let replayVersion = 0;
    if (shouldApplySnapshotState(replayVersion, replayState, 2, "running")) {
      replayState = "running";
      replayVersion = 2;
    }
    if (shouldApplySnapshotProgress(replayVersion, replayState, 3)) replayVersion = 3;
    assert.deepEqual({ state: replayState, version: replayVersion }, {
      state: "running",
      version: 3,
    });
  } finally {
    await server.close();
  }
});
