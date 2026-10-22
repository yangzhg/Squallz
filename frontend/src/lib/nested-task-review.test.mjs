import assert from "node:assert/strict";
import test from "node:test";
import { englishTranslate, createTestServer } from "../../tests/runtime.mjs";
import { extractionFixture } from "../../tests/extraction.mjs";
import { settingsDto } from "../../tests/settings.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { reviewNestedExtract } = await server.ssrLoadModule("/src/lib/nested-extract.ts");
const { SettingsSession } = await server.ssrLoadModule("/src/lib/settings-session.svelte.ts");
const fixture = await extractionFixture(server);
test.afterEach(() => fixture.dispose());
const plain = (value) => JSON.parse(JSON.stringify(value));

function spec() {
  return { kind: "extract_nested", outer_path: "squallz-archive://7", entry_path: "Previous versions/完整内层归档.7z",
    dest: "/output/original", overwrite: "rename", symlinks: "skip", smart: false,
    encoding: "gbk", password: "old-secret", best_effort: true };
}

function harness() {
  const calls = [];
  const context = {
    screen: "nestedExtract", currentArchive: { source: "/unrelated/current.zip", encoding_override: "shift_jis" },
    tr: englishTranslate, showNotice: (message) => calls.push(["notice", message]),
    submitJob: async (job) => { calls.push(["submit", plain(job)]); return 99; },
    document: { getElementById: (id) => ({ focus: () => calls.push(["focus-field", id]) }) },
    getDialogModule: async () => ({ open: async () => null }), openNativeDialog: async (_key, open, options) => open(options),
  };
  context.settingsSession = new SettingsSession({ platform: () => "macos", tr: context.tr, emit() {} });
  context.settingsSession.applySnapshot(settingsDto(), context.settingsSession.captureGenerations());
  const extraction = fixture.install(context);
  const session = extraction.review;
  return { context, calls, session,
    navigate: (screen) => { extraction.leave(); context.screen = screen; },
    surface: (variant) => ({ kind: "batch", mode: "nested", variant, session,
      title: context.tr("gui.nested_extract.title", "Extract inner archive"), tr: context.tr,
      archiveReturn: { visible: false }, onReady() {}, onRemove() {}, onBrowse() {},
      format: (path) => path.split(".").at(-1).toUpperCase() }),
  };
}

async function restore(run, state = "failed") {
  const job = spec();
  assert.equal(taskReviewScreen({ state, spec: job }), "nestedExtract");
  assert.equal(run.session.adoptNested(reviewNestedExtract(job, "/Archives/外层归档.zip")), true);
}

for (const state of ["failed", "cancelled"]) {
test(`${state} inner extraction restores the exact source and nonsecret settings into an editable review`, async () => {
  assert.equal(taskReviewScreen({state:"done",spec:spec(),result:{counts:{created:2,failed:1}}}),"nestedExtract");
  assert.equal(taskReviewScreen({state:"done",spec:spec(),result:{counts:{created:3,failed:0}}}),null);
  const run = harness();
  await restore(run, state);
  assert.equal(run.context.screen, "nestedExtract");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  const draft = run.session.nested;
  assert.equal(`${draft.outerDisplayPath} › ${draft.entry_path}`, "/Archives/外层归档.zip › Previous versions/完整内层归档.7z");
  assert.equal(draft.encoding, "gbk");
  assert.equal(draft.best_effort, true);
  assert.equal(draft.overwrite, "rename");
  assert.equal(draft.symlinks, "skip");
  assert.equal(draft.smart, false);
  run.session.editNested({ dest: "/edited/交付" });
  run.session.editNested({ overwrite: "ask", symlinks: "preserve", smart: true });
  run.session.editNested({ best_effort: false });
  await run.session.start("nested");
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.deepEqual(submitted, { ...spec(), dest: "/edited/交付", overwrite: "ask", symlinks: "preserve",
    smart: true, best_effort: false, password: null });
  assert.equal("password" in run.session.nested, false);
  assert.equal("outerDisplayPath" in submitted, false);
});
}

test("preview extraction first reviews a smart base directory without pre-adding the inner archive name", () => {
  const run = harness();
  run.context.currentArchive = { source: "squallz-archive://7", encoding_override: "shift_jis" };
  assert.equal(run.session.adoptNested(run.session.prepareNested("squallz-archive://7", "/Archives/outer.zip", "inner.zip")), true);
  assert.equal(run.session.nested.dest, "/Archives");
  assert.equal(run.session.nested.encoding, "shift_jis");
  assert.equal(run.session.nested.smart, true);
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  run.context.settingsSession.applySnapshot(settingsDto({ default_extract_dir: "/Preferred" }),
    run.context.settingsSession.captureGenerations());
  run.context.settingsSession.setGeneral("defaultExtractDir", "/not-applied");
  run.session.adoptNested(run.session.prepareNested("/another/outer.zip", "/another/outer.zip", "inner.zip"));
  assert.equal(run.session.nested.dest, "/Preferred");
  assert.equal(run.session.nested.encoding, null);
});

