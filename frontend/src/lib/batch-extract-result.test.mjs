import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { createTestServer } from "../../tests/runtime.mjs";

test("batch completion feedback keeps archive and entry failures distinct in both languages", async () => {
  const server = await createTestServer();
  try {
    const results = await server.ssrLoadModule("/src/lib/batch-extract-result.ts");
    const extraction = await server.ssrLoadModule("/src/lib/extract-result.ts");
    const i18n = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const code = readFileSync(new URL("./jobs.svelte.ts", import.meta.url), "utf8");
    const source = ts.createSourceFile("jobs.ts", code, ts.ScriptTarget.Latest, true);
    const names = ["terminalHistoryStatus", "taskHistoryDetail", "cleanHistoryDetail", "finishToast", "isRecoveryDiagnosticTask"];
    const declarations = source.statements.filter((node) => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
    const { outputText } = ts.transpileModule(declarations.map((node) => node.getText(source)).join("\n"), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    });
    const toasts = [];
    const run = vm.runInNewContext(`${outputText}\n({${names.join(",")}})`, {
      ...results, ...extraction, ...i18n, revealAfterExtract: false,
      revealPath() {}, pushToast: (toast) => toasts.push(toast),
    });
    for (const [locale, entryText] of [["en-US", "2 entries failed · 1 entries skipped"], ["zh-CN", "2 个条目失败 · 1 个条目跳过"]]) {
      await i18n.loadLocale(locale);
      const task = { state: "done", error: null, spec: { kind: "batch_extract", items: [{ dest: "/output" }] },
        result: { archives: 3, selected_archives: 4, extracted: 2, failed: 1,
          failures: [{ archive: "unreadable.zip" }], outputs: [
            { archive: "damaged.zip", dest: "/output/damaged", counts: { failed: 2, skipped: 1 } },
            { archive: "complete.zip", counts: { failed: 0, skipped: 0 } },
          ] } };
      const outcome = results.readBatchExtractResult(task.result);
      assert.equal(outcome.reviewCount, 2);
      assert.equal(outcome.completeArchives, 1);
      assert.equal(outcome.incompleteArchives, 1);
      assert.equal(outcome.failedEntries, 2);
      assert.equal(outcome.skippedEntries, 1);
      assert.equal(run.terminalHistoryStatus(task), "failed");
      const summary = results.batchExtractResultSummary(task.result, 3);
      assert.ok(summary.includes(entryText));
      assert.equal(run.taskHistoryDetail(task, "failed"), summary);
      run.finishToast(task);
      assert.equal(toasts.at(-1).kind, "warning");
      assert.equal(toasts.at(-1).title, summary);
      assert.equal(task.revealPath, "/output/damaged");

      task.result.failed = 0;
      task.result.failures = [];
      assert.equal(run.terminalHistoryStatus(task), "info");
      run.finishToast(task);
      assert.equal(toasts.at(-1).kind, "warning");
      task.result.outputs[0].counts.failed = 0;
      assert.equal(results.readBatchExtractResult(task.result).reviewCount, 0);
      assert.equal(run.terminalHistoryStatus(task), "info");
      run.finishToast(task);
      assert.equal(toasts.at(-1).kind, "warning");
      task.result.outputs[0].counts.skipped = 0;
      assert.equal(run.terminalHistoryStatus(task), "done");
      run.finishToast(task);
      assert.equal(toasts.at(-1).kind, "success");
    }
  } finally {
    await server.close();
  }
});
