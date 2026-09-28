import assert from "node:assert/strict";
import test from "node:test";
import { webcrypto } from "node:crypto";
import { createTestServer } from "../../tests/runtime.mjs";

test("task registration and window snapshots retain output protection without retaining passwords", async () => {
  const server = await createTestServer();
  const previousWindow = globalThis.window;
  globalThis.window = { crypto: webcrypto };
  let dispose;
  try {
    const { mockIPC, mockWindows } = await server.ssrLoadModule("@tauri-apps/api/mocks");
    mockWindows("main");
    mockIPC(() => {}, { shouldMockEvents: true });
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    let nextId = 0;
    ipc.submitJob = async () => ++nextId;
    const convert = { kind: "convert", src: "/source.7z", dest: "/output.zip", level: 5,
      src_encoding: null, src_password: "source-secret", dest_password: "output-secret",
      encrypt_names: false, split_size: null, split_mode: "generic",
      replace_existing: false, replacement_guard: "opaque-guard" };
    const inputs = [
      [convert, true],
      [{ ...convert, dest_password: null }, false],
      [{ ...convert, dest_password: null, encrypt_names: true }, true],
      [{ kind: "compress", inputs: ["/input"], dest: "/output.zip", level: 5,
        password: "output-secret", encrypt_names: false, replacement_guard: "opaque-guard" }, true],
      [{ kind: "export_sqz", src: "/source.sqz", dest: "/output.zip", level: 5,
        dest_password: "output-secret", replacement_guard: "opaque-guard" }, true],
    ];
    for (const [spec, required] of inputs) {
      const id = await jobs.submitJob(spec);
      const task = jobs.tasks().find((item) => item.id === id);
      assert.equal(task.outputPasswordRequired, required);
      assert.doesNotMatch(JSON.stringify(task), /source-secret|output-secret|opaque-guard/);
    }
    const snapshot = (id, required, version = 1) => ({
      id, version, spec: { ...convert, src_password: null, dest_password: null, replacement_guard: null },
      output_password_required: required, origin: "file_manager", owned_by_requester: false,
      state: "queued", queue_position: null, queue_wait_reason: null, cpu_threads: 1,
      stream_buffer_limit_bytes: null, progress: { done: 0, total: 0, current: "", current_done: 0,
        current_total: 0, speed: 0, interruptible: true }, error: null, result: null,
      interaction: null, question: null,
    });
    const records = [snapshot(1, true), snapshot(20, true), snapshot(21, false)];
    ipc.jobSnapshots = async () => ({ revision: 1, reset: true, upserts: records, removed: [] });
    dispose = await jobs.initJobEvents();
    for (let attempt = 0; attempt < 100 && !jobs.tasks().some((task) => task.id === 21); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    for (const [id, required] of [[1, true], [20, true], [21, false]]) {
      const task = jobs.tasks().find((item) => item.id === id);
      assert.equal(task.outputPasswordRequired, required);
      assert.equal(task.spec.dest_password, null);
      assert.equal(task.snapshotSeen, true);
    }
    assert.doesNotMatch(JSON.stringify(jobs.tasks()), /source-secret|output-secret|opaque-guard/);
  } finally {
    dispose?.();
    globalThis.window = previousWindow;
    await server.close();
  }
});
