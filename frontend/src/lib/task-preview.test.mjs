import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { createTestServer } from "../../tests/runtime.mjs";

const plain = (value) => JSON.parse(JSON.stringify(value));

async function withPreviews(body, production = false) {
  const server = production ? await createServer({
    root: fileURLToPath(new URL("../../", import.meta.url)), appType: "custom", logLevel: "silent",
    server: { hmr: false, middlewareMode: true },
    define: { "import.meta.env.DEV": false, "import.meta.env.PROD": true },
  }) : await createTestServer();
  try {
    const jobs = await server.ssrLoadModule("/src/lib/jobs.svelte.ts");
    const catalog = await server.ssrLoadModule("/src/lib/task-preview.ts");
    const runtime = await server.ssrLoadModule("/src/lib/dev-preview-data.ts");
    const history = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    const notices = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    const model = await server.ssrLoadModule("/src/lib/task-model.ts");
    const dialog = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const submissions = [];
    ipc.submitJob = async (spec) => { submissions.push(spec); return 1; };
    const install = (kind, state = "done") => {
      const id = (state === "done" ? jobs.installCompletedTaskPreview : jobs.installActiveTaskPreview)(kind);
      return jobs.tasks().find((task) => task.id === id);
    };
    await body({ jobs, catalog, runtime, model, dialog, install });
    assert.deepEqual(submissions, [], "preview installation never submits a native job");
    assert.deepEqual(history.operationHistory(), [], "preview records never enter operation history");
    assert.deepEqual(notices.toasts(), [], "preview installation does not publish notifications");
  } finally { await server.close(); }
}

test("the single preview catalog admits all scenarios with unique IDs and stable repeated jobs", () =>
  withPreviews(({ jobs, catalog, runtime, install }) => {
    assert.equal(catalog.taskPreviewKinds.length, 36);
    const ids = new Set();
    for (const kind of catalog.taskPreviewKinds) {
      assert.equal(catalog.parseTaskPreviewKind(kind), kind);
      const parsed = runtime.readRuntimePreviews(new URLSearchParams({ previewCompletedTask: kind, previewActiveTask: kind }), 500);
      assert.equal(parsed.completedTask, kind);
      assert.equal(parsed.activeTask, kind);
      for (const requested of ["done", "running"]) {
        const seed = catalog.createTaskPreview(kind, requested);
        const task = install(kind, requested);
        assert.equal(task.id, seed.id);
        assert.equal(ids.has(task.id), false, `${kind}/${requested} owns a distinct job`);
        ids.add(task.id);
        assert.deepEqual(plain(task.spec), seed.spec);
        assert.equal(task.state, seed.state);
        const review = jobs.previewTaskSpecForReview(task.id);
        assert.deepEqual(plain(review), seed.spec);
        const saved = plain(task);
        assert.equal(install(kind, requested), task);
        assert.equal(jobs.previewTaskSpecForReview(task.id), review);
        assert.deepEqual(plain(task), saved);
      }
    }
    assert.equal(jobs.tasks().length, 72);
    assert.equal(ids.size, 72);
    assert.equal(install("compress_failure").id, 940036);
    assert.equal(install("extract_metadata").id, 940021);
    for (const invalid of [null, "", "constructor", "toString", "__proto__", "COMPRESS", "compress ", "missing"]) {
      assert.equal(catalog.parseTaskPreviewKind(invalid), null);
      assert.equal(runtime.readRuntimePreviews(new URLSearchParams({ previewCompletedTask: invalid ?? "" }), 500).completedTask, null);
    }
    const isolated = catalog.createTaskPreview("batch_extract_partial", "done");
    isolated.spec.items[0].dest = "/changed/preview";
    isolated.result.failures[0].archive = "/changed/source";
    const fresh = catalog.createTaskPreview("batch_extract_partial", "done");
    assert.notEqual(fresh.spec.items[0].dest, isolated.spec.items[0].dest);
    assert.notEqual(fresh.result.failures[0].archive, isolated.result.failures[0].archive);
    assert.deepEqual(plain(install("batch_extract_partial").spec), fresh.spec);
  }));

