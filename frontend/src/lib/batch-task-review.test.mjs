import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { batchExtractJob, reviewBatchExtract } = await server.ssrLoadModule("/src/lib/batch-extract.ts");
const { readBatchExtractResult } = await server.ssrLoadModule("/src/lib/batch-extract-result.ts");
const { sameDesktopPath, desktopBasename, desktopDirname } = await server.ssrLoadModule("/src/lib/desktop-path.ts");

function spec() {
  return { kind: "batch_extract", overwrite: "rename", symlinks: "skip", smart: false,
    items: [
      { path: "/original/季度归档与设计资料/完整文件名.zip", dest: "/output/zip", encoding: "gbk", best_effort: true, password: "old-password" },
      { path: "/original/photos.7z", dest: "/output/photos", encoding: null, best_effort: false, password: null },
      { path: "/original/logs.tar", dest: "/another/logs", encoding: null, best_effort: false, password: null },
    ] };
}

const plain = (value) => JSON.parse(JSON.stringify(value));

function harness() {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["reviewTask", "newBatchExtractDraft", "effectiveBatchDraft", "batchDraftLocked", "setBatchArchivePaths",
    "updateBatchDraft", "removeBatchItem", "chooseBatchPaths", "startBatchExtract", "batchWorkspaceSurface", "setScreen"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = {
    batchDraft: null, batchSubmissionPending: false, batchPickerBusy: false,
    batchDraftGeneration: 0, batchReviewFocusPending: false, batchPickerRequest: 0,
    nestedExtractPickerBusy: false, nestedExtractPickerRequest: 0, nestedExtractDraftGeneration: 0,
    archiveUpdateReview: { cancelSourceChoice() {} }, taskReviewRequestGeneration: 0,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {},
    screen: "browse", taskWindowMode: false, runtimePreviews: {batchPaths:[]},
    currentArchive: {source:"/unrelated/current.zip"}, appliedDefaultExtractDir: "",
    uniqueNonEmptyPaths: (paths) => [...new Set(paths.filter(Boolean))],
    normalizedDefaultExtractDir: (path) => path || null,
    sameFilePath: (a,b) => sameDesktopPath(a,b,"macos"),
    pathDir: (path) => desktopDirname(path,"macos"),
    pathBaseName: (path) => desktopBasename(path,"macos"),
    archiveFormatFromPath: (path) => path.split(".").at(-1).toUpperCase(),
    archiveEncodingForJob: () => "shift_jis", platformKind: () => "macos",
    tr: (_key, fallback) => fallback, batchExtractJob, reviewBatchExtract, taskReviewScreen, readBatchExtractResult,
    preventCreateSubmissionNavigation: () => false, preventConvertSubmissionNavigation: () => false,
    focusBlockingTaskIfAny: () => false,
    setScreen: (screen) => { context.screen=screen; calls.push(["screen",screen]); },
    dismissTaskDialog: async () => { calls.push(["dismiss"]); },
    focusBatchReview: () => { calls.push(["focus"]); },
    showNotice: (message) => calls.push(["notice",message]), recordOperation() {},
    document: {documentElement:{},body:{},querySelectorAll:()=>[],getElementById:(id) => id.endsWith("--1") ? null : ({focus:() => calls.push(["focus-field",id])})},
    tick: async () => {},
    submitJob: async (job) => { calls.push(["submit",plain(job)]); return 99; },
    isJobSubmitBlocked: () => false, isErrorDto: () => false, tError: () => "error",
    toolsArchiveReturnSurface: () => ({visible:false}),
    extractOverwriteModes: ["ask","skip","overwrite","rename"], extractOverwriteLabel: (mode) => mode,
    extractSymlinkModes: ["preserve","skip","follow"], extractSymlinkLabel: (mode) => mode,
    getDialogModule: async () => ({open:async () => null}),
    openNativeDialog: async (_key,open,options) => open(options),
  };
  const {outputText} = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS},
  });
  return {...vm.runInNewContext(`${outputText}\n({${names.join(",")}})`,context),context,calls};
}

