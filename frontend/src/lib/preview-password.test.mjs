import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const passwordError = (scope, wrong = false) => ({
  key: wrong ? "error.wrong_password" : "error.password_required",
  params: { password_scope: scope }, detail: "",
});
const nextTurn = () => new Promise((resolve) => setImmediate(resolve));

async function withFlow(run) {
  const server = await createTestServer();
  try {
    const { createPreviewPasswordFlow } = await server.ssrLoadModule("/src/lib/preview-password.svelte.ts");
    await run(createPreviewPasswordFlow());
  } finally {
    await server.close();
  }
}

const context = { outerName: "outer.zip", innerName: "inner.7z", isCurrent: () => true };

test("preview prompts for distinct passwords, retries only the rejected layer, and forgets inputs", async () => {
  await withFlow(async (flow) => {
    const attempts = [];
    let credentials;
    const result = flow.run(context, async (passwords) => {
      credentials = passwords;
      attempts.push({ ...passwords });
      if (passwords.outer !== "outer-value") throw passwordError("outer", passwords.outer !== null);
      if (passwords.inner !== "inner-value") throw passwordError("inner", passwords.inner !== null);
      return { id: 3 };
    });
    await nextTurn();
    assert.equal(flow.prompt.name, "outer.zip");
    assert.equal(flow.answer(""), false);
    assert.equal(flow.answer("outer-value"), true);
    assert.equal(flow.prompt.busy, true);
    assert.equal(flow.answer("duplicate"), false);
    await nextTurn();
    assert.equal(flow.prompt.name, "inner.7z");
    assert.equal(flow.prompt.scope, "inner");
    const firstInnerPrompt = flow.prompt.id;
    flow.answer("wrong-value");
    await nextTurn();
    assert.equal(flow.prompt.wrong, true);
    assert.notEqual(flow.prompt.id, firstInnerPrompt);
    assert.equal(flow.prompt.name, "inner.7z");
    flow.answer("inner-value");
    assert.deepEqual(await result, { id: 3 });
    assert.equal(attempts.length, 4);
    assert.ok(attempts.slice(1).every((attempt) => attempt.outer === "outer-value"));
    assert.equal(flow.prompt, null);
    assert.equal(credentials.outer, null);
    assert.equal(credentials.inner, null);
  });
});

test("cancelling a password prompt stops retries and stale failures cannot replace a newer prompt", async () => {
  await withFlow(async (flow) => {
    let calls = 0;
    const cancelled = flow.run(context, async () => { calls++; throw passwordError("outer"); });
    await nextTurn();
    flow.cancel();
    assert.equal(await cancelled, null);
    assert.equal(calls, 1);
    let rejectOld;
    const old = flow.run(context, () => new Promise((_resolve, reject) => { rejectOld = reject; }));
    const newer = flow.run({ ...context, outerName: "new.zip" }, async () => { throw passwordError("outer"); });
    await nextTurn();
    rejectOld(passwordError("inner"));
    assert.equal(await old, null);
    assert.equal(flow.prompt.name, "new.zip");
    assert.equal(flow.prompt.scope, "outer");
    flow.cancel();
    assert.equal(await newer, null);
  });
});

test("late successful handles are returned for cleanup, while non-password errors keep their meaning", async () => {
  await withFlow(async (flow) => {
    let resolve;
    const late = flow.run(context, () => new Promise((done) => { resolve = done; }));
    flow.cancel();
    resolve({ id: 8 });
    assert.deepEqual(await late, { id: 8 });
    const error = { key: "error.corrupt_archive", params: {}, detail: "damaged" };
    await assert.rejects(flow.run(context, async () => { throw error; }), (actual) => actual === error);
    assert.equal(flow.prompt, null);
    let current = true;
    const pending = flow.run({ ...context, isCurrent: () => current }, async () => { throw passwordError("outer"); });
    await nextTurn();
    current = false;
    flow.answer("unused");
    assert.equal(await pending, null);
    assert.equal(flow.prompt, null);
  });
});
