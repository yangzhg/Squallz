import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ArchiveUpdateReview } = await server.ssrLoadModule("/src/lib/archive-update.svelte.ts");
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");
const plain = (value) => JSON.parse(JSON.stringify(value));
const spec = () => ({ kind: "update", path: "/original/历史归档.zip", encoding: "gbk", level: 3,
  expected_archive_id: null,
  add: ["/input/资料\n完整.txt"], mkdir: ["交付/审核/"], delete: ["old[1]/*.txt", "literal\\file.txt"],
  rename: [{from:"versions/旧说明.txt",to:"交付/完整说明.txt"}],
  content_policy: "custom", excludes: ["*.bak", ".DS_Store"], password: "old-secret" });

function handlers(review) {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["reviewTask", "submitArchiveUpdateReview", "chooseArchiveUpdateSource"];
  const functions = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = { taskWindowMode:false, archiveUpdateReview:review, taskReviewScreen,
    screen:"updateReview", getDialogModule:async()=>({open:async()=>null}),
    openNativeDialog:async(kind,open,options)=>{calls.push(["picker",kind,options]);return open(options);},
    preventCreateSubmissionNavigation:()=>false, preventConvertSubmissionNavigation:()=>false,
    focusBlockingTaskIfAny:()=>false, setScreen:(screen)=>calls.push(["screen",screen]),
    dismissTaskDialog:async()=>calls.push(["dismiss"]), focusArchiveUpdateReview:()=>calls.push(["focus"]),
    showNotice:(message)=>calls.push(["notice",message]), pushToast:(toast)=>calls.push(["toast",toast]),
    tr:(_key,fallback)=>fallback, submitJob:async(job)=>{calls.push(["submit",plain(job)]);return 1;},
    isJobSubmitBlocked:()=>false, isErrorDto:()=>false, tError:()=>"error" };
  const {outputText}=ts.transpileModule(functions.map((node)=>node.getText(source)).join("\n"),{
    compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS},
  });
  return {...vm.runInNewContext(`${outputText}\n({${names.join(",")}})`,context),context,calls};
}

