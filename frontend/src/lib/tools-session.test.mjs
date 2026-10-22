import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ToolsSession } = await server.ssrLoadModule("/src/lib/tools-session.svelte.ts");
const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
await loadLocale("en-US");

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function harness(overrides = {}) {
  const context = { archive: null, tasks: [], screen: "checksum", copyScope: "checksum:modern", visibleTaskId: null };
  const notices = [];
  const tools = new ToolsSession({
    read: () => context, choose: async () => null, submitJob: async () => 1,
    submissionBlocked: () => false, isSubmitBlocked: () => false,
    notice: (message) => notices.push(message), ...overrides,
  });
  return { context, notices, tools };
}

test("tool pickers cannot publish after leaving, newer input, another picker or disposal", async () => {
  for (const kind of ["file", "folder", "manifest", "duplicates"]) {
    for (const change of ["leave", "edit", "picker", "dispose"]) {
      const pending = deferred();
      let isCurrent;
      const run = harness({ choose: async (_purpose, _options, current) => {
        isCurrent = current;
        return pending.promise;
      } });
      const selection = run.tools.choose(kind);
      assert.equal(isCurrent(), true);
      const previousOwner = isCurrent;
      if (change === "edit") run.tools.edit({ checksumPath: "/newer" });
      else if (change === "picker") void run.tools.choose(kind);
      else run.tools[change]();
      assert.equal(previousOwner(), false, "an older picker must lose ownership");
      if (change === "picker") assert.equal(isCurrent(), true, "the newer picker retains ownership");
      pending.resolve("/obsolete");
      await selection;
      if (change !== "picker") {
        assert.ok(!Object.values(run.tools.draft).includes("/obsolete"));
        assert.deepEqual(run.notices, []);
      } else {
        assert.equal(run.notices.length, 1, "only the newer picker publishes feedback");
      }
      run.tools.dispose();
    }
  }
});

test("tool picker cancellation and failure preserve inputs and explain the outcome", async () => {
  for (const result of [null, [], new Error("unavailable")]) {
    const run = harness({ choose: async () => {
      if (result instanceof Error) throw result;
      return result;
    } });
    run.tools.edit({ checksumPath: "/kept", manifestPath: "/kept.manifest", duplicatePath: "/kept-folder" });
    const before = { ...run.tools.draft };
    for (const kind of ["file", "folder", "manifest", "duplicates"]) await run.tools.choose(kind);
    assert.deepEqual({ ...run.tools.draft }, before);
    assert.equal(run.notices.length, 4);
    assert.ok(run.notices.every((message) => result instanceof Error ? /dialog/.test(message) : /unchanged/.test(message)));
    run.tools.dispose();
  }
});

test("an older clipboard completion cannot replace or shorten the newer feedback timer", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const writes = [];
  const run = harness({ copy: async (_text, isCurrent) => {
    const pending = deferred();
    writes.push({ ...pending, isCurrent });
    return pending.promise;
  } });
  t.after(() => run.tools.dispose());
  const task = { id: 1, state: "done", spec: { kind: "checksum", inputs: ["/input"], algorithm: "sha256", excludes: [] },
    result: jobResult("checksum", { items: [{ path: "data", digest: "abc" }] }) };
  run.context.tasks.push(task);
  const old = run.tools.copyResults("checksum");
  const newer = run.tools.copyResults("checksum");
  writes[1].resolve("copied");
  await newer;
  const feedback = run.tools.feedback;
  t.mock.timers.tick(1300);
  writes[0].resolve("failed");
  await old;
  assert.equal(writes[0].isCurrent(), false);
  assert.equal(run.tools.feedback, feedback);
  assert.equal(run.notices.length, 1);
  t.mock.timers.tick(1299);
  assert.equal(run.tools.feedback, feedback);
  t.mock.timers.tick(1);
  assert.equal(run.tools.feedback, null);
});

test("automatic context changes permanently retire a pending copy before returning", async () => {
  for (const change of ["question", "report", "mode"]) {
    const pending = deferred();
    let owner;
    const run = harness({ copy: async (_text, isCurrent) => { owner = isCurrent; return pending.promise; } });
    const task = { id: 1, state: "done", spec: { kind: "checksum", inputs: ["/input"], algorithm: "sha256", excludes: [] },
      result: jobResult("checksum", { items: [{ path: "data", digest: "abc" }] }) };
    run.context.tasks.push(task);
    const copying = run.tools.copyResults("checksum");
    assert.equal(owner(), true);
    if (change === "report") run.context.tasks.length = 0;
    else run.context.copyScope = change;
    run.tools.syncContext();
    if (change === "report") run.context.tasks.push(task);
    else run.context.copyScope = "checksum:modern";
    assert.equal(owner(), false);
    pending.resolve("copied");
    await copying;
    assert.deepEqual(run.notices, []);
    assert.equal(run.tools.feedback, null);
    run.tools.dispose();
  }
});