test("empty targets and submission locks prevent accidental or duplicate extraction; failures preserve edits", async () => {
  const run = harness();
  assert.equal(await run.session.start("nested"), false);
  await restore(run);
  run.session.editNested({ dest: "  " });
  assert.equal(await run.session.start("nested"), false);
  assert.deepEqual(run.calls.at(-1), ["focus-field", "batch-destination-0"]);
  run.session.editNested({ dest: "/edited" });
  const saved = plain(run.session.nested);
  let reject;
  run.context.submitJob = (job) => { run.calls.push(["submit", plain(job)]); return new Promise((_, fail) => { reject = fail; }); };
  const pending = run.session.start("nested");
  assert.equal(run.session.locked("nested"), true);
  run.session.editNested({ dest: "/ignored" });
  assert.equal(run.session.adoptNested(run.session.prepareNested("/other.zip", "/other.zip", "other.7z")), false);
  await run.session.start("nested");
  assert.equal(run.calls.filter(([name]) => name === "submit").length, 1);
  assert.deepEqual(plain(run.session.nested), saved);
  reject(new Error("service unavailable")); await pending;
  assert.equal(run.session.submitting("nested"), false);
  assert.deepEqual(plain(run.session.nested), saved);
  assert.match(run.calls.at(-1)[1], /Could not start extraction/);
});

test("destination selection preserves cancellation and ignores responses after leaving the review", async () => {
  const run = harness(); await restore(run);
  run.context.getDialogModule = async () => ({ open: async () => "/picked" });
  await run.session.choose("nested");
  assert.equal(run.session.nested.dest, "/picked");
  run.context.getDialogModule = async () => ({ open: async () => null });
  await run.session.choose("nested");
  assert.equal(run.session.nested.dest, "/picked");
  assert.match(run.calls.at(-1)[1], /cancelled/);
  let finish;
  run.context.getDialogModule = async () => ({ open: () => new Promise((resolve) => { finish = resolve; }) });
  const pending = run.session.choose("nested");
  await new Promise((resolve) => setImmediate(resolve));
  run.session.leave();
  finish("/stale"); await pending;
  assert.equal(run.session.nested.dest, "/picked");
  assert.equal(run.session.locked("nested"), false);
});

test("navigation invalidates an inner destination chooser before opening and preserves newer requests", async () => {
  for (const phase of ["loading", "choosing"]) {
    for (const outcome of ["selected", "cancelled", "failed"]) {
      const run = harness(); await restore(run);
      const saved = plain(run.session.nested);
      let finish, fail, started;
      const ready = new Promise(resolve => { started = resolve; });
      const oldResult = new Promise((resolve, reject) => { finish = resolve; fail = reject; });
      let opened = 0;
      run.context.getDialogModule = phase === "loading" ? () => { started(); return oldResult; }
        : async () => ({open:async()=>null});
      run.context.openNativeDialog = () => { opened++; started(); return oldResult; };
      const oldChoice = run.session.choose("nested"); await ready;
      run.navigate("settingsGeneral"); run.navigate("nestedExtract");
      assert.equal(run.session.locked("nested"), false);
      let completeNew;
      run.context.getDialogModule = async () => ({open:async()=>null});
      run.context.openNativeDialog = () => { opened++; return new Promise(resolve => { completeNew = resolve; }); };
      const newChoice = run.session.choose("nested");
      await new Promise(resolve => setImmediate(resolve));
      const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
      if (outcome === "failed") fail(new Error("late dialog failure"));
      else finish(phase === "loading" ? {open:async()=>"/stale"} : outcome === "cancelled" ? null : "/stale");
      await oldChoice;
      assert.equal(opened, phase === "loading" ? 1 : 2);
      assert.deepEqual(plain(run.session.nested), saved);
      assert.equal(run.session.locked("nested"), true);
      assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
      completeNew("/new/destination"); await newChoice;
      assert.equal(run.session.nested.dest, "/new/destination");
      assert.equal(run.session.locked("nested"), false);
    }
  }
});

test("shared review layout presents the inner source, outer encoding and real options in both languages", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: ToolsWorkspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const run = harness(); await restore(run);
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale); run.context.tr = tFallback;
    for (const variant of ["modern", "classic"]) {
      const { body } = render(ToolsWorkspace, { props: { surface: run.surface(variant) } });
      assert.ok(body.includes("/Archives/外层归档.zip › Previous versions/完整内层归档.7z"));
      assert.ok(body.includes(tFallback("gui.nested_extract.outer_encoding").replace("{encoding}", "gbk")));
      assert.ok(body.includes(tFallback("gui.nested_extract.execution_hint")));
      assert.doesNotMatch(body, /old-secret|squallz-archive:|gui\.nested_extract\.|batch-remove-0/);
      assert.equal(body.includes(tFallback("gui.batch.add_archives")), false);
    }
  }
  const empty = harness();
  empty.context.tr = tFallback;
  const { body } = render(ToolsWorkspace, { props: { surface: empty.surface("classic") } });
  assert.ok(body.includes(tFallback("gui.nested_extract.empty_hint")));
});