test("preview questions, failure reviews and progress retain their workflow and protection data", () =>
  withPreviews(({ jobs, model, dialog, install }) => {
    const password = install("extract_password", "running");
    const conflict = install("extract_conflict", "running");
    assert.equal(jobs.pendingPassword(), password.question.prompt);
    assert.equal(jobs.pendingConflict(), conflict.question.prompt);
    assert.equal(password.question.prompt.id, password.id);
    assert.equal(conflict.question.prompt.id, conflict.id);
    assert.equal(password.question.prompt.version, 1);
    assert.equal(password.question.prompt.wrong, false);
    assert.equal(conflict.question.prompt.incoming_size, 5120000);
    for (const kind of ["extract_password", "extract_conflict"]) {
      const done = install(kind);
      assert.equal(done.question, null);
      assert.equal(done.result.operation, "create");
      assert.match(done.revealPath, /\/client-data$/u);
    }
    for (const [kind, screen, key, protection] of [
      ["compress_failure", "create", "error.io", true],
      ["compress_sfx_failure", "create", "error.sfx_recovery", false],
      ["convert_failure", "convert", "error.io", true],
      ["convert_encrypted_failure", "convert", "error.io", true],
      ["extract_failure", "extract", "error.io", false],
      ["batch_extract_failure", "batch", "error.io", false],
      ["extract_nested_failure", "nestedExtract", "error.io", false],
      ["update_failure", "updateReview", "error.io", false],
      ["duplicate_scan_failure", "duplicates", "error.io", false],
      ["recovery_cleanup_ready", "recovery", "error.recovery_cleanup_output_ready", false],
      ["recovery_cleanup_unconfirmed", "recovery", "error.recovery_cleanup_unconfirmed", false],
      ["recovery_cleanup_record", "recovery", "error.recovery_cleanup_record", false],
    ]) {
      for (const requested of ["done", "running"]) {
        const task = install(kind, requested);
        assert.equal(task.state, "failed");
        assert.equal(task.error.key, key);
        assert.equal(model.taskReviewScreen(task), screen);
        assert.equal(task.outputPasswordRequired, protection);
        assert.equal(task.result, null);
        assert.equal(task.revealPath, null);
      }
    }
    const compress = jobs.previewTaskSpecForReview(install("compress_failure").id);
    assert.equal(compress.password, null);
    assert.equal(compress.replacement_guard, null);
    assert.equal(compress.split_size, 123456789);
    assert.equal(compress.split_mode, "native");
    for (const kind of ["convert_failure", "convert_encrypted_failure"]) {
      const review = jobs.previewTaskSpecForReview(install(kind).id);
      assert.equal(review.src_password, null);
      assert.equal(review.dest_password, null);
      assert.equal(review.replacement_guard, null);
      assert.equal(review.encrypt_names, kind === "convert_encrypted_failure");
    }
    const update = jobs.previewTaskSpecForReview(install("update_failure").id);
    assert.equal(update.password, null);
    assert.equal(update.expected_archive_id, null);
    assert.ok(update.rename[0].to.includes("交付文档/"));
    const partial = install("batch_extract_partial");
    assert.equal(model.taskReviewScreen(partial), "batch");
    assert.deepEqual(partial.result.failures.map((item) => item.archive), [partial.spec.items[0].path, partial.spec.items[2].path]);
    assert.ok(partial.spec.items.every((item) => item.password === null));
    assert.equal(partial.spec.items[0].best_effort, true);
    for (const kind of ["archive_open", "extract_metadata", "batch_extract_metadata"]) {
      const task = install(kind, "running");
      assert.equal(dialog.taskOverallProgressIndeterminate(task), true);
      assert.equal(dialog.taskProgressPercent(task), 0);
    }
    const batch = install("batch_extract", "running");
    assert.equal(batch.done, 1000);
    assert.equal(batch.total, 2000);
    assert.equal(dialog.taskProgressPercent(batch), 50);
    const scan = install("update_scan", "running");
    assert.equal(scan.scanEntries, 128);
    assert.equal(scan.total, 0);
    const commit = install("update_commit", "running");
    assert.equal(commit.phase, "update_commit");
    assert.equal(commit.interruptible, false);
  }));