function workspaceHandlers(review, overrides = {}) {
  const component = readFileSync(new URL("../components/ArchiveUpdateWorkspace.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("Workspace.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const functions = source.statements.filter((node) => ts.isFunctionDeclaration(node) && ["submit", "changePage"].includes(node.name?.text));
  const reset = source.statements.find((node) => ts.isExpressionStatement(node)
    && node.getText(source).startsWith("$effect(") && node.getText(source).includes("const restoredDraft = draft"));
  assert.ok(reset);
  const focused = [];
  const submitted = [];
  let restoreView;
  const context = { review, draft: review.draft, pageSize: 50,
    pageCount: Math.ceil(review.draft.operations.length / 50), pageSelection: null, advancedOpen: false,
    body: { isConnected: true, scrollTop: 100 },
    heading: { focus: () => focused.push("heading") }, tick: async () => {},
    $effect: (effect) => { restoreView = effect; },
    document: { getElementById: (id) => ({ focus: () => focused.push(id) }) },
    surface: { onSubmit: () => review.submit(async (job) => { submitted.push(plain(job)); }) },
    ...overrides };
  const { outputText } = ts.transpileModule([...functions, reset].map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return { ...vm.runInNewContext(`${outputText}\n({submit,changePage})`, context), context, focused, submitted,
    restoreView: () => restoreView() };
}

for (const state of ["failed", "cancelled"]) {
test(`${state} updates restore their original operations without submitting or inheriting the current archive`,async()=>{
  const review=new ArchiveUpdateReview(); const run=handlers(review);
  const original={...spec(), expected_archive_id: 123};
  await run.reviewTask({state,spec:original},{...original,path:"Displayed archive.zip"});
  assert.deepEqual(run.calls,[["screen","updateReview"],["dismiss"],["focus"]]);
  assert.equal(review.draft.displayPath,"Displayed archive.zip");
  assert.equal(review.draft.path,original.path);
  assert.equal("password" in review.draft,false);
  assert.equal("expected_archive_id" in review.draft,false);
  assert.equal(taskReviewScreen({state:"done",spec:original}),null);
  await run.submitArchiveUpdateReview();
  assert.deepEqual(run.calls.find(([kind])=>kind==="submit")[1],{...original,password:null,expected_archive_id:null});
  original.add[0]="/changed"; original.rename[0].to="changed"; original.excludes.push("*");
  assert.equal(review.draft.operations[0].value,"/input/资料\n完整.txt");
  assert.deepEqual(plain(review.draft.excludes),["*.bak",".DS_Store"]);
});
}

test("edits and deselection submit exactly the reviewed values while preserving literal deletion targets",async()=>{
  const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
  review.editOperation(0,{value:"/replacement/新资料.txt"});
  review.editOperation(1,{enabled:false});
  review.editOperation(4,{value:"交付/Final notes.txt"});
  review.editSettings({encoding:"utf-8",level:"0",contentPolicy:"keep_all_files"});
  review.editExcludes("*.tmp\n");
  assert.equal(review.draft.excludesText,"*.tmp\n");
  let job; await review.submit(async(value)=>{job=plain(value);});
  assert.deepEqual(job,{...spec(),password:null,encoding:"utf-8",level:0,content_policy:"keep_all_files",excludes:["*.tmp"],
    add:["/replacement/新资料.txt"],mkdir:[],rename:[{from:"versions/旧说明.txt",to:"交付/Final notes.txt"}]});
  review.editOperation(1,{enabled:true});
  assert.equal(review.selectedCount(),5);
});

test("invalid fields and an empty selection never submit an implicit set of archive changes",async()=>{
  for(const [field,value,kind] of [[0,"  ","empty"],[1,"../outside/","path"],[4,"versions/旧说明.txt","unchanged"],["level","1.5","level"],["level","","level"]]){
    const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
    if(field==="level")review.editSettings({level:value});else review.editOperation(field,{value});
    assert.equal(await review.submit(async()=>assert.fail("invalid changes submitted")),false);
    assert.equal(review.issue.kind,kind);
    assert.equal(review.issue.field,field==="level"?"update-level":`update-operation-${field}`);
  }
  const review=new ArchiveUpdateReview();
  assert.equal(await review.submit(async()=>assert.fail()),false);
  review.restore(spec(),"Original.zip");
  for(const row of review.draft.operations)review.editOperation(row.id,{enabled:false});
  assert.equal(await review.submit(async()=>assert.fail()),false);
  assert.equal(review.issue.kind,"selection");
});

test("absolute archive targets are rejected before queuing while literal sources remain intact", async () => {
  for (const value of ["/outside/", "/", "\\outside\\", "//server/share/folder/", "\\\\server\\share\\folder\\"]) {
    for (const id of [1, 4]) {
      const review = new ArchiveUpdateReview();
      review.restore(spec(), "Original.zip");
      review.editOperation(id, { value });
      assert.equal(await review.submit(async () => assert.fail("absolute target queued")), false, `${id}: ${value}`);
      assert.deepEqual(plain(review.issue), { field: `update-operation-${id}`, kind: "path" });
      assert.equal(review.draft.operations[id].value, value, "keep the invalid input for correction");
    }
  }
  const review = new ArchiveUpdateReview();
  review.restore(spec(), "Original.zip");
  review.editOperation(1, { value: "交付//审核/" });
  review.editOperation(4, { value: "交付\\完整说明.txt" });
  let queued;
  assert.equal(await review.submit(async (value) => { queued = plain(value); }), true);
  assert.deepEqual(queued.add, spec().add);
  assert.deepEqual(queued.delete, spec().delete);
  assert.deepEqual(queued.mkdir, ["交付//审核/"]);
  assert.deepEqual(queued.rename, [{ from: "versions/旧说明.txt", to: "交付\\完整说明.txt" }]);
});

test("a late invalid operation opens its page without expanding the whole batch or losing other selections", async () => {
  const review = new ArchiveUpdateReview();
  const original = { ...spec(), add: [spec().add[0]], delete: [spec().delete[1]],
    mkdir: Array.from({ length: 4997 }, (_, index) => `folders/item-${index}/`),
    rename: [{ from: "original.txt", to: "" }] };
  review.restore(original, "Original.zip");
  const before = plain(review.draft);
  const run = workspaceHandlers(review);
  await run.submit();
  assert.equal(run.context.pageSelection.index, 99);
  assert.equal(run.context.pageSelection.draft, review.draft);
  assert.deepEqual(run.focused, ["update-operation-4999"]);
  assert.deepEqual(plain(review.draft), before);
  assert.deepEqual(run.submitted, []);

  review.editOperation(4999, { value: "revised/完整说明.txt" });
  review.editOperation(0, { enabled: false });
  await run.changePage(0);
  assert.equal(run.context.pageSelection.index, 0);
  assert.equal(run.context.body.scrollTop, 0);
  assert.equal(run.focused.at(-1), "heading");
  await run.changePage(999);
  assert.equal(run.context.pageSelection.index, 99);
  await run.submit();
  assert.equal(run.submitted.length, 1);
  assert.deepEqual(run.submitted[0], { ...original, add: [], password: null,
    rename: [{ from: "original.txt", to: "revised/完整说明.txt" }] });
});

test("validation focus stays within the current workspace and reveals compression errors", async () => {
  const review = new ArchiveUpdateReview();
  review.restore(spec(), "Original.zip");
  review.editSettings({ level: "1.5" });
  const run = workspaceHandlers(review);
  await run.submit();
  assert.equal(run.context.advancedOpen, true);
  assert.deepEqual(run.focused, ["update-level"]);
  run.context.tick = async () => { run.context.body.isConnected = false; };
  await run.submit();
  assert.deepEqual(run.focused, ["update-level"], "a removed workspace cannot focus a newer page");
});

test("a newly restored draft resets paging, options, and scroll without letting an old restoration move it", async () => {
  const review = new ArchiveUpdateReview();
  review.restore(spec(), "Original.zip");
  const completions = [];
  const run = workspaceHandlers(review, { tick: () => new Promise((resolve) => { completions.push(resolve); }) });
  run.context.pageSelection = { draft: review.draft, index: 99 };
  run.context.advancedOpen = true;
  run.restoreView();
  assert.equal(run.context.pageSelection, null);
  assert.equal(run.context.advancedOpen, false);
  review.restore({ ...spec(), path: "/another.zip" }, "Another.zip");
  run.context.draft = review.draft;
  completions.shift()();
  await Promise.resolve();
  assert.equal(run.context.body.scrollTop, 100, "an older draft cannot scroll a newer review");
  run.restoreView();
  completions.shift()();
  await Promise.resolve();
  assert.equal(run.context.body.scrollTop, 0);
  assert.deepEqual(run.submitted, []);
});

test("large update reviews render one page, preserve list positions, and place errors beside their field", async () => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: Workspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const review = new ArchiveUpdateReview();
  review.restore({ ...spec(), mkdir: Array.from({ length: 4995 }, (_, index) => `folders/item-${index}/`) }, "Original.zip");
  review.editOperation(1, { value: "/outside/" });
  await review.submit(async () => assert.fail("invalid batch queued"));
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    for (const variant of ["modern", "classic"]) {
      const surface = { kind: "update", variant, title: tFallback("gui.update_review.title"), tr: tFallback, review,
        archiveReturn: { visible: false }, policyLabel: () => "", onReady() {}, onSubmit: async () => {}, onChooseSource: async () => {}, onOpenTasks() {} };
      const body = render(Workspace, { props: { surface } }).body;
      assert.equal((body.match(/class="operation-card"/g) ?? []).length, 50);
      assert.ok(body.includes('aria-setsize="4999"'));
      assert.ok(body.includes('aria-posinset="50"'));
      assert.ok(body.includes(tFallback("gui.update_review.last_page")));
      assert.ok(body.includes(tFallback("gui.update_review.pages")));
      const target = body.indexOf('id="update-operation-1"');
      const error = body.indexOf('id="update-review-error"');
      const next = body.indexOf('id="update-operation-2"');
      assert.ok(target < error && error < next, "the explanation belongs to the invalid operation");
      assert.equal((body.match(/id="update-review-error"/g) ?? []).length, 1);
      assert.doesNotMatch(body, /style=|gui\.update_review\./);
    }
  }
});

test("pending updates reject edits, another review, and duplicate submission; failure retains the draft",async()=>{
  const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
  const snapshot=plain(review.draft);let fail;
  const pending=review.submit(()=>new Promise((_,reject)=>{fail=reject;}));
  review.editOperation(0,{enabled:false});review.editSettings({level:"9"});review.editExcludes("*");
  assert.equal(review.restore({...spec(),path:"/other.zip"},"Other.zip"),false);
  assert.equal(await review.submit(async()=>assert.fail("duplicate")),false);
  assert.deepEqual(plain(review.draft),snapshot);
  fail(new Error("unavailable"));await assert.rejects(pending,/unavailable/);
  assert.equal(review.pending,false);assert.deepEqual(plain(review.draft),snapshot);
  const run=handlers(review);run.context.submitJob=async()=>{throw new Error("offline");};
  await run.submitArchiveUpdateReview();
  assert.match(run.calls.at(-1)[1].body,/desktop service/);
  assert.deepEqual(plain(review.draft),snapshot);
});

test("navigation guards and task windows do not replace an existing update draft",async()=>{
  for(const flag of ["taskWindowMode","preventCreateSubmissionNavigation","preventConvertSubmissionNavigation","focusBlockingTaskIfAny"]){
    const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
    const run=handlers(review);run.context[flag]=flag==="taskWindowMode"?true:()=>true;
    await run.reviewTask({state:"failed",spec:{...spec(),path:"/other.zip"}});
    assert.equal(review.draft.path,spec().path);assert.deepEqual(run.calls,[]);
  }
});

test("source selection replaces only its add operation and submits the chosen literal path",async()=>{
  for(const kind of ["file","folder"]){
    const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
    const before=plain(review.draft);const run=handlers(review);
    const selected=kind==="file"?"/revised/资料\n完整.txt":"/revised/交付文件夹";
    run.context.getDialogModule=async()=>({open:async()=>selected});
    await run.chooseArchiveUpdateSource(0,kind);
    assert.deepEqual(plain(review.draft),{...before,operations:before.operations.map(row=>row.id===0?{...row,value:selected}:row)});
    assert.deepEqual(plain(review.sourceFeedback),{id:0,kind:"selected"});
    assert.equal(run.calls[0][2].multiple,false);
    assert.equal(run.calls[0][2].directory,kind==="folder");
    assert.equal(run.calls[0][2].defaultPath,spec().add[0]);
    await run.submitArchiveUpdateReview();
    assert.deepEqual(run.calls.find(([kind])=>kind==="submit")[1],{...spec(),password:null,add:[selected]});
  }
});

test("cancelled or failed source choices keep the draft and allow retry; disabled rows never choose",async()=>{
  const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
  const before=plain(review.draft);
  for(const failure of [false,true]){
    await review.chooseSource(0,async()=>{if(failure)throw new Error("unavailable");return null;});
    assert.deepEqual(plain(review.draft),before);
    assert.equal(review.sourceFeedback.kind,failure?"failed":"cancelled");
    assert.equal(review.sourcePicking,null);
  }
  review.editOperation(0,{enabled:false});
  let unavailableCalls=0;
  for(const id of [0,1,2,99])await review.chooseSource(id,async()=>{unavailableCalls++;return "/wrong";});
  assert.equal(unavailableCalls,0,"unavailable operations must not open the chooser");
  review.editOperation(0,{enabled:true});
  await review.chooseSource(0,async()=>"/corrected");
  assert.equal(review.draft.operations[0].value,"/corrected");
  review.editOperation(0,{value:"/manually edited"});
  assert.equal(review.sourceFeedback,null);
});

test("pending source choices cannot submit, overlap, or replace newer drafts and edits",async()=>{
  for(const change of ["cancel","restore","edit","disable"]){
    for(const fails of [false,true]){
      const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
      let resolve,reject;
      const pending=review.chooseSource(0,()=>new Promise((yes,no)=>{resolve=yes;reject=no;}));
      assert.equal(review.sourcePicking,0);
      assert.equal(await review.submit(async()=>assert.fail("submitted with a pending source")),false);
      let duplicateCalls=0;
      await review.chooseSource(0,async()=>{duplicateCalls++;return "/duplicate";});
      assert.equal(duplicateCalls,0);
      if(change==="cancel")review.cancelSourceChoice();
      if(change==="restore")review.restore({...spec(),path:"/new.zip",add:["/new-source"]},"New.zip");
      if(change==="edit")review.editOperation(0,{value:"/edited"});
      if(change==="disable")review.editOperation(0,{enabled:false});
      const expected=plain(review.draft);
      if(fails)reject(new Error("late"));else resolve("/late");
      await pending;
      assert.deepEqual(plain(review.draft),expected);
      assert.equal(review.sourceFeedback,null);
      assert.equal(review.sourcePicking,null);
    }
  }
  const review=new ArchiveUpdateReview();review.restore(spec(),"Original.zip");
  const run=handlers(review);let loaded;
  run.context.getDialogModule=()=>new Promise(resolve=>{loaded=resolve;});
  const pending=run.chooseArchiveUpdateSource(0,"file");
  review.cancelSourceChoice();run.context.screen="browse";
  loaded({open:async()=>assert.fail("stale chooser opened")});await pending;
  assert.deepEqual(run.calls,[]);
  let release;
  const submitting=review.submit(()=>new Promise(resolve=>{release=resolve;}));
  let submittingCalls=0;
  await review.chooseSource(0,async()=>{submittingCalls++;return "/during-submit";});
  assert.equal(submittingCalls,0);
  release(1);await submitting;
  let oldResult,newResult;
  const oldChoice=review.chooseSource(0,()=>new Promise(resolve=>{oldResult=resolve;}));
  review.cancelSourceChoice();
  const newChoice=review.chooseSource(0,()=>new Promise(resolve=>{newResult=resolve;}));
  oldResult("/old");await oldChoice;
  assert.equal(review.sourcePicking,0,"old completion must not unlock the newer chooser");
  assert.equal(review.sourceFeedback,null);
  newResult("/new");await newChoice;
  assert.equal(review.draft.operations[0].value,"/new");
});

test("both layouts expose the original archive, every operation type, empty state, and localized actions",async()=>{
  const {render}=await server.ssrLoadModule("svelte/server");
  const {default:Workspace}=await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const {loadLocale,tFallback}=await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const review=new ArchiveUpdateReview();review.restore(spec(),"Shown/完整外层路径.zip");
  for(const locale of ["en-US","zh-CN"]){
    await loadLocale(locale);
    for(const variant of ["modern","classic"]){
      const surface={kind:"update",variant,title:tFallback("gui.update_review.title"),tr:tFallback,review,
        archiveReturn:{visible:false},policyLabel:()=>"",onReady(){},onSubmit:async()=>{},onChooseSource:async()=>{},onOpenTasks(){}};
      const {body}=render(Workspace,{props:{surface}});
      assert.ok(body.includes("Shown/完整外层路径.zip"));
      assert.ok(body.includes("old[1]/*.txt"));assert.ok(body.includes("literal\\file.txt"));
      assert.ok(body.includes(tFallback("gui.update_review.hint")));
      assert.ok(body.includes('id="update-operation-4"'));
      assert.ok(body.includes(tFallback("gui.update_review.choose_file")));
      assert.ok(body.includes(tFallback("gui.update_review.choose_folder")));
      assert.doesNotMatch(body,/old-secret|\/original\/历史归档|style=|gui\.update_review\./);
      const empty=render(Workspace,{props:{surface:{...surface,review:new ArchiveUpdateReview()}}}).body;
      assert.ok(empty.includes(tFallback("gui.update_review.empty_hint")));
    }
  }
});