test("tool submissions capture their input once and discarded sessions publish no late feedback", async () => {
  for (const kind of ["checksum", "checksum_check", "duplicate_scan"]) {
    const pending = deferred();
    const submitted = [];
    const run = harness({ submitJob: (spec) => { submitted.push(spec); return pending.promise; } });
    run.tools.edit({ checksumPath: "/original", manifestPath: "/original.manifest", duplicatePath: "/original-folder",
      algorithm: "sha512", checksumExcludes: "cache", duplicateExcludes: "temp", minimumSize: 123 });
    const submission = run.tools.submit(kind);
    run.tools.edit({ checksumPath: "/newer", manifestPath: "/newer.manifest", duplicatePath: "/newer-folder",
      algorithm: "md5", minimumSize: 999 });
    assert.equal(submitted.length, 1);
    if (kind === "checksum_check") assert.equal(submitted[0].manifest, "/original.manifest");
    else assert.deepEqual(Array.from(submitted[0].inputs), [kind === "checksum" ? "/original" : "/original-folder"]);
    if (kind === "duplicate_scan") assert.equal(submitted[0].min_size, 123);
    else assert.equal(submitted[0].algorithm, "sha512");
    run.tools.dispose();
    pending.resolve(20);
    await submission;
    assert.equal(run.tools.selectedReports[kind], undefined);
    assert.deepEqual(run.notices, []);
  }
});

test("verification reports expose late failures and every row through bounded pages", () => {
  for (const failures of [[24], [0, 24, 124], []]) {
    const run = harness();
    const items = Array.from({ length: 125 }, (_, index) => ({
      path: `/data/file-${index}.txt`, expected: "expected", actual: failures.includes(index) ? "different" : "expected",
      ok: !failures.includes(index), error: null,
    }));
    const task = { id: 1, state: "done", spec: { kind: "checksum_check", manifest: "/data/SHA256SUMS", algorithm: "sha256" },
      result: jobResult("checksum_check", { items, checked: 125, failed: failures.length, passed: 125 - failures.length }) };
    run.context.tasks.push(task);
    let report = run.tools.checksumReport("checksum_check");
    assert.equal(report.failuresOnly, failures.length > 0);
    if (failures.length) assert.deepEqual(Array.from(report.rows, (row) => row.name), failures.map((index) => items[index].path));
    report.onFilter(false);
    const visible = [];
    for (let page = 0; page < 3; page += 1) {
      run.tools.checksumReport("checksum_check").onPage(page);
      report = run.tools.checksumReport("checksum_check");
      assert.equal(report.page, page);
      assert.ok(report.rows.length <= 50);
      visible.push(...report.rows.map((row) => row.name));
    }
    assert.deepEqual(visible, items.map((item) => item.path));
    report.onFilter(true);
    report = run.tools.checksumReport("checksum_check");
    assert.equal(report.page, 0);
    assert.equal(report.filteredRows, failures.length);
    assert.equal(run.tools.checksumText("checksum_check").split("\n").length, 125, "copy always includes the complete report");
    run.context.tasks.push({ ...task, id: 2, result: jobResult("checksum_check", { items: [items[1]] }) });
    report.onPage(2);
    const current = run.tools.checksumReport("checksum_check");
    assert.equal(current.taskId, 2);
    assert.equal(current.page, 0);
    assert.equal(current.failuresOnly, false, "a new report starts with its own result state");
    run.tools.dispose();
  }
});

test("calculation report pages retain their algorithm and mark changed inputs", () => {
  const run = harness();
  const items = Array.from({ length: 55 }, (_, index) => ({ path: `/data/file-${index}`, size: index, digest: `digest-${index}` }));
  run.context.tasks.push({ id: 1, state: "done", spec: { kind: "checksum", inputs: ["/data"], algorithm: "sha256", excludes: [] },
    result: jobResult("checksum", { items, files_hashed: 55 }) });
  run.tools.edit({ checksumPath: "/data", checksumExcludes: "" });
  let report = run.tools.checksumReport("checksum");
  assert.equal(report.matchesInput, true);
  assert.equal(report.rows.length, 50);
  report.onPage(1);
  report = run.tools.checksumReport("checksum");
  assert.equal(report.rows[0].name, "/data/file-50");
  assert.equal(report.rows.length, 5);
  run.tools.edit({ algorithm: "blake3" });
  report = run.tools.checksumReport("checksum");
  assert.equal(report.matchesInput, false);
  assert.equal(report.context.algorithm, "SHA-256");
  run.tools.edit({ algorithm: "sha256", checksumExcludes: "cache" });
  assert.equal(run.tools.checksumReport("checksum").matchesInput, false);
  run.tools.edit({ checksumPath: "/other", checksumExcludes: "" });
  assert.equal(run.tools.checksumReport("checksum").matchesInput, false);
  run.tools.dispose();
});

test("duplicate size controls convert units without changing byte-based submissions", async () => {
  const specs = [];
  const run = harness({ submitJob: async (spec) => { specs.push(spec); return 1; } });
  run.tools.edit({ duplicatePath: "/data" });
  assert.equal(run.tools.minimumSizeUnit, "mib");
  assert.equal(run.tools.minimumSizeValue, 1);
  run.tools.updateMinimumSize({ currentTarget: { value: "1.5" } });
  assert.equal(run.tools.draft.minimumSize, 1572864);
  run.tools.changeMinimumSizeUnit("kib");
  assert.equal(run.tools.minimumSizeValue, 1536);
  await run.tools.submit("duplicate_scan");
  assert.equal(specs[0].min_size, 1572864);
  run.tools.changeMinimumSizeUnit("b");
  for (const invalid of ["", "-1", "0.5", "Infinity", "9007199254740992"]) {
    run.tools.updateMinimumSize({ currentTarget: { value: invalid } });
    assert.ok(run.tools.minimumSizeError);
    await run.tools.submit("duplicate_scan");
  }
  assert.equal(specs.length, 1, "invalid sizes cannot silently submit the previous value");
  run.tools.updateMinimumSize({ currentTarget: { value: "0" } });
  assert.equal(run.tools.minimumSizeError, "");
  await run.tools.submit("duplicate_scan");
  assert.equal(specs[1].min_size, 0);
  run.tools.dispose();
});
