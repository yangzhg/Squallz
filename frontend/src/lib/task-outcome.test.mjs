import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

async function withOutcomes(run) {
  const server = await createTestServer();
  try {
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const outcome = await server.ssrLoadModule("/src/lib/task-outcome.ts");
    const dialog = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { createTaskPreview } = await server.ssrLoadModule("/src/lib/task-preview.ts");
    await loadLocale("en-US");
    const task = (kind, changes = {}) => ({ ...createTaskPreview(kind, "done"), ...changes });
    await run({ ...outcome, dialog, task, loadLocale });
  } finally { await server.close(); }
}

test("result severity is consistent for retained outputs, partial extraction and source cleanup", () =>
  withOutcomes(({ task, taskResultPresentation, dialog }) => {
    const created = task("compress");
    const cases = [
      [created, "done"],
      [task("batch_extract_partial"), "failed"],
      [task("recovery_verify_over_capacity"), "failed"],
      [{ ...created, spec: { ...created.spec, kind: "convert" },
        result: jobResult("convert", { ...created.result, preserved_outputs: ["/exports/previous.zip"] }) }, "info"],
      [{ ...created, spec: { ...created.spec, post_success: "trash_source" },
        result: { ...created.result, source_cleanup: { ...created.result.source_cleanup, status: "completed", moved: 1, kept: 1 } } }, "info"],
      [{ ...created, spec: { ...created.spec, post_success: "trash_source" },
        result: { ...created.result, source_cleanup: { ...created.result.source_cleanup, status: "completed", recovery_required: 1 } } }, "info"],
      [{ ...task("checksum_check"), result: jobResult("checksum_check", { ok: false, passed: 1, checked: 2, failed: 1 }) }, "failed"],
    ];
    for (const [item, status] of cases) {
      const result = taskResultPresentation(item);
      assert.equal(result.status, status, item.spec.kind);
      assert.equal(dialog.taskOutcomeNeedsAttention(item), status !== "done", item.spec.kind);
      assert.equal(result.toastKind, status === "done" ? "success" : "warning");
      assert.equal(result.summary, dialog.taskDialogResultSummary(item));
    }
  }));

test("output paths survive observer recovery without running local notification effects", () =>
  withOutcomes(({ task, taskResultPresentation, dialog }) => {
    for (const kind of ["compress", "compress_split", "compress_sfx", "extract", "batch_extract", "recovery_protect"]) {
      const original = task(kind);
      const expectedOutput = kind === "extract" ? original.result.dest : original.revealPath;
      const recovered = { ...original, revealPath: null, localEffects: false };
      const result = taskResultPresentation(recovered);
      assert.equal(result.output, expectedOutput, kind);
      assert.equal(dialog.taskOutputPath(recovered), expectedOutput, kind);
      assert.equal(dialog.taskOutputCanOpen(recovered), !["compress_split", "compress_sfx", "recovery_protect"].includes(kind));
    }
    const base = task("compress", { revealPath: null });
    for (const kind of ["export_sqz", "repair_sqz", "repair_zip", "extract_nested", "convert"]) {
      const item = { ...base, spec: { kind, dest: "/exports/copy.zip", split_size: null }, result: jobResult(kind, { dest: "/exports/copy.zip", primary_output: "/exports/copy.zip" }) };
      assert.equal(taskResultPresentation(item).output, "/exports/copy.zip", kind);
    }
    const repair = { ...base, spec: { kind: "repair_recovery", path: "/source.zip", output: "/repaired.zip" } };
    assert.equal(taskResultPresentation({ ...repair, result: jobResult("repair_recovery", { ok: false }) }).output, null);
    assert.equal(taskResultPresentation({ ...repair, result: jobResult("repair_recovery", { ok: true, output: "/repaired.zip" }) }).output, "/repaired.zip");
  }));

test("completion retains actionable backup, structural recovery, integrity and cleanup notices in both languages", () =>
  withOutcomes(async ({ task, taskResultPresentation, loadLocale }) => {
    for (const locale of ["en-US", "zh-CN"]) {
      await loadLocale(locale);
      const sfx = task("compress_sfx");
      sfx.result = { ...sfx.result, preserved_outputs: ["/exports/previous.app"] };
      const preserved = taskResultPresentation(sfx);
      assert.equal(preserved.toastKind, "warning");
      assert.ok(preserved.body);
      assert.doesNotMatch(preserved.body, /gui\./u);
      const extracted = task("extract");
      extracted.result = { ...extracted.result, structure: "zip_local_headers_recovered" };
      assert.equal(taskResultPresentation(extracted).toastKind, "warning");
      assert.ok(taskResultPresentation(extracted).body);
      const created = task("compress");
      created.spec.post_success = "trash_source";
      created.result = { ...created.result, tested_after_create: true, entries_tested_after_create: 7,
        source_cleanup: { status: "partial", moved: 2, kept: 1, recovery_required: 0 } };
      const completion = taskResultPresentation(created);
      assert.match(completion.historyDetail, /7/u);
      assert.match(completion.historyDetail, /2/u);
      assert.match(completion.historyDetail, /1/u);
      assert.doesNotMatch(completion.historyDetail, /gui\./u);
      const failed = taskResultPresentation({ ...created, state: "failed", result: null, revealPath: null,
        error: { key: "error.disk_full", params: {}, detail: "" } });
      assert.equal(failed.status, "failed");
      assert.equal(failed.historyDetail, failed.summary, "a failed archive did not run source cleanup");
      assert.equal(failed.body, undefined);
    }
  }));