test("partially successful batches expose a review action for failed archives", () => {
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 1, extracted: 2 } }), "batch");
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 0, extracted: 3 } }), null);
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 0, extracted: 3, outputs: [{ archive: "damaged.zip", counts: { failed: 1 } }] } }), "batch");
});

for (const state of ["failed", "cancelled"]) {
test(`${state} batch review restores original nonsecret settings; edits are exactly what is submitted`, async () => {
  const run = harness();
  await run.reviewTask({state,spec:spec(),result:null});
  assert.equal(run.context.screen,"batch");
  assert.equal(run.calls.some(([name]) => name === "submit"),false);
  let surface = run.batchWorkspaceSurface("modern");
  assert.deepEqual(Array.from(surface.rows,(row)=>row.target),spec().items.map((item)=>item.dest));
  assert.equal(surface.rows[0].encoding,"gbk");
  assert.equal(surface.rows[0].bestEffort,true);
  assert.equal(surface.overwrite,"rename");
  assert.equal(surface.symlinks,"skip");
  assert.equal(surface.smart,false);
  surface.rows[0].onTargetInput("/edited/完整目标");
  surface.onSmartChange(true);
  surface.onOverwriteChange("ask");
  surface.onSymlinksChange("preserve");
  surface = run.batchWorkspaceSurface("classic");
  await run.startBatchExtract();
  const submitted = run.calls.find(([name])=>name==="submit")[1];
  assert.deepEqual(submitted.items.map((item)=>item.dest),Array.from(surface.rows,(row)=>row.target));
  assert.equal(submitted.items[0].dest,"/edited/完整目标");
  assert.equal(submitted.items[0].encoding,"gbk");
  assert.equal(submitted.items[0].best_effort,true);
  assert.equal(submitted.smart,true);
  assert.equal(submitted.overwrite,"ask");
  assert.equal(submitted.symlinks,"preserve");
  assert.ok(submitted.items.every((item)=>item.password===null));
  assert.equal("password" in run.context.batchDraft.items[0],false);
});
}

test("partial success restores only identified failures and ambiguous results preserve the current draft", async () => {
  const run = harness();
  const job=spec();
  await run.reviewTask({state:"done",spec:job,result:{failed:2,failures:[{archive:job.items[2].path},{archive:job.items[0].path}]}});
  assert.deepEqual(Array.from(run.context.batchDraft.items,(item)=>item.path),[job.items[0].path,job.items[2].path]);
  const saved=plain(run.context.batchDraft);
  for (const failures of [[],[{archive:"/unknown.zip"}],[{archive:job.items[0].path},{archive:job.items[0].path}]]) {
    await run.reviewTask({state:"done",spec:job,result:{failed:2,failures}});
    assert.deepEqual(plain(run.context.batchDraft),saved);
    assert.match(run.calls.at(-1)[1],/cannot identify/);
  }
  const duplicate={...job,items:[job.items[0],{...job.items[0],dest:"/different"}]};
  assert.equal(reviewBatchExtract(duplicate,[{archive:job.items[0].path}],1,"macos"),null);
  const windows={...job,items:[{...job.items[0],path:"C:\\Sources\\A.zip"}]};
  assert.equal(reviewBatchExtract(windows,[{archive:"c:/sources/a.zip"}],1,"windows").items.length,1);
  assert.equal(reviewBatchExtract(windows,[{archive:"c:/sources/a.zip"}],1,"linux"),null);
  assert.equal(run.calls.some(([name])=>name==="submit"),false);
});

test("best-effort batches restore only archives with failed entries or archive failures", async () => {
  const run = harness();
  const job = spec();
  const result = { failed: 1, failures: [{ archive: job.items[2].path }], outputs: [
    { archive: job.items[0].path, counts: { failed: 2, skipped: 1 } },
    { archive: job.items[1].path, counts: { failed: 0, skipped: 1 } },
  ] };
  await run.reviewTask({ state: "done", spec: job, result });
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => item.path), [job.items[0].path, job.items[2].path]);
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  assert.match(run.calls.at(-1)[1], /from the beginning/);
  await run.startBatchExtract();
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.deepEqual(submitted.items.map((item) => item.path), [job.items[0].path, job.items[2].path]);
  assert.ok(submitted.items.every((item) => item.password === null));
  const saved = plain(run.context.batchDraft);
  await run.reviewTask({ state: "done", spec: job, result: { failed: 0, outputs: [
    { archive: job.items[0].path, counts: { failed: 1 } },
    { archive: job.items[0].path, counts: { failed: 1 } },
  ] } });
  assert.deepEqual(plain(run.context.batchDraft), saved);
  assert.match(run.calls.at(-1)[1], /cannot identify/);
});

