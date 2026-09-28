import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

test("window recovery reconnects a task without replaying an external launch", async () => {
  const server = await createTestServer();
  try {
    const { readWindowRecovery, windowRecoveryUrl } = await server.ssrLoadModule("/src/lib/window-recovery.ts");
    const { taskWindowLaunchStateFromParams } = await server.ssrLoadModule("/src/lib/task-window.ts");
    const original = "http://tauri.localhost/?externalTask=extract-here&externalPath=%2Ftmp%2Fa.zip&externalPath=%2Ftmp%2Fb.zip&externalOutput=%2Ftmp%2Fout&theme=dark";
    const originalParams = new URL(original).searchParams;
    assert.ok(taskWindowLaunchStateFromParams(originalParams).launch);
    const recovered = new URL(windowRecoveryUrl(original, { taskId: 42 }));
    assert.deepEqual(readWindowRecovery(recovered.searchParams), { taskId: 42, taskCenter: false });
    const launch = taskWindowLaunchStateFromParams(recovered.searchParams);
    assert.equal(launch.mode, true);
    assert.equal(launch.launch, null);
    assert.equal(launch.pendingAction, null);
    for (const name of ["externalTask", "externalPath", "externalOutput"]) assert.equal(recovered.searchParams.has(name), false);
    assert.equal(recovered.searchParams.get("theme"), "dark");
    assert.equal(originalParams.getAll("externalPath").length, 2);
  } finally {
    await server.close();
  }
});

test("main-window recovery reopens the task center and clears stale task targets", async () => {
  const server = await createTestServer();
  try {
    const { readWindowRecovery, windowRecoveryUrl } = await server.ssrLoadModule("/src/lib/window-recovery.ts");
    const url = new URL(windowRecoveryUrl("tauri://localhost/?mode=classic&recoveryTask=19", { taskCenter: true }));
    assert.deepEqual(readWindowRecovery(url.searchParams), { taskId: null, taskCenter: true });
    assert.equal(url.searchParams.has("taskWindow"), false);
    assert.equal(url.searchParams.get("mode"), "classic");
    for (const taskId of [-1, 0, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      const invalid = new URL(windowRecoveryUrl(url.href, { taskId }));
      assert.deepEqual(readWindowRecovery(invalid.searchParams), { taskId: null, taskCenter: false });
    }
    for (const value of ["", "invalid", "-1", "1.2", "9007199254740992"]) {
      assert.equal(readWindowRecovery(new URLSearchParams({ recoveryTask: value })).taskId, null);
    }
  } finally {
    await server.close();
  }
});
