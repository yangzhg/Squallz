import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { compileTestScript, readSvelteScriptAsync, selectFunctions } from "../../tests/source.mjs";

async function loadOpenTaskOutput(dependencies) {
  const source = await readSvelteScriptAsync(new URL("../App.svelte", import.meta.url), "App.ts");
  const [declaration] = selectFunctions(source, ["openTaskOutput"]);
  assert.ok(declaration);
  const outputText = compileTestScript(declaration.getText(source));
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
