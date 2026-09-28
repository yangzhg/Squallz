import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

async function loadOpenTaskOutput(dependencies) {
  const component = await readFile(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  assert.ok(script);
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const declaration = source.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === "openTaskOutput",
  );
  assert.ok(declaration);
  const { outputText } = ts.transpileModule(declaration.getText(source), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\nopenTaskOutput`, {
    ...dependencies,
    require: () => ({ openPath: async () => { throw new Error("path is not allowed"); } }),
  });
}

test("task output opening uses the window-scoped job command and preserves feedback", async () => {
  const calls = [];
  const notices = [];
  let unavailable = false;
  let archiveOpens = true;
  const openTaskOutput = await loadOpenTaskOutput({
    ipc: { openJobOutput: async (id) => {
      calls.push(["output", id]);
      if (unavailable) throw new Error("output unavailable");
    } },
    taskOutputPath: (task) => task.revealPath,
    taskOutputCanOpen: (task) => task.canOpen !== false,
    taskOutputIsFolder: (task) => task.spec.kind === "extract",
    openArchivePath: async (path, source) => {
      calls.push(["archive", path, source]);
      return archiveOpens;
    },
    dismissTaskDialog: async (task) => { calls.push(["dismiss", task.id]); },
    showNotice: (message) => { notices.push(message); },
    tr: (key) => key,
  });
  const task = { id: 42, state: "done", spec: { kind: "extract" }, revealPath: "/output" };

  await openTaskOutput(task);
  assert.deepEqual(calls, [["output", 42]]);
  assert.deepEqual(notices, ["gui.task.output_folder_opened"]);

  unavailable = true;
  await openTaskOutput(task);
  assert.equal(notices.at(-1), "gui.task.open_output_failed");
  unavailable = false;
  await openTaskOutput({ ...task, spec: { kind: "convert" } });
  assert.equal(notices.at(-1), "gui.task.output_opened");

  const before = calls.length;
  for (const override of [
    { id: null }, { state: "running" }, { state: "failed" },
    { state: "cancelled" }, { revealPath: null }, { canOpen: false },
  ]) {
    await openTaskOutput({ ...task, ...override });
  }
  assert.equal(calls.length, before);

  await openTaskOutput({ ...task, spec: { kind: "compress" } });
  assert.deepEqual(calls.slice(-2), [["archive", "/output", "open-file"], ["dismiss", 42]]);
  archiveOpens = false;
  await openTaskOutput({ ...task, spec: { kind: "compress" } });
  assert.deepEqual(calls.at(-1), ["archive", "/output", "open-file"]);
});
