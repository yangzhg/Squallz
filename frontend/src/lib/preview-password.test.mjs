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
    const cancellations = [];
    await run(createPreviewPasswordFlow(async (id) => { cancellations.push(id); }), cancellations);
  } finally {
    await server.close();
  }
}

const context = { outerName: "outer.zip", innerName: "inner.7z", isCurrent: () => true };

test("both password layouts retain labelled input, error context, cancellation and optional password management", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Workspace } = await server.ssrLoadModule("/src/components/PasswordWorkspace.svelte");
    const { loadLocale, tFallback, t } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    for (const language of ["en-US", "zh-CN"]) {
      await loadLocale(language);
      for (const variant of ["modern", "classic"]) {
        const surface = {
          variant, tr: tFallback, active: true,
          name: "Quarterly archives & supporting documents.7z", detail: "Awaiting the inner archive",
          sessionDetail: "Used for this archive in this session", failureDetail: "Retry or cancel",
          secretStoreLabel: "Keychain", value: "", busy: false, rejected: false, error: t("gui.password.empty_error"),
          forgetVisible: true, forgetDisabledReason: "", forgetAriaLabel: t("gui.settings.password_book.forget_current"),
          onInputMount() {}, onValueChange() {}, onSubmit() {}, onCancel() {}, onForget() {}, onBack() {},
        };
        const body = render(Workspace, { props: { surface } }).body;
        assert.equal((body.match(/<input\b/g) ?? []).length, 1);
        const input = body.match(/<input\b[^>]*>/)[0];
        assert.match(input, /type="password"/);
        assert.match(input, /aria-invalid="true"/);
        for (const id of input.match(/aria-describedby="([^"]+)"/)[1].split(" ")) {
          assert.ok(body.includes(`id="${id}"`), `${variant} describes input with ${id}`);
        }
        assert.ok(body.includes(t("gui.settings.password_book.forget_current")));
        assert.match(body, /<details\b[^>]*><summary/);
        assert.match(body, /Quarterly archives &amp; supporting documents\.7z/);
        const rejected = render(Workspace, { props: { surface: { ...surface, rejected: true, error: null } } }).body;
        assert.match(rejected, /<input\b[^>]*aria-invalid="true"/);
        assert.match(rejected, /<p\b[^>]*id="password-request-detail"[^>]*role="alert"/);
        const busy = render(Workspace, { props: { surface: { ...surface, busy: true, error: null } } }).body;
        assert.match(busy, /<input\b[^>]*disabled/);
        assert.match(busy, /<button\b[^>]*type="submit"[^>]*aria-busy="true"[^>]*disabled/);
        assert.ok(busy.includes(t("gui.password.unlocking")));
        assert.match(busy, new RegExp(`<button type="button">${t("common.cancel")}</button>`));
        const empty = render(Workspace, { props: { surface: { ...surface, active: false } } }).body;
        assert.doesNotMatch(empty, /<input\b|<form\b/);
        assert.ok(empty.includes(t("gui.nav.back_to_archive")));
        assert.doesNotMatch(empty, /gui\.[a-z_.]+/);
      }
    }
  } finally {
    await server.close();
  }
});

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

test("cancelling sends only the active request id and each password retry has its own id", async () => {
  await withFlow(async (flow, cancellations) => {
    const ids = [];
    let finish;
    const pending = flow.run(context, async (passwords, requestId) => {
      ids.push(requestId);
      if (!passwords.outer) throw passwordError("outer");
      return new Promise((resolve) => { finish = resolve; });
    });
    await nextTurn();
    assert.equal(cancellations.length, 0);
    flow.answer("answer");
    await nextTurn();
    assert.notEqual(ids[0], ids[1]);
    flow.cancel();
    flow.cancel();
    assert.deepEqual(cancellations, [ids[1]]);
    finish({ id: 12 });
    assert.deepEqual(await pending, { id: 12 });
  });
});
