import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";
import { jobResult, resultKinds } from "../../tests/job-results.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { parseJobResult } = await server.ssrLoadModule("/src/lib/job-result.ts");

test("all job kinds decode one complete result and preserve zero counts and null metrics", () => {
  assert.equal(resultKinds.length, 17);
  for (const kind of resultKinds) {
    const value = jobResult(kind);
    assert.deepEqual(parseJobResult(value, kind, "done"), value, kind);
    assert.equal(parseJobResult(null, kind, "running"), null);
    assert.throws(() => parseJobResult(null, kind, "done"));
    assert.throws(() => parseJobResult(value, kind === "test" ? "compress" : "test", "done"));
  }
});

test("malformed required fields never turn into zero, empty rows or successful reports", () => {
  const fields = [
    ["test", "ok", "true"], ["test", "entries_tested", "4"],
    ["test", "problems", [null]], ["test", "problems_total", -1],
    ["checksum", "files_hashed", NaN], ["checksum", "bytes_hashed", Infinity],
    ["checksum", "items", Array(1)], ["checksum", "items", [{ path: "file", digest: "abc" }]],
    ["compress", "volume_count", 1.5], ["compress", "outputs", [42]],
    ["compress", "source_cleanup", {}], ["extract", "counts", {}],
    ["extract", "structure", "unknown"], ["verify_recovery", "metrics", {}],
    ["verify_recovery", "operation", "repair"], ["duplicate_scan", "groups", [null]],
  ];
  for (const [kind, key, invalid] of fields) {
    const value = jobResult(kind);
    value[key] = invalid;
    assert.throws(() => parseJobResult(value, kind, "done"), `${kind}.${key}`);
    delete value[key];
    assert.throws(() => parseJobResult(value, kind, "done"), `missing ${kind}.${key}`);
  }
  assert.throws(() => parseJobResult(jobResult("test", { problems: ["bad file"], problems_total: 0 }), "test", "done"));
  assert.throws(() => parseJobResult(jobResult("batch_extract", { extracted: 1 }), "batch_extract", "done"));
});

test("partial extraction and recovery diagnostics retain their exact counts and errors", () => {
  const value = jobResult("batch_extract", {
    archives: 2, selected_archives: 3, collapsed_volumes: 1, extracted: 1, failed: 1, recovered_archives: 1,
    outputs: [{ archive: "damaged.zip", ...jobResult("extract", { structure: "zip_local_headers_recovered", problems: ["bad.txt"], counts: { failed: 2, skipped: 1 } }) }],
    failures: [{ archive: "missing.7z", error: { key: "error.io", params: { detail: "missing" }, detail: "missing" } }],
  });
  const result = parseJobResult(value, "batch_extract", "done");
  assert.equal(result.outputs[0].counts.failed, 2);
  assert.equal(result.outputs[0].counts.skipped, 1);
  assert.equal(result.failures[0].error.detail, "missing");
  assert.equal(result.recovered_archives, 1);
  const repair = jobResult("repair_recovery", { ok: false, metrics: { blocks_needed: 5, recovery_blocks_available: 3, repair_possible: false } });
  assert.deepEqual(parseJobResult(repair, "repair_recovery", "done"), repair);
});
