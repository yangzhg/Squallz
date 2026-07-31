import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("snapshot versions reject stale updates and terminal regressions", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

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