test("new batches use the displayed smart base and an explicitly empty list never falls back to the current archive", async () => {
  const run=harness();
  run.context.screen="batch";
  run.setBatchArchivePaths(["/inbox/backup.zip","/other/photos.7z"]);
  assert.deepEqual(Array.from(run.effectiveBatchDraft().items,(item)=>item.dest),["/inbox","/other"]);
  run.context.appliedDefaultExtractDir="/preferred";
  run.setBatchArchivePaths(["/inbox/backup.zip"]);
  assert.equal(run.effectiveBatchDraft().items[0].dest,"/preferred");
  run.removeBatchItem(0);
  await run.startBatchExtract();
  assert.equal(run.effectiveBatchDraft().items.length,0);
  assert.equal(run.calls.some(([name])=>name==="submit"),false);
  assert.ok(run.calls.some(([name,id])=>name==="focus-field"&&id==="batch-workspace-heading"));
});

test("batch review matches displayed failures while submitting the original opaque source", async () => {
  const run=harness();
  const displayed=spec();
  const actual={...displayed,items:displayed.items.map((item,index)=>({...item,path:`squallz-archive://${index+1}`}))};
  await run.reviewTask({state:"done",spec:actual,result:{failed:1,failures:[{archive:displayed.items[0].path}]}},displayed);
  const surface=run.batchWorkspaceSurface("modern");
  assert.equal(surface.rows.length,1);
  assert.equal(surface.rows[0].path,displayed.items[0].path);
  assert.equal(surface.rows[0].name,"完整文件名.zip");
  await run.startBatchExtract();
  const submitted=run.calls.find(([name])=>name==="submit")[1];
  assert.equal(submitted.items[0].path,"squallz-archive://1");
  assert.equal("displayPath" in submitted.items[0],false);
  run.context.currentArchive={source:"squallz-archive://8",path:"/Archives/outer.zip › inner.zip"};
  run.context.appliedDefaultExtractDir="";
  run.setBatchArchivePaths([run.context.currentArchive.source]);
  assert.equal(run.effectiveBatchDraft().items[0].dest,"/Archives");
  assert.equal(run.batchWorkspaceSurface("classic").rows[0].path,run.context.currentArchive.path);
});

test("submission locks a batch against edits, duplicate submits and incoming selections; errors preserve its draft", async () => {
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  const saved=plain(run.context.batchDraft);
  let reject;
  run.context.submitJob=(job)=>{run.calls.push(["submit",plain(job)]);return new Promise((_,fail)=>{reject=fail;});};
  const pending=run.startBatchExtract();
  assert.equal(run.batchWorkspaceSurface("modern").locked,true);
  run.batchWorkspaceSurface("classic").rows[0].onTargetInput("/ignored");
  assert.equal(run.setBatchArchivePaths(["/ignored.zip"]),false);
  await run.startBatchExtract();
  await run.reviewTask({state:"failed",spec:{...spec(),items:[spec().items[1]]}});
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.equal(run.calls.filter(([name])=>name==="submit").length,1);
  reject(new Error("not connected"));
  await pending;
  assert.equal(run.batchWorkspaceSurface("classic").locked,false);
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.match(run.calls.at(-1)[1],/desktop service/);
  run.batchWorkspaceSurface("modern").rows[1].onTargetInput("  ");
  await run.startBatchExtract();
  assert.deepEqual(run.calls.at(-1),["focus-field","batch-destination-1"]);
  assert.equal(run.calls.filter(([name])=>name==="submit").length,1);
});

