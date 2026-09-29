import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskDuplicateGroups, taskHasInlineResults, taskResultScreen, taskOutcomeStateLabel } = await server.ssrLoadModule("/src/lib/task-model.ts");

function harness() {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const source = ts.createSourceFile("App.ts", component.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1], ts.ScriptTarget.Latest, true);
  const names = ["viewTaskResults", "selectedDuplicateScanTask", "duplicateResultNumber", "duplicateResultLabel", "submitDuplicateScanJob", "duplicatesWorkspaceSurface"];
  const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
  const context = {
    jobRows: [], duplicateReportTaskId: null, taskWindowMode: false,
    duplicateMinSize: 1048576, duplicateMinSizeError: "", duplicateExcludeText: "cache",
    duplicateScanTarget: () => "/new-scan", duplicateScanTargetName: () => "new-scan", duplicateScanTargetLabel: () => "/new-scan",
    duplicateExcludeRules: () => ["cache"], focusBlockingTaskIfAny: () => false,
    taskDuplicateGroups, taskHasInlineResults, taskResultScreen, taskOutcomeStateLabel,
    setScreen() {}, dismissTaskDialog() {}, focusDuplicateReportPanel() {},
    registerDuplicateReportPanel() {}, showNotice() {}, recordOperation() {},
    toolsArchiveReturnSurface: () => ({visible:false}), updateDuplicateMinSizeFromInput() {},
    chooseDuplicateScanFolder() {}, useCurrentArchiveFolderForDuplicates() {},
    taskStateLabel: (state) => state ?? "Pending", formatBytes: (value) => `${value} B`,
    pathBaseName: (path) => path.split("/").at(-1), tr: (_key, fallback) => fallback,
    submitJob: async (spec) => { context.jobRows.push({id:99,spec,state:"queued",result:null}); return 99; },
  };
  const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\n({${declarations.map((node) => node.name.text).join(",")},context:globalThis})`,context);
}

function task(id) {
  return {id,state:"done",spec:{kind:"duplicate_scan",inputs:[`/scan-${id}`],excludes:[`exclude-${id}`],min_size:id},
    result:{duplicate_groups:id,files_scanned:id*2,duplicate_files:id*2,reclaimable_bytes:id*10,groups:[]}};
}

test("duplicate report selection stays bound to its task until another report or scan is chosen", async () => {
  const run = harness();
  const first = task(1);
  run.context.jobRows.push(first,task(2));
  run.viewTaskResults(first);
  assert.equal(run.duplicateResultNumber("duplicate_groups"),1);
  run.context.jobRows.push(task(3));
  assert.equal(run.duplicateResultNumber("duplicate_groups"),1);
  run.viewTaskResults(run.context.jobRows[1]);
  assert.equal(run.duplicateResultNumber("duplicate_groups"),2);
  await run.submitDuplicateScanJob();
  assert.equal(run.duplicateResultNumber("duplicate_groups"),null);
  assert.equal(run.context.duplicateReportTaskId,99);
  run.context.jobRows.pop();
  assert.equal(run.duplicateResultNumber("duplicate_groups"),3);
  run.context.jobRows.length = 0;
  assert.equal(run.duplicateResultNumber("duplicate_groups"),null);
});

test("report context belongs to the scan and only a completed empty report means no duplicates", () => {
  const run = harness();
  const scan = task(7);
  run.context.jobRows.push(scan, task(8));
  run.viewTaskResults(scan);
  const surface = run.duplicatesWorkspaceSurface("modern");
  assert.deepEqual(Array.from(surface.report.context.sources), ["/scan-7"]);
  assert.deepEqual(Array.from(surface.report.context.excludes), ["exclude-7"]);
  assert.equal(surface.report.context.minimumSize, "7 B");
  assert.equal(surface.target.label, "/new-scan");
  assert.equal(surface.minimumSize.value, 1048576);
  for (const state of ["queued", "running", "paused", "failed", "cancelled"]) {
    scan.state = state;
    scan.result = null;
    assert.doesNotMatch(run.duplicatesWorkspaceSurface("classic").report.emptyMessage, /No duplicate files were found/);
    assert.ok(run.duplicatesWorkspaceSurface("classic").report.summary.every((item) => item.value === "—"));
  }
  scan.state = "done";
  scan.result = {};
  assert.match(run.duplicatesWorkspaceSurface("classic").report.emptyMessage, /No complete report/);
  scan.result = { duplicate_groups: 0, groups: [] };
  assert.match(run.duplicatesWorkspaceSurface("classic").report.emptyMessage, /No duplicate files were found/);
  run.context.jobRows.length = 0;
  assert.match(run.duplicatesWorkspaceSurface("modern").report.emptyMessage, /Choose a folder/);
});

test("both report layouts preserve long paths and hashes while bounding initial group and file rendering", async () => {
  const run = harness();
  const scan = task(25);
  scan.result.groups = Array.from({ length: 25 }, (_, index) => ({
    hash: String(index).padStart(64, "a"), size: 2048,
    paths: Array.from({ length: index === 0 ? 60 : 2 }, (_, copy) => `/scan-25/group-${index}/copy-${copy}/完整文件名与归档资料.txt`),
  }));
  run.context.jobRows.push(scan, task(99));
  run.viewTaskResults(scan);
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: ToolsWorkspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    run.context.tr = tFallback;
    for (const variant of ["modern", "classic"]) {
      const surface = run.duplicatesWorkspaceSurface(variant);
      assert.equal(surface.report.groups.length, 25);
      assert.equal(surface.report.groups[0].paths.length, 60);
      const { body } = render(ToolsWorkspace, { props: { surface } });
      assert.equal((body.match(/class="duplicate-report-group"/g) ?? []).length, 20);
      assert.ok(body.includes(scan.result.groups[0].paths[49]));
      assert.ok(body.includes(scan.result.groups[0].hash));
      assert.ok(body.includes(tFallback("gui.duplicates.next_groups")));
      assert.ok(body.includes(tFallback("gui.duplicates.show_more_paths").replace("{shown}", "50").replace("{total}", "60")));
      assert.doesNotMatch(body, /copy-50|scan-99|gui\.duplicates\./);
    }
  }
});
