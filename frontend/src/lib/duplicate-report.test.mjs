import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ToolsSession } = await server.ssrLoadModule("/src/lib/tools-session.svelte.ts");
const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
await loadLocale("en-US");

function harness() {
  const tasks = [];
  const tools = new ToolsSession({
    read: () => ({ tasks, archive: null, screen: "duplicates", copyScope: "duplicates:modern", visibleTaskId: null }),
    choose: async () => null, notice() {}, submissionBlocked: () => false, isSubmitBlocked: () => false,
    submitJob: async (spec) => { tasks.push({ id: 99, spec, state: "queued", result: null }); return 99; },
  }, { duplicatePath: "/new-scan", minimumSize: 1048576, duplicateExcludes: "cache" });
  return { tasks, tools, surface: (variant) => ({ kind: "duplicates", variant, session: tools,
    title: tFallback("gui.screen.duplicates", "Duplicate Finder"), tr: tFallback,
    archiveReturn: { visible: false }, onNavigate() {}, onPanelMount() {},
  }) };
}

function task(id) {
  return {id,state:"done",spec:{kind:"duplicate_scan",inputs:[`/scan-${id}`],excludes:[`exclude-${id}`],min_size:id},
    result:jobResult("duplicate_scan", {duplicate_groups:id,files_scanned:id*2,duplicate_files:id*2,reclaimable_bytes:id*10,groups:[]})};
}

test("duplicate report selection stays bound to its task until another report or scan is chosen", async () => {
  const run = harness();
  const first = task(1);
  run.tasks.push(first,task(2));
  run.tools.adoptTask(first);
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),1);
  run.tasks.push(task(3));
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),1);
  run.tools.adoptTask(run.tasks[1]);
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),2);
  await run.tools.submit("duplicate_scan");
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),null);
  assert.equal(run.tools.selectedReports.duplicate_scan,99);
  run.tasks.pop();
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),3);
  run.tasks.length = 0;
  assert.equal((run.tools.result("duplicate_scan")?.duplicate_groups ?? null),null);
});

test("report context belongs to the scan and only a completed empty report means no duplicates", () => {
  const run = harness();
  const scan = task(7);
  run.tasks.push(scan, task(8));
  run.tools.adoptTask(scan);
  const surface = run.surface("modern");
  assert.deepEqual(Array.from(surface.session.duplicateReport.context.sources), ["/scan-7"]);
  assert.deepEqual(Array.from(surface.session.duplicateReport.context.excludes), ["exclude-7"]);
  assert.equal(surface.session.duplicateReport.context.minimumSize, "7 B");
  assert.equal(surface.session.targetLabel("duplicate_scan"), "/new-scan");
  assert.equal(surface.session.draft.minimumSize, 1048576);
  for (const state of ["queued", "running", "paused", "failed", "cancelled"]) {
    scan.state = state;
    scan.result = null;
    assert.doesNotMatch(run.tools.duplicateReport.emptyMessage, /No duplicate files were found/);
    assert.ok(run.tools.duplicateReport.summary.every((item) => item.value === "—"));
  }
  scan.state = "done";
  scan.result = null;
  assert.match(run.tools.duplicateReport.emptyMessage, /No complete report/);
  scan.result = jobResult("duplicate_scan");
  assert.match(run.tools.duplicateReport.emptyMessage, /No duplicate files were found/);
  run.tasks.length = 0;
  assert.match(run.tools.duplicateReport.emptyMessage, /Choose a folder/);
});

test("both report layouts preserve long paths and hashes while bounding initial group and file rendering", async () => {
  const run = harness();
  const scan = task(25);
  scan.result.groups = Array.from({ length: 25 }, (_, index) => ({
    hash: String(index).padStart(64, "a"), size: 2048,
    paths: Array.from({ length: index === 0 ? 60 : 2 }, (_, copy) => `/scan-25/group-${index}/copy-${copy}/完整文件名与归档资料.txt`),
  }));
  run.tasks.push(scan, task(99));
  run.tools.adoptTask(scan);
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: ToolsWorkspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  for (const locale of ["en-US", "zh-CN"]) {
    await loadLocale(locale);
    for (const variant of ["modern", "classic"]) {
      const surface = run.surface(variant);
      assert.equal(surface.session.duplicateReport.groups.length, 25);
      assert.equal(surface.session.duplicateReport.groups[0].paths.length, 60);
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