test("native selection adds without duplication, changes the chosen destination and ignores stale responses", async () => {
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  run.context.getDialogModule=async()=>({open:async(options)=>options.directory?"/picked": [spec().items[0].path,"/new/new.zip"]});
  await run.chooseBatchPaths();
  assert.equal(run.context.batchDraft.items.length,4);
  await run.chooseBatchPaths(1);
  assert.equal(run.context.batchDraft.items[1].dest,"/picked");
  const saved=plain(run.context.batchDraft);
  run.context.getDialogModule=async()=>({open:async()=>null});
  await run.chooseBatchPaths();
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.match(run.calls.at(-1)[1],/cancelled/);
  let finish;
  run.context.getDialogModule=async()=>({open:()=>new Promise((resolve)=>{finish=resolve;})});
  const pending=run.chooseBatchPaths();
  await new Promise((resolve)=>setImmediate(resolve));
  run.context.screen="browse";
  finish(["/stale.zip"]);
  await pending;
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.equal(run.context.batchPickerBusy,false);
});

test("leaving batch review abandons loading and open choosers without unlocking a newer choice", async () => {
  for (const index of [null, 1]) {
    for (const phase of ["loading", "choosing"]) {
      for (const outcome of ["selected", "cancelled", "failed"]) {
        const run = harness(); await run.reviewTask({state:"failed",spec:spec()});
        const saved = plain(run.context.batchDraft);
        let finish, fail, started;
        const ready = new Promise(resolve => { started = resolve; });
        const oldResult = new Promise((resolve, reject) => { finish = resolve; fail = reject; });
        let opened = 0;
        run.context.getDialogModule = phase === "loading" ? () => { started(); return oldResult; }
          : async () => ({open:async()=>null});
        run.context.openNativeDialog = () => { opened++; started(); return oldResult; };
        const oldChoice = run.chooseBatchPaths(index); await ready;
        assert.equal(run.context.batchPickerBusy, true);
        run.setScreen("settingsGeneral"); run.setScreen("batch");
        assert.equal(run.context.batchPickerBusy, false, "navigation must release the abandoned chooser");
        let completeNew;
        run.context.getDialogModule = async () => ({open:async()=>null});
        run.context.openNativeDialog = () => { opened++; return new Promise(resolve => { completeNew = resolve; }); };
        const newChoice = run.chooseBatchPaths(index);
        await new Promise(resolve => setImmediate(resolve));
        const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
        if (outcome === "failed") fail(new Error("late dialog failure"));
        else finish(phase === "loading" ? {open:async()=>"/stale"} : outcome === "cancelled" ? null : ["/stale"]);
        await oldChoice;
        assert.equal(opened, phase === "loading" ? 1 : 2, "abandoned module loads must not open a dialog");
        assert.deepEqual(plain(run.context.batchDraft), saved);
        assert.equal(run.context.batchPickerBusy, true, "old completion must not unlock the new chooser");
        assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
        completeNew(index === null ? ["/new/archive.zip"] : "/new/destination"); await newChoice;
        assert.equal(run.context.batchPickerBusy, false);
        if (index === null) assert.equal(run.context.batchDraft.items.at(-1).path, "/new/archive.zip");
        else assert.equal(run.context.batchDraft.items[index].dest, "/new/destination");
      }
    }
  }
});

test("both layouts show editable targets and honest checks in both languages without dropping full source paths", async () => {
  const {render}=await server.ssrLoadModule("svelte/server");
  const {default:ToolsWorkspace}=await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const {loadLocale,tFallback}=await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  for(const locale of ["en-US","zh-CN"]) {
    await loadLocale(locale);run.context.tr=tFallback;
    for(const variant of ["modern","classic"]) {
      const surface=run.batchWorkspaceSurface(variant);
      const {body}=render(ToolsWorkspace,{props:{surface}});
      assert.ok(body.includes(spec().items[0].path));
      assert.ok(body.includes('value="/output/zip"'));
      assert.ok(body.includes(tFallback("gui.batch.execution_hint")));
      assert.ok(body.includes(tFallback("gui.batch.add_archives")));
      assert.doesNotMatch(body,/gui\.batch\.|old-password|100%|Ready to start|batch-readiness/);
    }
  }
});
