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
  add: ["/input/资料\n完整.txt"], mkdir: ["交付/审核/"], delete: ["old[1]/*.txt", "literal\\file.txt"],
  rename: [{from:"versions/旧说明.txt",to:"交付/完整说明.txt"}],
  content_policy: "custom", excludes: ["*.bak", ".DS_Store"], password: "old-secret" });

function handlers(review) {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", app.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["reviewTask", "submitArchiveUpdateReview"];
  const functions = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const calls = [];
  const context = { taskWindowMode:false, archiveUpdateReview:review, taskReviewScreen,
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

test("failed updates restore their original operations without submitting or inheriting the current archive",async()=>{
  const review=new ArchiveUpdateReview(); const run=handlers(review);
  const original=spec();
  await run.reviewTask({state:"failed",spec:original},{...original,path:"Displayed archive.zip"});
  assert.deepEqual(run.calls,[["screen","updateReview"],["dismiss"],["focus"]]);
  assert.equal(review.draft.displayPath,"Displayed archive.zip");
  assert.equal(review.draft.path,original.path);
  assert.equal("password" in review.draft,false);
  assert.equal(taskReviewScreen({state:"done",spec:original}),null);
  await run.submitArchiveUpdateReview();
  assert.deepEqual(run.calls.find(([kind])=>kind==="submit")[1],{...original,password:null});
  original.add[0]="/changed"; original.rename[0].to="changed"; original.excludes.push("*");
  assert.equal(review.draft.operations[0].value,"/input/资料\n完整.txt");
  assert.deepEqual(plain(review.draft.excludes),["*.bak",".DS_Store"]);
});

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

test("both layouts expose the original archive, every operation type, empty state, and localized actions",async()=>{
  const {render}=await server.ssrLoadModule("svelte/server");
  const {default:Workspace}=await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const {loadLocale,tFallback}=await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const review=new ArchiveUpdateReview();review.restore(spec(),"Shown/完整外层路径.zip");
  for(const locale of ["en-US","zh-CN"]){
    await loadLocale(locale);
    for(const variant of ["modern","classic"]){
      const surface={kind:"update",variant,title:tFallback("gui.update_review.title"),tr:tFallback,review,
        archiveReturn:{visible:false},policyLabel:()=>"",onReady(){},onSubmit:async()=>{},onOpenTasks(){}};
      const {body}=render(Workspace,{props:{surface}});
      assert.ok(body.includes("Shown/完整外层路径.zip"));
      assert.ok(body.includes("old[1]/*.txt"));assert.ok(body.includes("literal\\file.txt"));
      assert.ok(body.includes(tFallback("gui.update_review.hint")));
      assert.ok(body.includes('id="update-operation-4"'));
      assert.doesNotMatch(body,/old-secret|\/original\/历史归档|style=|gui\.update_review\./);
      const empty=render(Workspace,{props:{surface:{...surface,review:new ArchiveUpdateReview()}}}).body;
      assert.ok(empty.includes(tFallback("gui.update_review.empty_hint")));
    }
  }
});
