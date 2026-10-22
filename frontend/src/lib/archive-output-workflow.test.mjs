import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer, englishTranslate } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ArchiveOutputWorkflow } = await server.ssrLoadModule("/src/lib/archive-output-workflow.svelte.ts");
const { destinationInspectionCancelled } = await server.ssrLoadModule("/src/lib/archive-output-authorization.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const original = { ...ipc };
test.afterEach(() => Object.assign(ipc, original));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

function workflowFor(kind, extra = {}) {
  let request = 0;
  return new ArchiveOutputWorkflow({
    kind, ensureListener: async () => {}, nextRequestId: () => `${kind}-${++request}`,
    tr: englishTranslate, notice() {}, tError: (error) => error.key,
    getDialogModule: async () => ({ confirm: async () => true }), ...extra,
  });
}

async function waitFor(predicate) {
  for (let index = 0; index < 100; index += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail("Workflow did not reach the expected boundary");
}

for (const kind of ["create", "convert"]) {
  test(`${kind} cannot start backend inspection after its listener outlives the owner`, async () => {
    const listener = deferred();
    const workflow = workflowFor(kind, { ensureListener: () => listener.promise });
    let inspections = 0;
    ipc.inspectCreateDestination = async () => { inspections += 1; return { conflict: false, guard: null }; };
    const inspection = workflow.inspect("/output/archive.zip", false, null, workflow.begin());
    workflow.reset();
    listener.resolve();
    await assert.rejects(inspection, destinationInspectionCancelled);
    assert.equal(inspections, 0);
    assert.equal(workflow.state.phase, "idle");
    assert.equal(workflow.state.requestId, null);
  });

  test(`${kind} cancels the real plan and isolates its late success or failure from a new request`, async () => {
    const method = kind === "create" ? "planCreate" : "planConvert";
    const cancel = kind === "create" ? "cancelCreatePlan" : "cancelConvertPlan";
    for (const rejects of [false, true]) {
      const workflow = workflowFor(kind);
      const first = deferred(), second = deferred();
      const cancelled = [];
      let plans = 0;
      ipc[method] = async () => (++plans === 1 ? first.promise : second.promise);
      ipc[cancel] = async (id) => cancelled.push(id);
      ipc.checkDiskSpace = async () => { throw new Error("stale plans cannot check disks"); };
      const spec = { kind: kind === "create" ? "compress" : "convert", dest: "/output/archive.zip" };
      const old = workflow.prepare(spec, "/output", workflow.begin());
      await waitFor(() => plans === 1);
      const oldId = workflow.state.requestId;
      const next = workflow.prepare(spec, "/output", workflow.begin());
      await waitFor(() => plans === 2);
      const newId = workflow.state.requestId;
      assert.notEqual(newId, oldId);
      assert.deepEqual(cancelled, [oldId]);
      if (rejects) first.reject(new Error("old plan failed"));
      else first.resolve({ entries: 2 });
      assert.equal((await old).status, "stale");
      assert.equal(workflow.state.requestId, newId);
      assert.equal(workflow.state.phase, "measuring");
      assert.equal(workflow.state.plan, null);
      workflow.dispose();
      assert.deepEqual(cancelled, [oldId, newId]);
      second.resolve({ entries: 2 });
      assert.equal((await next).status, "stale");
      assert.equal(workflow.state.phase, "idle");
    }
  });

  test(`${kind} renews overwrite guards once and expires focus from a cancelled earlier confirmation`, async () => {
    const workflow = workflowFor(kind);
    workflow.state.plan = { total_bytes: 10 };
    workflow.review({ spec: {
      kind: kind === "create" ? "compress" : "convert", dest: "/output/archive.zip",
      split_size: null, sfx_target: null, replace_existing: true, replacement_guard: "old",
      ...(kind === "create" ? { password: "pending-secret" } : { dest_password: "pending-secret" }),
    } });
    const focus = deferred(), submission = deferred();
    let oldFocusCurrent;
    let inspections = 0, submissions = 0, queued = 0;
    ipc.inspectCreateDestination = async () => {
      if (++inspections === 1) throw { key: "error.cancelled", params: {} };
      return { conflict: true, guard: "renewed" };
    };
    const hooks = {
      recheck: true, prepare() {}, blockedMessage: () => null,
      focusReview: async (isCurrent) => { oldFocusCurrent = isCurrent; await focus.promise; },
      submit: async (spec) => {
        submissions += 1;
        assert.equal(spec.replacement_guard, "renewed");
        assert.equal(spec.password ?? spec.dest_password, "pending-secret");
        return submission.promise;
      },
      queued: () => { queued += 1; },
    };
    const first = workflow.confirm(hooks);
    await waitFor(() => Boolean(oldFocusCurrent));
    assert.equal(workflow.state.phase, "reviewing");
    assert.equal(oldFocusCurrent(), true);
    const second = workflow.confirm(hooks);
    await waitFor(() => submissions === 1);
    assert.equal(oldFocusCurrent(), false);
    await workflow.confirm(hooks);
    assert.equal(inspections, 2);
    assert.equal(submissions, 1);
    focus.resolve();
    await first;
    assert.equal(workflow.state.phase, "submitting");
    submission.resolve(42);
    await second;
    assert.equal(queued, 1);
    assert.equal(workflow.state.pending, null);
    assert.equal(workflow.state.phase, "ready");
  });
}
