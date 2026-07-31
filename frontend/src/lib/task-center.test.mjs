import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("deferred component loaders cache success and retry failed imports", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { createDeferredComponentLoader } = await server.ssrLoadModule(
      "/src/lib/deferred-component.ts",
    );
    let calls = 0;
    const loader = createDeferredComponentLoader(async () => {
      calls += 1;
      if (calls === 1) throw new Error("chunk unavailable");
      return { default: "ready" };
    });

    await assert.rejects(loader.load(), /chunk unavailable/);
    await assert.rejects(loader.load(), /chunk unavailable/);
    assert.equal(calls, 1);
    assert.equal(await loader.retry(), "ready");
    assert.equal(await loader.load(), "ready");
    assert.equal(calls, 2);
  } finally {
    await server.close();
  }
});

function task(id, state, overrides = {}) {
  return {
    id,
    version: 1,
    spec: {
      kind: "compress",
      inputs: [`/tmp/source-${id}`],
      dest: `/tmp/archive-${id}.sqz`,
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: null,
      split_mode: "generic",
      excludes: [],
      replace_existing: false,
      completion: "none",
      post_success: "keep_source",
    },
    title: `Task ${id}`,
    origin: "app",
    ownedByRequester: true,
    interaction: null,
    state,
    queuePosition: null,
    done: 0,
    total: 0,
    current: "",
    currentDone: 0,
    currentTotal: 0,
    scanEntries: null,
    speed: 0,
    phase: null,
    interruptible: true,
    pausable: true,
    error: null,
    result: state === "done" ? {} : null,
    revealPath: null,
    historyRecorded: state === "done" || state === "failed",
    localEffects: false,
    snapshotSeen: true,
    controlIntent: null,
    queueMoveIntent: null,
    expanded: false,
    ...overrides,
  };
}

test("task center keeps attention visible and follows authoritative waiting order", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const {
      clearableTaskIds,
      taskCenterActionableCount,
      taskCenterCounts,
      taskCenterRows,
      taskQueueDropBeforeId,
      taskQueuePosition,
      taskSubmissionBlockReason,
    } = await server.ssrLoadModule("/src/lib/task-center.ts");

    const rows = [
      task(11, "done"),
      task(12, "running", {
        interaction: "password",
        origin: "file_manager",
        ownedByRequester: false,
      }),
      task(13, "queued", { queuePosition: 2 }),
      task(14, "queued", { queuePosition: 1 }),
      task(15, "failed", { error: { key: "error.io", params: {}, detail: "failed" } }),
      task(16, "done", {
        result: {
          operation: "create",
          primary_output: "/tmp/archive-16.sqz",
          outputs: ["/tmp/archive-16.sqz"],
          preserved_outputs: ["/tmp/archive-16.sqz.previous"],
          total_bytes: 10,
          volume_count: 1,
          split: false,
        },
      }),
      task(17, "cancelled"),
    ];

    assert.deepEqual(taskCenterRows(rows).map((row) => row.id), [12, 16, 15, 14, 13, 11, 17]);
    assert.deepEqual(taskCenterCounts(rows), {
      active: 1,
      waiting: 2,
      attention: 3,
      completed: 4,
      clearable: 2,
      total: 7,
    });
    assert.equal(taskCenterActionableCount(rows), 5);
    assert.equal(taskCenterActionableCount([rows[1]]), 1);
    assert.deepEqual(taskQueuePosition(rows, 13), {
      position: 2,
      aheadInQueue: 2,
      canMoveEarlier: true,
      canMoveLater: false,
    });
    assert.deepEqual(taskQueuePosition(rows, 14), {
      position: 1,
      aheadInQueue: 1,
      canMoveEarlier: false,
      canMoveLater: true,
    });
    assert.equal(taskQueuePosition(rows, 12), null);
    assert.equal(
      taskQueuePosition([...rows, task(18, "queued", { queuePosition: null })], 18),
      null,
    );
    assert.equal(taskQueueDropBeforeId(rows, 13, 14, "before"), 14);
    assert.equal(taskQueueDropBeforeId(rows, 13, 14, "after"), undefined);
    assert.equal(taskQueueDropBeforeId(rows, 14, 13, "after"), null);
    assert.equal(taskQueueDropBeforeId(rows, 12, 14, "before"), undefined);
    assert.deepEqual(clearableTaskIds(rows), [11, 17]);
    assert.deepEqual(rows.map((row) => row.id), [11, 12, 13, 14, 15, 16, 17]);

    assert.equal(
      taskSubmissionBlockReason({
        submitInFlight: true,
        taskWindowMode: false,
        hasActiveTask: false,
        replacesExistingOutput: false,
      }),
      "starting",
    );
    assert.equal(
      taskSubmissionBlockReason({
        submitInFlight: false,
        taskWindowMode: true,
        hasActiveTask: true,
        replacesExistingOutput: false,
      }),
      "task-window-busy",
    );
    assert.equal(
      taskSubmissionBlockReason({
        submitInFlight: false,
        taskWindowMode: false,
        hasActiveTask: true,
        replacesExistingOutput: true,
      }),
      "replace-existing",
    );
    assert.equal(
      taskSubmissionBlockReason({
        submitInFlight: false,
        taskWindowMode: false,
        hasActiveTask: true,
        replacesExistingOutput: false,
      }),
      null,
    );
  } finally {
    await server.close();
  }
});
