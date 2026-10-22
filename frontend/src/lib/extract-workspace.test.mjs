import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";
import { extractionFixture } from "../../tests/extraction.mjs";

const server = await createTestServer();
test.after(() => server.close());
const fixture = await extractionFixture(server);
test.afterEach(() => fixture.dispose());
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const original = { planExtract: ipc.planExtract, cancelExtractPlan: ipc.cancelExtractPlan };
test.afterEach(() => Object.assign(ipc, original));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};

function harness() {
  const calls = [];
  const context = {
    currentArchive: { id: 1, source: "squallz-archive://1", path: "/Archive/outer.zip", name: "outer.zip", encoding_override: null },
    screen: "extract", selectedPaths: () => new Set(["notes.txt"]),
    getDialogModule: async () => ({ open: async () => null }),
    openNativeDialog: (_purpose, open, options) => open(options),
    showNotice: (text) => calls.push(["notice", text]),
    submitCurrentArchiveJob: async (spec) => { calls.push(["submit", spec]); return true; },
    submitJob: async (spec) => { calls.push(["submit", spec]); return calls.length; },
  };
  return { session: fixture.install(context), context, calls };
}

test("disposing extraction abandons a module load before native open and an already open picker after resolution", async () => {
  for (const loading of [false, true]) {
    const { session, context, calls } = harness();
    const waiting = deferred();
    let opens = 0;
    if (loading) context.getDialogModule = () => waiting.promise;
    context.openNativeDialog = () => { opens++; return waiting.promise; };
    const choosing = session.chooseDestination();
    await Promise.resolve();
    assert.equal(session.choosing, true);
    session.dispose();
    waiting.resolve(loading ? { open: async () => "/late" } : "/late");
    assert.equal(await choosing, false);
    assert.equal(opens, loading ? 0 : 1);
    assert.equal(session.state.customDest, "");
    assert.deepEqual(calls, []);
    assert.equal(await session.chooseDestination(), false);
    assert.equal(session.review.replaceSources(["/late.zip"]), false);
    assert.equal(await session.review.start("batch"), false);
    assert.equal(opens, loading ? 0 : 1, "a disposed owner cannot open another native picker");
  }
});

test("batch and nested submissions retain independent locks through navigation and settlement", async () => {
  const { session, context } = harness();
  const batch = deferred(), nested = deferred();
  context.submitJob = (spec) => spec.kind === "batch_extract" ? batch.promise : nested.promise;
  session.review.replaceSources(["/Archives/one.zip"]);
  const first = session.review.start("batch");
  session.leave();
  assert.equal(session.review.submitting("batch"), true);
  const draft = session.review.prepareNested("squallz-archive://1", "/Archive/outer.zip", "inner.zip");
  assert.equal(session.review.adoptNested(draft), true);
  const second = session.review.start("nested");
  batch.resolve(1); await first;
  assert.equal(session.review.locked("batch"), false);
  assert.equal(session.review.locked("nested"), true, "an older batch cannot unlock the inner extraction submission");
  nested.resolve(2); await second;
  assert.equal(session.review.locked("nested"), false);
});

test("late review submissions preserve accepted history without replacing the new workspace feedback", async () => {
  const { clearOperationHistory, operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
  for (const kind of ["batch", "nested"]) {
    for (const lifecycle of ["leave", "dispose"]) {
      for (const accepted of [false, true]) {
        clearOperationHistory();
        const { session, context, calls } = harness();
        context.screen = kind === "batch" ? "batch" : "nestedExtract";
        session.review.replaceSources(["/Archives/one.zip"]);
        session.review.adoptNested(session.review.prepareNested("squallz-archive://1", "/Archive/outer.zip", "inner.zip"));
        const response = deferred();
        context.submitJob = () => response.promise;
        const submission = session.review.start(kind);
        session[lifecycle]();
        context.screen = "settingsGeneral";
        context.showNotice("Current settings feedback");
        if (accepted) response.resolve(7);
        else response.reject(new Error("late service error"));
        assert.equal(await submission, accepted);
        assert.deepEqual(calls, [["notice", "Current settings feedback"]]);
        assert.equal(operationHistory().length, accepted ? 1 : 0);
        assert.equal(session.review.submitting(kind), false);
      }
    }
  }
});

test("both extract skins read one session and localized plan without exposing the opaque archive source", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: ExtractWorkspace } = await server.ssrLoadModule("/src/components/ExtractWorkspace.svelte");
  const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const { session, context } = harness();
  session.options.syncArchive(context.currentArchive);
  session.options.acceptDestination("/Exact/完整输出目录");
  session.open("selection", ["notes.txt"]);
  const bridge = { archiveTitle: "outer.zip", archiveLine: "/Archive/outer.zip", warning: "",
    password: "", testEnabled: true, onReady() {}, onTest() {}, onBatch() {} };
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    for (const variant of ["modern", "classic"]) {
      const { body } = render(ExtractWorkspace, { props: {
        variant, session, view: context.extractView, presets: context.presets, bridge,
      } });
      assert.ok(body.includes("/Exact/完整输出目录"));
      assert.ok(body.includes(tFallback("gui.extract.plan_heading")));
      assert.ok(body.includes(tFallback("gui.selection.count_selected").replace("{count}", "1")));
      assert.doesNotMatch(body, /squallz-archive:|gui\.extract\.|style=/);
    }
  }
});
