import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { nestedExtractJob, reviewNestedExtract } = await server.ssrLoadModule("/src/lib/nested-extract.ts");
const { desktopBasename, desktopDirname } = await server.ssrLoadModule("/src/lib/desktop-path.ts");
const plain = (value) => JSON.parse(JSON.stringify(value));

function spec() {
  return { kind: "extract_nested", outer_path: "squallz-archive://7", entry_path: "Previous versions/完整内层归档.7z",
    dest: "/output/original", overwrite: "rename", symlinks: "skip", smart: false,
    encoding: "gbk", password: "old-secret", best_effort: true };
}

function harness() {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["cancelTaskReview", "reviewTask", "nestedExtractDraftLocked", "updateNestedExtractDraft", "restoreNestedExtractDraft",
    "prepareNestedExtract", "chooseNestedExtractDestination", "startNestedExtract", "nestedExtractWorkspaceSurface", "setScreen", "dismissRecoveryPicker"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = {
    nestedExtractDraft: null, nestedExtractSubmissionPending: false, nestedExtractPickerBusy: false,
    nestedExtractDraftGeneration: 0, nestedExtractReviewFocusPending: false, nestedExtractPickerRequest: 0,
    batchPickerRequest: 0, batchPickerBusy: false,
    archiveUpdateReview: { cancelSourceChoice() {} }, taskReviewRequestGeneration: 0,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {}, tick: async () => {},
    screen: "browse", archiveOpenStatus: "idle", taskWindowMode: false, appliedDefaultExtractDir: "",
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0,
    currentArchive: { source: "/unrelated/current.zip" },
    nestedExtractJob, reviewNestedExtract, taskReviewScreen,
    normalizedDefaultExtractDir: (path) => path || null,
    pathDir: (path) => desktopDirname(path, "macos"), pathBaseName: (path) => desktopBasename(path, "macos"),
    archiveFormatFromPath: (path) => path.split(".").at(-1).toUpperCase(), archiveEncodingForJob: () => "shift_jis",
    tr: (_key, fallback) => fallback, preventCreateSubmissionNavigation: () => false,
    preventConvertSubmissionNavigation: () => false, focusBlockingTaskIfAny: () => false,
    setScreen: (screen) => { context.screen = screen; calls.push(["screen", screen]); },
    dismissTaskDialog: async () => calls.push(["dismiss"]), focusNestedExtractReview: () => calls.push(["focus"]),
    showNotice: (message) => calls.push(["notice", message]), recordOperation() {},
    document: { documentElement:{},body:{},querySelectorAll:()=>[],getElementById: (id) => ({ focus: () => calls.push(["focus-field", id]) }) },
    submitJob: async (job) => { calls.push(["submit", plain(job)]); return 99; },
    isJobSubmitBlocked: () => false, isErrorDto: () => false, tError: () => "error",
    toolsArchiveReturnSurface: () => ({ visible: false }),
    extractOverwriteModes: ["ask", "skip", "overwrite", "rename"], extractOverwriteLabel: (mode) => mode,
    extractSymlinkModes: ["preserve", "skip", "follow"], extractSymlinkLabel: (mode) => mode,
    getDialogModule: async () => ({ open: async () => null }), openNativeDialog: async (_key, open, options) => open(options),
  };
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return { ...vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, context), context, calls };
}

async function restore(run, state = "failed") {
  const job = spec();
  await run.reviewTask({ state, spec: job }, { ...job, outer_path: "/Archives/外层归档.zip" });
}

for (const state of ["failed", "cancelled"]) {
test(`${state} inner extraction restores the exact source and nonsecret settings into an editable review`, async () => {
  assert.equal(taskReviewScreen({state:"done",spec:spec(),result:{counts:{created:2,failed:1}}}),"nestedExtract");
  assert.equal(taskReviewScreen({state:"done",spec:spec(),result:{counts:{created:3,failed:0}}}),null);
  const run = harness();
  await restore(run, state);
  assert.equal(run.context.screen, "nestedExtract");
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  const surface = run.nestedExtractWorkspaceSurface("modern");
  assert.equal(surface.rows[0].path, "/Archives/外层归档.zip › Previous versions/完整内层归档.7z");
  assert.equal(surface.rows[0].encoding, "gbk");
  assert.equal(surface.rows[0].bestEffort, true);
  assert.equal(surface.overwrite, "rename");
  assert.equal(surface.symlinks, "skip");
  assert.equal(surface.smart, false);
  surface.rows[0].onTargetInput("/edited/交付");
  surface.onOverwriteChange("ask"); surface.onSymlinksChange("preserve"); surface.onSmartChange(true);
  surface.rows[0].onBestEffortChange(false);
  await run.startNestedExtract();
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.deepEqual(submitted, { ...spec(), dest: "/edited/交付", overwrite: "ask", symlinks: "preserve",
    smart: true, best_effort: false, password: null });
  assert.equal("password" in run.context.nestedExtractDraft, false);
  assert.equal("outerDisplayPath" in submitted, false);
});
}

test("preview extraction first reviews a smart base directory without pre-adding the inner archive name", () => {
  const run = harness();
  run.context.currentArchive = { source: "squallz-archive://7" };
  assert.equal(run.prepareNestedExtract("squallz-archive://7", "/Archives/outer.zip", "inner.zip"), true);
  assert.equal(run.context.nestedExtractDraft.dest, "/Archives");
  assert.equal(run.context.nestedExtractDraft.encoding, "shift_jis");
  assert.equal(run.context.nestedExtractDraft.smart, true);
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  run.context.appliedDefaultExtractDir = "/Preferred";
  run.prepareNestedExtract("/another/outer.zip", "/another/outer.zip", "inner.zip");
  assert.equal(run.context.nestedExtractDraft.dest, "/Preferred");
  assert.equal(run.context.nestedExtractDraft.encoding, null);
});

