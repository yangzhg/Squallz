import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer, englishTranslate } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { runArchiveDiskPreflight } = await server.ssrLoadModule("/src/lib/archive-disk-preflight.ts");
const { authorizeArchiveOutput, refreshArchiveOutputAuthorization, DestinationInspectionError } =
  await server.ssrLoadModule("/src/lib/archive-output-authorization.ts");

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function diskOptions(events, isCurrent = () => true) {
  return { destinationDirectory: "/output", isCurrent,
    onPhase: (phase) => events.push(phase),
    onTempDisk: () => events.push("workspace"),
    onSystemTempDisk: () => events.push("system"),
    onDestinationDisk: () => events.push("destination") };
}
const plan = { workspace_budget_bytes: 20, system_temp_budget_bytes: 30, final_output_budget_bytes: 10 };

test("disk failures preserve their stage and stop before later allocations", async (t) => {
  const original = { checkDiskSpace: ipc.checkDiskSpace, tempDir: ipc.tempDir };
  t.after(() => Object.assign(ipc, original));
  for (const [budget, stage, code] of [[20, "temp", "workspace_space"], [30, "temp", "system_temp_space"],
    [10, "destination", "destination_space"]]) {
    for (const serviceFailure of [false, true]) {
      const checks = [], events = [];
      ipc.tempDir = async () => "/system";
      ipc.checkDiskSpace = async (path, requested) => {
        checks.push([path, requested]);
        if (requested === budget && serviceFailure) throw new Error("unavailable");
        return { ok: requested !== budget, available_bytes: 5 };
      };
      const outcome = await runArchiveDiskPreflight(plan, diskOptions(events));
      assert.deepEqual(outcome, serviceFailure
        ? { status: "error", stage, code: stage === "temp" ? "workspace_service" : "destination_service" }
        : { status: "error", stage, code, availableBytes: 5 });
      assert.equal(checks.at(-1)[1], budget);
      assert.equal(checks.length, [20, 30, 10].indexOf(budget) + 1);
    }
  }
  ipc.checkDiskSpace = async () => ({ ok: true, available_bytes: 100 });
  ipc.tempDir = async () => { throw new Error("unavailable"); };
  assert.deepEqual(await runArchiveDiskPreflight(plan, diskOptions([])),
    { status: "error", stage: "temp", code: "workspace_service" });
  const events = [];
  assert.deepEqual(await runArchiveDiskPreflight({ ...plan, system_temp_budget_bytes: 0 }, diskOptions(events)),
    { status: "ready" });
  assert.deepEqual(events, ["checkingTemp", "workspace", "checkingDest", "destination"]);
});

test("departed disk checks discard late successes and errors at every service boundary", async (t) => {
  const original = { checkDiskSpace: ipc.checkDiskSpace, tempDir: ipc.tempDir };
  t.after(() => Object.assign(ipc, original));
  for (const boundary of [20, "tempDir", 30, 10]) {
    for (const failed of [false, true]) {
      const blocked = deferred(), reached = deferred(), events = [];
      let current = true;
      const invoke = (at, value) => {
        if (at !== boundary) return Promise.resolve(value);
        reached.resolve();
        return blocked.promise;
      };
      ipc.tempDir = () => invoke("tempDir", "/system");
      ipc.checkDiskSpace = (_path, bytes) => invoke(bytes, { ok: true, available_bytes: 100 });
      const pending = runArchiveDiskPreflight(plan, diskOptions(events, () => current));
      await reached.promise;
      current = false;
      const retained = [...events];
      if (failed) blocked.reject(new Error("late failure"));
      else blocked.resolve(boundary === "tempDir" ? "/system" : { ok: true, available_bytes: 100 });
      assert.deepEqual(await pending, { status: "stale" });
      assert.deepEqual(events, retained);
    }
  }
});

function authorizationOptions(overrides = {}) {
  return { path: "/output/archive.zip", inspect: async () => ({ conflict: true, guard: "new" }),
    isCurrent: () => true, confirm: async () => async () => true, tr: englishTranslate,
    message: "gui.output.replace_existing", ...overrides };
}

test("authorization rejects malformed guards and rechecks changed or removed outputs", async () => {
  let confirmations = 0;
  const confirm = async () => async () => { confirmations += 1; return true; };
  for (const inspection of [{ conflict: false, guard: "unexpected" }, { conflict: true, guard: null }]) {
    await assert.rejects(authorizeArchiveOutput(authorizationOptions({ inspect: async () => inspection, confirm })),
      DestinationInspectionError);
  }
  assert.equal(confirmations, 0);
  const spec = { dest: "/output/archive.zip", replace_existing: true, replacement_guard: "new" };
  assert.equal(await refreshArchiveOutputAuthorization(spec, authorizationOptions({ confirm })), spec);
  assert.equal(confirmations, 0);
  const changed = await refreshArchiveOutputAuthorization({ ...spec, replacement_guard: "old" }, authorizationOptions({ confirm }));
  assert.equal(changed.replacement_guard, "new");
  assert.equal(confirmations, 1);
  const removed = await refreshArchiveOutputAuthorization(spec, authorizationOptions({ confirm,
    inspect: async () => ({ conflict: false, guard: null }) }));
  assert.deepEqual(removed, { ...spec, replace_existing: false, replacement_guard: null });
  assert.equal(confirmations, 1);
  const absent = { ...spec, replace_existing: false, replacement_guard: null };
  const appeared = await refreshArchiveOutputAuthorization(absent, authorizationOptions());
  assert.deepEqual(appeared, { ...absent, replace_existing: true, replacement_guard: "new" });
  assert.equal(await refreshArchiveOutputAuthorization(spec, authorizationOptions({
    inspect: async () => ({ conflict: true, guard: "changed" }), confirm: async () => async () => false,
  })), null);
  assert.equal(spec.replacement_guard, "new");
});

test("authorization ignores late inspection, dialog loading and approval without another prompt", async () => {
  for (const boundary of ["inspect", "load", "confirm"]) {
    const blocked = deferred(), reached = deferred(), events = [];
    let current = true;
    const invoke = (at, value) => {
      events.push(at);
      if (at !== boundary) return Promise.resolve(value);
      reached.resolve();
      return blocked.promise;
    };
    const confirm = () => invoke("confirm", true);
    const pending = authorizeArchiveOutput(authorizationOptions({
      inspect: () => invoke("inspect", { conflict: true, guard: "new" }),
      confirm: () => invoke("load", confirm), isCurrent: () => current,
    }));
    await reached.promise;
    current = false;
    const retained = [...events];
    blocked.resolve(boundary === "inspect" ? { conflict: true, guard: "new" } : boundary === "load" ? confirm : true);
    assert.equal(await pending, null);
    assert.deepEqual(events, retained);
  }
});

test("split replacement retains its body and title with the existing action labels", async () => {
  let dialog;
  await authorizeArchiveOutput(authorizationOptions({ message: "gui.output.replace_existing_split",
    confirm: async () => async (body, options) => { dialog = { body, ...options }; return true; },
  }));
  assert.equal(dialog.body, englishTranslate("gui.output.replace_existing_split.body").replace("{path}", "/output/archive.zip"));
  assert.equal(dialog.title, englishTranslate("gui.output.replace_existing_split.title"));
  assert.equal(dialog.okLabel, englishTranslate("gui.output.replace_existing.action"));
  assert.equal(dialog.cancelLabel, englishTranslate("gui.output.replace_existing.cancel"));
});
