import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

test("external launch is consumed before asynchronous work and remembers the submitted task", async () => {
  const server = await createTestServer();
  try {
    const { windowRecoveryUrl, readWindowRecovery } = await server.ssrLoadModule("/src/lib/window-recovery.ts");
    const helpers = await server.ssrLoadModule("/src/lib/task-window.ts");
    const source = readFileSync(new URL("../App.svelte", import.meta.url), "utf8").match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
    const ast = ts.createSourceFile("App.ts", source, ts.ScriptTarget.Latest, true);
    const functions = ast.statements.filter((node) => ts.isFunctionDeclaration(node)
      && ["submitExternalTaskWindow", "submitJob"].includes(node.name?.text));
    assert.equal(functions.length, 2);
    const { outputText } = ts.transpileModule(functions.map((node) => node.getText(ast)).join("\n").replaceAll("import.meta.env.DEV", "false"), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    });
    let url = "http://tauri.localhost/?taskWindow=1&externalTask=extract-here&externalPath=%2Ftmp%2Freports.zip";
    let resolveSpec;
    let resolveSubmission;
    let signalSubmission;
    const submissionStarted = new Promise((resolve) => { signalSubmission = resolve; });
    let submissions = 0;
    const context = vm.createContext({
      ...helpers, taskWindowMode: true, taskWindowLaunchState: null,
      taskDialogTaskId: 7, taskDialogDismissedId: null,
      jobSubmitInFlight: false, submittingJobSpec: null,
      checksumAlgorithm: "sha256", checksumExcludeRules: () => [],
      tr: (_key, fallback) => fallback, showNotice: () => {},
      focusBlockingTaskIfAny: () => null, isErrorDto: () => false,
      isJobSubmitBlocked: () => false,
      rememberWindowRecovery: (recovery) => { url = windowRecoveryUrl(url, recovery); },
      ipc: { resolveExternalTaskJob: () => new Promise((resolve) => { resolveSpec = resolve; }) },
      submitArchiveJob: () => {
        submissions += 1;
        signalSubmission();
        return new Promise((resolve) => { resolveSubmission = resolve; });
      },
    });
    vm.runInContext(outputText, context);
    const pending = context.submitExternalTaskWindow("extract-here", ["/tmp/reports.zip"], null);
    assert.equal(context.taskDialogTaskId, null);
    assert.equal(helpers.taskWindowLaunchStateFromParams(new URL(url).searchParams).launch, null);
    resolveSpec({ kind: "extract", path: "/tmp/reports.zip", dest: "/tmp/reports" });
    await submissionStarted;
    assert.equal(submissions, 1);
    assert.equal(helpers.taskWindowLaunchStateFromParams(new URL(url).searchParams).launch, null);
    resolveSubmission(42);
    await pending;
    assert.equal(readWindowRecovery(new URL(url).searchParams).taskId, 42);
    assert.equal(context.taskDialogTaskId, 42);
    assert.equal(submissions, 1);
    assert.equal(context.jobSubmitInFlight, false);
  } finally {
    await server.close();
  }
});

test("task windows recover owned completed tasks without switching an explicit task target", async () => {
  const server = await createTestServer();
  try {
    const { taskWindowTask } = await server.ssrLoadModule("/src/lib/task-window.ts");
    const source = readFileSync(new URL("../App.svelte", import.meta.url), "utf8").match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
    const ast = ts.createSourceFile("App.ts", source, ts.ScriptTarget.Latest, true);
    const declaration = ast.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "taskDialogTask");
    const { outputText } = ts.transpileModule(declaration.getText(ast), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    });
    const done = { id: 4, ownedByRequester: true, state: "done", result: { files_hashed: 2 } };
    const failed = { id: 5, ownedByRequester: true, state: "failed" };
    const other = { id: 6, ownedByRequester: false, state: "running" };
    assert.equal(taskWindowTask([done, other], null), done);
    assert.equal(taskWindowTask([failed, other, done], null), failed);
    assert.equal(taskWindowTask([done, failed, other], 4), done);
    assert.equal(taskWindowTask([done, failed, other], 6), null);
    assert.equal(taskWindowTask([done, failed, other], 9), null);
    assert.equal(taskWindowTask([], null), null);
    const context = vm.createContext({ taskWindowMode: true, taskWindowLaunchState: { status: "waiting" }, submittingTaskModel: () => null,
      taskDialogTaskId: null, jobRows: [done, other], blockingTask: () => null, taskWindowTask });
    vm.runInContext(outputText, context);
    assert.equal(context.taskDialogTask(), done, "completed results are restored in the actual window selector");
    for (const status of ["starting", "requires-desktop-service"]) {
      context.taskWindowLaunchState.status = status;
      assert.equal(context.taskDialogTask(), null, "a new launch never restores an unrelated older result");
    }
    context.taskWindowLaunchState.status = "waiting";
    context.taskDialogTaskId = 9;
    assert.equal(context.taskDialogTask(), null);
  } finally {
    await server.close();
  }
});

test("window recovery reconnects a task without replaying an external launch", async () => {
  const server = await createTestServer();
  try {
    const { readWindowRecovery, windowRecoveryUrl } = await server.ssrLoadModule("/src/lib/window-recovery.ts");
    const { taskWindowLaunchStateFromParams } = await server.ssrLoadModule("/src/lib/task-window.ts");
    const original = "http://tauri.localhost/?externalTask=extract-here&externalPath=%2Ftmp%2Fa.zip&externalPath=%2Ftmp%2Fb.zip&externalOutput=%2Ftmp%2Fout&theme=dark";
    const originalParams = new URL(original).searchParams;
    assert.ok(taskWindowLaunchStateFromParams(originalParams).launch);
    const recovered = new URL(windowRecoveryUrl(original, { taskId: 42 }));
    assert.deepEqual(readWindowRecovery(recovered.searchParams), { taskId: 42, taskCenter: false });
    const launch = taskWindowLaunchStateFromParams(recovered.searchParams);
    assert.equal(launch.mode, true);
    assert.equal(launch.launch, null);
    assert.equal(launch.pendingAction, null);
    for (const name of ["externalTask", "externalPath", "externalOutput"]) assert.equal(recovered.searchParams.has(name), false);
    assert.equal(recovered.searchParams.get("theme"), "dark");
    assert.equal(originalParams.getAll("externalPath").length, 2);
  } finally {
    await server.close();
  }
});

test("main-window recovery reopens the task center and clears stale task targets", async () => {
  const server = await createTestServer();
  try {
    const { readWindowRecovery, windowRecoveryUrl } = await server.ssrLoadModule("/src/lib/window-recovery.ts");
    const url = new URL(windowRecoveryUrl("tauri://localhost/?mode=classic&recoveryTask=19", { taskCenter: true }));
    assert.deepEqual(readWindowRecovery(url.searchParams), { taskId: null, taskCenter: true });
    assert.equal(url.searchParams.has("taskWindow"), false);
    assert.equal(url.searchParams.get("mode"), "classic");
    for (const taskId of [-1, 0, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      const invalid = new URL(windowRecoveryUrl(url.href, { taskId }));
      assert.deepEqual(readWindowRecovery(invalid.searchParams), { taskId: null, taskCenter: false });
    }
    for (const value of ["", "invalid", "-1", "1.2", "9007199254740992"]) {
      assert.equal(readWindowRecovery(new URLSearchParams({ recoveryTask: value })).taskId, null);
    }
  } finally {
    await server.close();
  }
});
