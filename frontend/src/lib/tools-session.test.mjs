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
