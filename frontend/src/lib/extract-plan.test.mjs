import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ExtractPlanSession } = await server.ssrLoadModule("/src/lib/extract-plan.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const originalIpc = { planExtract: ipc.planExtract, cancelExtractPlan: ipc.cancelExtractPlan };
test.beforeEach(() => {
  Object.assign(ipc, originalIpc);
  ipc.cancelExtractPlan = async () => {};
});
test.afterEach(() => Object.assign(ipc, originalIpc));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function input(overrides = {}) {
  return { archiveId: 1, path: "archive-source:one", displayPath: "/Photos.zip",
    dest: "/Output", selection: ["photos/", "notes.txt"], smart: true, encoding: null, ...overrides };
}

function plan(overrides = {}) {
  return { requested_destination: "/Output", destination: "/Output/Photos", layout: "wrap_in_folder",
    entries: 3, files: 2, directories: 1, symlinks: 0, hardlinks: 0, other: 0,
    total_bytes: 12, estimated_conflicts: 0, input_guard: "checked-input",
    required_free_bytes: 12300, available_bytes: 20000, space_ok: true, ...overrides };
}

function owner(t, preview) {
  let nextId = 0;
  const session = new ExtractPlanSession(() => `extract-${++nextId}`, preview);
  t.after(() => session.dispose());
  return session;
}

test("matching requests share work and immediate submission flushes only the latest queued plan", async (t) => {
  const session = owner(t);
  const first = deferred();
  const latest = deferred();
  const calls = [];
  const cancelled = [];
  ipc.planExtract = (...args) => {
    calls.push(args);
    return calls.length === 1 ? first.promise : latest.promise;
  };
  ipc.cancelExtractPlan = async (id) => { cancelled.push(id); };

  const original = input();
  const starting = session.request(original, { debounce: true });
  assert.equal(calls.length, 0);
  assert.equal(session.request(input()), starting, "an immediate request flushes the matching debounce");
  assert.equal(calls.length, 1);
  assert.equal(session.request(input()), starting, "an active request keeps the same promise");
  const superseded = session.request(input({ dest: "/Obsolete" }), { debounce: true });
  const newestInput = input({ dest: "/Newest" });
  const newest = session.request(newestInput, { debounce: true });
  await superseded;
  assert.equal(session.request(newestInput), newest);
  assert.equal(calls.length, 1, "cancellation retains the active read until it settles");
  assert.deepEqual(cancelled, ["extract-1"]);

  first.resolve(plan({ destination: "/Obsolete/Photos" }));
  await starting;
  assert.equal(session.phase, "loading");
  assert.equal(session.plan, null, "a late success cannot publish an obsolete plan");
  assert.equal(calls.length, 2);
  assert.equal(calls[1][2], "/Newest");
  latest.resolve(plan({ requested_destination: "/Newest", destination: "/Newest/Photos" }));
  await newest;
  assert.equal(session.phase, "ready");
  assert.equal(session.plan.destination, "/Newest/Photos");
  await session.request(newestInput);
  assert.equal(calls.length, 2, "a ready matching plan is reused");
});

test("plan ownership includes every input field and late errors cannot replace a newer snapshot", async (t) => {
  const session = owner(t);
  const first = deferred();
  const latest = deferred();
  const calls = [];
  const cancelled = [];
  ipc.planExtract = (...args) => {
    calls.push(args);
    return calls.length === 1 ? first.promise : latest.promise;
  };
  ipc.cancelExtractPlan = async (id) => { cancelled.push(id); throw new Error("already finished"); };
  const supplied = input();
  const starting = session.request(supplied);
  assert.equal(session.matches(input()), true);
  for (const change of [
    { archiveId: 2 }, { path: "archive-source:two" }, { displayPath: "/Other.zip" },
    { dest: "/Other" }, { selection: ["notes.txt", "photos/"] }, { selection: null },
    { selection: [] }, { smart: false }, { encoding: "gbk" },
  ]) assert.equal(session.matches(input(change)), false);
  assert.equal(session.matches(null), false);
  supplied.path = "replaced-source";
  supplied.selection.push("late.txt");
  assert.equal(calls[0][0], "archive-source:one");
  assert.deepEqual(calls[0][3], ["photos/", "notes.txt"], "IPC receives the owned selection snapshot");
  assert.equal(session.matches(input()), true);
  assert.equal(session.matches(supplied), false);

  const currentInput = input({ encoding: "gbk" });
  const current = session.request(currentInput);
  assert.equal(session.request(currentInput), current);
  currentInput.path = "replaced-queued-source";
  currentInput.selection.push("late-queued.txt");
  assert.deepEqual(cancelled, ["extract-1"]);
  assert.equal(calls.length, 1);
  first.reject(new Error("obsolete source failed"));
  await starting;
  assert.equal(session.phase, "loading");
  assert.equal(session.errorKey, "");
  assert.equal(session.plan, null);
  assert.equal(calls.length, 2);
  assert.equal(calls[1][0], "archive-source:one");
  assert.deepEqual(calls[1][3], ["photos/", "notes.txt"], "queued work retains its own snapshot before IPC starts");
  assert.equal(calls[1][5], "gbk");
  latest.resolve(plan({ input_guard: "new-guard" }));
  await current;
  assert.equal(session.phase, "ready");
  assert.equal(session.plan.input_guard, "new-guard");
  assert.equal(session.matches(input({ encoding: "gbk" })), true);
  assert.equal(session.matches(currentInput), false);
});

test("reset and disposal settle queued work while retaining the active read until completion", async (t) => {
  const session = owner(t);
  const first = deferred();
  const next = deferred();
  const calls = [];
  const cancelled = [];
  ipc.planExtract = (...args) => {
    calls.push(args);
    return calls.length === 1 ? first.promise : next.promise;
  };
  ipc.cancelExtractPlan = async (id) => { cancelled.push(id); };
  let firstSettled = false;
  const starting = session.request(input());
  starting.then(() => { firstSettled = true; });
  const discarded = session.request(input({ dest: "/Discarded" }), { debounce: true });
  session.reset();
  await discarded;
  assert.equal(firstSettled, false);
  assert.equal(session.phase, "idle");
  assert.equal(session.plan, null);
  assert.equal(session.errorKey, "");
  assert.equal(session.matches(input()), false);
  assert.deepEqual(cancelled, ["extract-1"]);

  const replacementInput = input({ dest: "/Replacement" });
  const replacement = session.request(replacementInput);
  assert.equal(calls.length, 1);
  first.resolve(plan());
  await starting;
  assert.equal(calls.length, 2);
  assert.equal(calls[1][2], "/Replacement");
  assert.equal(session.phase, "loading");
  assert.equal(session.matches(replacementInput), true);
  let replacementSettled = false;
  replacement.then(() => { replacementSettled = true; });
  const abandoned = session.request(input({ dest: "/Abandoned" }), { debounce: true });
  session.dispose();
  await abandoned;
  assert.equal(replacementSettled, false);
  assert.deepEqual(cancelled, ["extract-1", "extract-3"]);
  await session.request(input({ dest: "/After disposal" }));
  assert.equal(calls.length, 2, "disposal cannot admit another backend read");
  next.resolve(plan());
  await replacement;
  assert.equal(session.phase, "idle");
  assert.equal(session.plan, null);
  assert.equal(session.errorKey, "");
  assert.equal(calls.length, 2);
});

test("blocked and failed plans support explicit retry while preview plans use the existing adapter", async (t) => {
  const session = owner(t);
  let calls = 0;
  ipc.planExtract = async () => {
    calls += 1;
    if (calls === 1) return plan({ space_ok: false, available_bytes: 1 });
    if (calls === 2) throw new Error("read failed");
    return plan();
  };
  const requested = input();
  await session.request(requested);
  assert.equal(session.phase, "blocked");
  assert.equal(session.plan.available_bytes, 1);
  await session.request(requested);
  assert.equal(calls, 1);
  await session.request(requested, { force: true });
  assert.equal(session.phase, "error");
  assert.equal(session.plan, null);
  assert.equal(session.errorKey, "gui.extract.plan_unavailable_body");
  assert.equal(session.matches(requested), true, "the App effect can retain its same-input guard after failure");
  await session.request(requested, { force: true });
  assert.equal(session.phase, "ready");
  assert.equal(session.errorKey, "");
  assert.equal(calls, 3);

  const previewInputs = [];
  const preview = owner(t, (snapshot) => {
    previewInputs.push(snapshot);
    return plan({ destination: "/Preview/Photos" });
  });
  await preview.request(requested);
  assert.equal(preview.phase, "ready");
  assert.equal(preview.plan.destination, "/Preview/Photos");
  assert.deepEqual(previewInputs, [requested]);
  assert.notEqual(previewInputs[0].selection, requested.selection);
  assert.equal(calls, 3, "the existing preview adapter bypasses native IPC");
});
