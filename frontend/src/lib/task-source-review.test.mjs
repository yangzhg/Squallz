import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

function harness() {
  const component=readFileSync(new URL("../App.svelte",import.meta.url),"utf8");
  const source=ts.createSourceFile("App.ts",component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1],ts.ScriptTarget.Latest,true);
  const names=["prepareTaskReview","closeTaskCenter","openTaskCenterDetails","returnToTaskCenter","adoptRecoveryTargetFromTask","testTaskUsesRecoveryContext"];
  const declarations=source.statements.filter((node)=>ts.isFunctionDeclaration(node)&&names.includes(node.name?.text));
  const calls=[];
  const context={taskWindowMode:false,taskReviewRequestGeneration:0,screen:"browse",currentArchive:{id:1},
    taskReviewScreen:()=>"extract",previewTaskSpecForReview:()=>null,
    ipc:{reviewJobSpec:async(id)=>{calls.push(["resolve",id]);return {kind:"extract",path:`squallz-archive://${id}`};}},
    reviewTask:async(task,displayed)=>calls.push(["restore",task.spec,displayed]),
    showNotice:(message)=>calls.push(["notice",message]),tr:(_key,fallback)=>fallback,
    taskCenterReturnFocus:null,taskCenterOpen:true,taskCenterSelectedTaskId:7,taskCenterFocusTaskId:null,
    tick:()=>Promise.resolve(),sameFilePath:(left,right)=>left===right,
    recoverySourceMode:"none",recoverySourceOverride:null,recoveryPar2Override:null,
    recoverySourcePath:()=>null,recoveryContextTaskIds:new Set(),
  };
  const {outputText}=ts.transpileModule(declarations.map(node=>node.getText(source)).join("\n"),{
    compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS},
  });
  const api=vm.runInNewContext(`${outputText}\n({${names.join(",")}})`,context);
  return {review:api.prepareTaskReview,api,context,calls};
}

function task(id=7) {
  return {id,state:"failed",spec:{kind:"extract",path:"/Archives/outer.zip › inner.zip"}};
}

test("review resolves the authoritative source without replacing the displayed task",async()=>{
  const run=harness();const displayed=task();
  await run.review(displayed);
  assert.deepEqual(run.calls[0],["resolve",7]);
  assert.equal(run.calls[1][1].path,"squallz-archive://7");
  assert.equal(run.calls[1][2],displayed.spec);
  assert.equal(displayed.spec.path,"/Archives/outer.zip › inner.zip");
});

test("an unavailable or mismatched source never falls back to the displayed filesystem path",async()=>{
  for(const resolve of [async()=>{throw new Error("expired source");},async()=>({kind:"test",path:"/other.zip"})]) {
    const run=harness();run.context.ipc.reviewJobSpec=resolve;
    await run.review(task());
    assert.equal(run.calls.some(([name])=>name==="restore"),false);
    assert.match(run.calls.at(-1)[1],/Reopen the original archive/);
  }
});

test("a later review, navigation or archive change invalidates a pending source lookup",async()=>{
  for(const action of ["review","navigation","archive","close","back","details"]) {
    const run=harness();let finish;
    run.context.ipc.reviewJobSpec=(id)=>id===7?new Promise(resolve=>{finish=resolve;}):Promise.resolve({kind:"extract",path:"/next.zip"});
    const pending=run.review(task());
    if(action==="review")await run.review(task(8));
    if(action==="navigation")run.context.screen="create";
    if(action==="archive")run.context.currentArchive={id:2};
    if(action==="close")run.api.closeTaskCenter();
    if(action==="back")run.api.returnToTaskCenter(task());
    if(action==="details")run.api.openTaskCenterDetails(task(8));
    finish({kind:"extract",path:"squallz-archive://7"});await pending;
    const restores=run.calls.filter(([name])=>name==="restore");
    assert.equal(restores.length,action==="review"?1:0);
    if(restores.length)assert.equal(restores[0][1].path,"/next.zip");
  }
});

test("recovery uses the current inner archive without exposing its handle or selecting a displayed path as a file",()=>{
  const run=harness();const displayed=task();const actual={...displayed,spec:{...displayed.spec,path:"squallz-archive://7",best_effort:true}};
  run.context.currentArchive={id:7,source:actual.spec.path,path:displayed.spec.path};
  assert.equal(run.api.adoptRecoveryTargetFromTask(actual,displayed.spec),true);
  assert.equal(run.context.recoverySourceMode,"current");
  assert.equal(run.context.recoverySourceOverride,null);
  assert.equal(run.api.testTaskUsesRecoveryContext({...actual,spec:{...actual.spec,kind:"test"}}),false);
  run.context.currentArchive={id:8,source:"/Archives/other.zip",path:"/Archives/other.zip"};
  run.context.recoverySourceMode="selected";
  run.context.recoverySourceOverride="/Archives/kept.zip";
  assert.equal(run.api.adoptRecoveryTargetFromTask(actual,displayed.spec),false);
  assert.equal(run.context.recoverySourceOverride,"/Archives/kept.zip");
  assert.match(run.calls.at(-1)[1],/Reopen the original archive/);
});