test("preview report history, queue ownership and unavailable status preserve distinct task behavior", async () => {
  for (const kind of ["checksum", "checksum_check", "duplicate_scan"]) {
    await withPreviews(({ jobs }) => {
      const id = jobs.installCompletedTaskPreview(kind);
      const previous = jobs.tasks()[0];
      previous.speed = 17;
      jobs.setTaskExpanded(id, false);
      const previousSnapshot = plain(previous);
      assert.equal(jobs.installCompletedTaskPreview(kind, true), id);
      const tasks = jobs.tasks();
      assert.equal(tasks.length, 2);
      const extra = tasks[1];
      assert.deepEqual(plain(previous), previousSnapshot);
      if (kind === "duplicate_scan") {
        assert.equal(extra.result.groups.length, 0);
        assert.notEqual(jobs.previewTaskSpecForReview(extra.id), null);
      } else {
        assert.equal(extra.id, id + 2000);
        assert.equal(extra.spec.algorithm, "sha512");
        assert.equal(jobs.previewTaskSpecForReview(extra.id), null);
        assert.equal(extra.speed, 17, "report history inherits metadata outside its output fields");
        assert.equal(extra.expanded, false);
        assert.deepEqual([extra.done, extra.total, extra.current, extra.currentDone, extra.currentTotal],
          [64, 64, "release.txt", 64, 64]);
        if (kind === "checksum") assert.equal(extra.result.items[0].digest.length, 128);
        else assert.equal(extra.result.items[0].ok, false);
      }
      const saved = plain(tasks);
      assert.equal(jobs.installCompletedTaskPreview(kind, true), id);
      assert.equal(jobs.tasks()[1], extra);
      assert.deepEqual(plain(jobs.tasks()), saved);
    });
  }
  for (const reason of ["parallel_limit", "cpu_budget"]) {
    await withPreviews(({ jobs }) => {
      const id = jobs.installTaskQueuePreview(reason);
      const tasks = jobs.tasks();
      const running = tasks.find((task) => task.id === id);
      const waiting = tasks.filter((task) => task.state === "queued");
      assert.equal(tasks.length, 5);
      assert.equal(jobs.queuedCount(), 2);
      assert.equal(running.ownedByRequester, false);
      assert.equal(running.origin, "file_manager");
      assert.equal(running.interaction, "password");
      assert.equal(running.question, null);
      assert.equal(jobs.pendingPassword(), null);
      assert.deepEqual(waiting.map((task) => task.queuePosition), [1, 2]);
      assert.deepEqual(waiting.map((task) => task.queueWaitReason), [reason, "queue_order"]);
      assert.equal(waiting[0].cpuThreads, reason === "cpu_budget" ? 8 : 1);
      assert.equal(waiting[0].spec.kind, reason === "cpu_budget" ? "compress" : "extract");
      assert.ok(waiting.every((task) => task.historyRecorded === false && task.localEffects === false && task.snapshotSeen === false));
      assert.ok(waiting.every((task) => jobs.previewTaskSpecForReview(task.id) === null));
      const saved = plain(tasks);
      assert.equal(jobs.installTaskQueuePreview(reason), id);
      assert.equal(jobs.tasks().find((task) => task.id === id), running);
      assert.deepEqual(plain(jobs.tasks()), saved);
    });
  }
  await withPreviews(({ jobs }) => {
    const id = jobs.installActiveTaskPreview("compress", true);
    const task = jobs.tasks().find((item) => item.id === id);
    assert.equal(task.statusStale, true);
    assert.equal(task.speed, 0);
    assert.equal(jobs.installActiveTaskPreview("compress"), id);
    assert.equal(jobs.tasks()[0], task);
    assert.equal(task.statusStale, true);
  });
});

test("production SSR ignores every task preview query and keeps all preview installers inert", () =>
  withPreviews(({ jobs, catalog, runtime }) => {
    for (const kind of catalog.taskPreviewKinds) {
      const params = new URLSearchParams({ previewCompletedTask: kind, previewActiveTask: kind,
        previewTaskQueue: "cpu", previewChecksumHistory: "1", previewStatusUnavailable: "1" });
      const parsed = runtime.readRuntimePreviews(params, 500);
      assert.equal(parsed.completedTask, null);
      assert.equal(parsed.activeTask, null);
      assert.equal(parsed.taskQueue, null);
      assert.equal(jobs.installCompletedTaskPreview(kind, true), null);
      assert.equal(jobs.installActiveTaskPreview(kind, true), null);
      for (const state of ["done", "running"]) {
        assert.equal(jobs.previewTaskSpecForReview(catalog.createTaskPreview(kind, state).id), null);
      }
    }
    assert.equal(jobs.installTaskQueuePreview("parallel_limit"), null);
    assert.equal(jobs.installTaskQueuePreview("cpu_budget"), null);
    assert.deepEqual(jobs.tasks(), []);
  }, true));
