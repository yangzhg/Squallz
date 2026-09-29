import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

import { createTestServer } from "../../tests/runtime.mjs";

function reportHarness() {
  const component = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const script = component.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
  const source = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);
  const names = ["selectedChecksumTask", "checksumItems", "checksumResultText", "checksumResultNumber",
    "viewTaskResults", "checksumCopyFeedbackFor", "checksumCopyFeedbackToneFor",
    "submitChecksumJob", "submitChecksumCheckJob", "checksumWorkspaceSurface",
    "checksumAlgorithmLabel", "checksumAlgorithmHint", "checksumItemNumber"];
  const functions = names.map((name) => {
    const declaration = source.statements.find((node) =>
      ts.isFunctionDeclaration(node) && node.name?.text === name);
    assert.ok(declaration, name);
    return declaration.getText(source);
  }).join("\n");
  const context = {
    jobRows: [], checksumReportTaskIds: {}, taskWindowMode: false,
    checksumCopyFeedbackKind: "checksum", checksumCopyFeedbackTaskId: 1,
    checksumCopyFeedbackMessage: "Copied", checksumCopyFeedbackTone: "success",
    checksumManifestPath: "/new/SHA256SUMS", checksumAlgorithm: "sha256",
    checksumAlgorithms: ["sha256", "sha512"], checksumExcludeText: "",
    toolsArchiveReturnSurface: () => ({ visible: false }),
    checksumCurrentArchiveDisabledReason: () => "", checksumTargetName: () => "New selection",
    checksumTargetLabel: () => "/new/files", checksumManifestLabel: () => "/new/SHA256SUMS",
    checksumItemText: (item, key) => item[key] ?? "", checksumItemStatus: () => "OK",
    taskStateLabel: (state) => state ?? "Pending", formatBytes: (value) => `${value} B`,
    taskOutcomeNeedsAttention: (task) => task.result?.ok === false,
    selectChecksumAlgorithm() {}, copyChecksumResults() {}, chooseChecksumFile() {},
    chooseChecksumFolder() {}, useCurrentArchiveForChecksum() {}, chooseChecksumManifest() {},
    taskHasInlineResults: () => false, taskResultScreen: () => "checksum",
    setScreen() {}, dismissTaskDialog() {}, focusChecksumResultPanel() {},
    checksumResultLine: (_kind, item) => `${item.digest ?? item.actual}  ${item.path}`,
    checksumTarget: () => "/new/files", checksumExcludeRules: () => [],
    focusBlockingTaskIfAny: () => false, showNotice() {}, recordOperation() {},
    tr: (_key, fallback) => fallback, pathBaseName: (path) => path.split("/").at(-1),
    submitJob: async (spec) => {
      context.jobRows.push({ id: 20, spec, result: null, state: "queued" });
      return 20;
    },
  };
  const { outputText } = ts.transpileModule(functions, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  });
  return vm.runInNewContext(`${outputText}\n({${names.join(",")}, context:globalThis})`, context);
}

function checksumTask(id, kind = "checksum") {
  return { id, state: "done", expanded: true,
    spec: kind === "checksum"
      ? { kind, inputs: [`/run-${id}`], algorithm: "sha256", excludes: [] }
      : { kind, manifest: `/run-${id}/SHA256SUMS`, algorithm: "sha256" },
    result: { files_hashed: id, checked: id,
      items: [{ path: `/run-${id}/report.txt`, digest: `digest-${id}`, actual: `actual-${id}` }] },
  };
}

test("checksum report navigation and copying stay bound to the selected task", async () => {
  const harness = reportHarness();
  const { context } = harness;
  context.jobRows.push(checksumTask(1), checksumTask(2), checksumTask(3, "checksum_check"));
  harness.viewTaskResults(context.jobRows[0]);
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  assert.equal(harness.checksumResultNumber("checksum", "files_hashed"), 1);
  assert.equal(harness.checksumResultText("checksum"), "digest-1  /run-1/report.txt");
  context.jobRows.push(checksumTask(4));
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  harness.viewTaskResults(context.jobRows[2]);
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 3);
  assert.equal(harness.selectedChecksumTask("checksum").id, 1);
  assert.equal(harness.checksumCopyFeedbackFor("checksum"), "Copied");
  harness.viewTaskResults(context.jobRows[1]);
  assert.equal(harness.checksumCopyFeedbackFor("checksum"), null);
  assert.equal(harness.checksumCopyFeedbackToneFor("checksum"), null);
  await harness.submitChecksumJob();
  assert.equal(harness.selectedChecksumTask("checksum").id, 20);
  assert.equal(harness.checksumResultText("checksum"), "");
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 3);
});

test("removed reports and manifest submissions resolve to the appropriate task", async () => {
  const harness = reportHarness();
  harness.context.jobRows.push(checksumTask(1), checksumTask(2, "checksum_check"));
  harness.viewTaskResults(harness.context.jobRows[1]);
  await harness.submitChecksumCheckJob();
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 20);
  harness.context.jobRows.pop();
  assert.equal(harness.selectedChecksumTask("checksum_check").id, 2);
  harness.context.jobRows.pop();
  assert.equal(harness.selectedChecksumTask("checksum_check"), null);
});

test("both workspace styles show the selected manifest report and full result paths", async () => {
  const harness = reportHarness();
  const task = checksumTask(7, "checksum_check");
  task.spec.algorithm = "sha512";
  task.result.ok = false;
  task.result.items = Array.from({ length: 25 }, (_, i) => ({
    path: `/run-7/folder-${i}/report.txt`, expected: "ab".repeat(64), actual: "cd".repeat(64), ok: false,
  }));
  harness.context.jobRows.push(task, checksumTask(8, "checksum_check"));
  harness.viewTaskResults(task);
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: ToolsWorkspace } = await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
    const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const { taskOutcomeStateLabel } = await server.ssrLoadModule("/src/lib/task-model.ts");
    harness.context.taskOutcomeStateLabel = taskOutcomeStateLabel;
    for (const locale of ["en-US", "zh-CN"]) {
      await loadLocale(locale);
      harness.context.tr = tFallback;
      for (const variant of ["modern", "classic"]) {
        const surface = harness.checksumWorkspaceSurface(variant);
        assert.equal(surface.verification.context.algorithm, "SHA-512");
        assert.equal(surface.verification.context.sources[0], "/run-7/SHA256SUMS");
        assert.equal(surface.algorithm.selected, "sha256");
        assert.equal(surface.verification.rows.length, 20);
        assert.equal(surface.verification.totalRows, 25);
        assert.equal(harness.checksumResultText("checksum_check").split("\n").length, 25);
        const { body } = render(ToolsWorkspace, { props: { surface } });
        assert.ok(body.includes('/run-7/folder-0/report.txt'));
        assert.ok(body.includes('/run-7/folder-19/report.txt'));
        assert.ok(body.includes('ab'.repeat(64)) && body.includes('cd'.repeat(64)));
        assert.ok(body.includes(tFallback("gui.checksum.report_manifest")));
        assert.ok(body.includes(tFallback("gui.task.state.needs_attention")));
        assert.ok(body.includes(tFallback("gui.checksum.result_preview_rows")
          .replace("{shown}", "20").replace("{total}", "25")));
        assert.doesNotMatch(body, /run-8|gui\.checksum\./u);
      }
    }
  } finally {
    await server.close();
  }
});