test("empty targets and submission locks prevent accidental or duplicate extraction; failures preserve edits", async () => {
  const run = harness();
  assert.equal(await run.startNestedExtract(), false);
  await restore(run);
  run.nestedExtractWorkspaceSurface("classic").rows[0].onTargetInput("  ");
  assert.equal(await run.startNestedExtract(), false);
  assert.deepEqual(run.calls.at(-1), ["focus-field", "batch-destination-0"]);
  run.nestedExtractWorkspaceSurface("modern").rows[0].onTargetInput("/edited");
  const saved = plain(run.context.nestedExtractDraft);
  let reject;
  run.context.submitJob = (job) => { run.calls.push(["submit", plain(job)]); return new Promise((_, fail) => { reject = fail; }); };
  const pending = run.startNestedExtract();
  assert.equal(run.nestedExtractWorkspaceSurface("classic").locked, true);
  run.nestedExtractWorkspaceSurface("classic").rows[0].onTargetInput("/ignored");
  run.prepareNestedExtract("/other.zip", "/other.zip", "other.7z");
  await run.startNestedExtract();
  assert.equal(run.calls.filter(([name]) => name === "submit").length, 1);
  assert.deepEqual(plain(run.context.nestedExtractDraft), saved);
  reject(new Error("service unavailable")); await pending;
  assert.equal(run.context.nestedExtractSubmissionPending, false);
  assert.deepEqual(plain(run.context.nestedExtractDraft), saved);
  assert.match(run.calls.at(-1)[1], /Could not start extraction/);
});

test("destination selection preserves cancellation and ignores responses after leaving the review", async () => {
  const run = harness(); await restore(run);
  run.context.getDialogModule = async () => ({ open: async () => "/picked" });
  await run.chooseNestedExtractDestination();
  assert.equal(run.context.nestedExtractDraft.dest, "/picked");
  run.context.getDialogModule = async () => ({ open: async () => null });
  await run.chooseNestedExtractDestination();
  assert.equal(run.context.nestedExtractDraft.dest, "/picked");
  assert.match(run.calls.at(-1)[1], /cancelled/);
  let finish;
  run.context.getDialogModule = async () => ({ open: () => new Promise((resolve) => { finish = resolve; }) });
  const pending = run.chooseNestedExtractDestination();
  await new Promise((resolve) => setImmediate(resolve));
  run.context.nestedExtractDraftGeneration++;
  finish("/stale"); await pending;
  assert.equal(run.context.nestedExtractDraft.dest, "/picked");
  assert.equal(run.context.nestedExtractPickerBusy, false);
});

test("navigation invalidates an inner destination chooser before opening and preserves newer requests", async () => {
  for (const phase of ["loading", "choosing"]) {
    for (const outcome of ["selected", "cancelled", "failed"]) {
      const run = harness(); await restore(run);
      const saved = plain(run.context.nestedExtractDraft);
      let finish, fail, started;
      const ready = new Promise(resolve => { started = resolve; });
      const oldResult = new Promise((resolve, reject) => { finish = resolve; fail = reject; });
      let opened = 0;
      run.context.getDialogModule = phase === "loading" ? () => { started(); return oldResult; }
        : async () => ({open:async()=>null});
      run.context.openNativeDialog = () => { opened++; started(); return oldResult; };
      const oldChoice = run.chooseNestedExtractDestination(); await ready;
      run.setScreen("settingsGeneral"); run.setScreen("nestedExtract");
      assert.equal(run.context.nestedExtractPickerBusy, false);
      let completeNew;
      run.context.getDialogModule = async () => ({open:async()=>null});
      run.context.openNativeDialog = () => { opened++; return new Promise(resolve => { completeNew = resolve; }); };
      const newChoice = run.chooseNestedExtractDestination();
      await new Promise(resolve => setImmediate(resolve));
      const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
      if (outcome === "failed") fail(new Error("late dialog failure"));
      else finish(phase === "loading" ? {open:async()=>"/stale"} : outcome === "cancelled" ? null : "/stale");
      await oldChoice;
      assert.equal(opened, phase === "loading" ? 1 : 2);
      assert.deepEqual(plain(run.context.nestedExtractDraft), saved);
      assert.equal(run.context.nestedExtractPickerBusy, true);
      assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
      completeNew("/new/destination"); await newChoice;
      assert.equal(run.context.nestedExtractDraft.dest, "/new/destination");
      assert.equal(run.context.nestedExtractPickerBusy, false);
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
      const { body } = render(ToolsWorkspace, { props: { surface: run.nestedExtractWorkspaceSurface(variant) } });
      assert.ok(body.includes("/Archives/外层归档.zip › Previous versions/完整内层归档.7z"));
      assert.ok(body.includes(tFallback("gui.nested_extract.outer_encoding").replace("{encoding}", "gbk")));
      assert.ok(body.includes(tFallback("gui.nested_extract.execution_hint")));
      assert.doesNotMatch(body, /old-secret|squallz-archive:|gui\.nested_extract\.|batch-remove-0/);
      assert.equal(body.includes(tFallback("gui.batch.add_archives")), false);
    }
  }
  run.context.nestedExtractDraft = null;
  const { body } = render(ToolsWorkspace, { props: { surface: run.nestedExtractWorkspaceSurface("classic") } });
  assert.ok(body.includes(tFallback("gui.nested_extract.empty_hint")));
});
