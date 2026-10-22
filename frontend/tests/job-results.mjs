/** Complete native reports for presentation tests; malformed payloads are tested at the decoder. */
const output = { primary_output: "/output/archive.zip", outputs: ["/output/archive.zip"], preserved_outputs: [], total_bytes: 64, split: false, volume_count: 1 };
const cleanup = { status: "not_requested", moved: 0, kept: 0, recovery_required: 0 };
const counts = { destination: "/output", selected_entries: 1, created: 1, directories: 0, skipped: 0, replaced: 0, renamed: 0, failed: 0, output_bytes: 64 };
const plan = { requested_destination: "/output", destination: "/output", layout: "direct", entries: 1, files: 1, directories: 0, symlinks: 0, hardlinks: 0, other: 0, total_bytes: 64, estimated_conflicts: 0 };
const problems = { problems: [], problems_total: 0, problems_truncated: false };
const extraction = { dest: "/output", best_effort: false, ...problems, plan, counts, structure: "complete" };
const recovery = { ok: true, archive: "/source.zip", recovery: "/source.zip.par2", outputs: [], output: null, tool: "rust-par2", redundancy_percent: null, source_file_count: 1, status_code: 0, metrics: null, stdout: "", stderr: "" };
export const recoveryMetrics = { all_correct: false, repair_possible: true, blocks_needed: 3, recovery_blocks_available: 4, blocks_repaired: null, files_repaired: null, no_damage: false };
const samples = {
  compress: { ...output, source_cleanup: cleanup, entries_tested_after_create: null, sfx: null },
  convert: { ...output, source_cleanup: cleanup, entries_tested_after_create: null, sfx: null },
  publish_macos_sfx: { source: "/Source.app", primary_output: "/Published.app", outputs: ["/Published.app"], target: "macos", layout: "macos_app", payload_bytes: 32, total_bytes: 64, team_id: "TEAM", submission_id: "submission" },
  extract: extraction, extract_nested: extraction,
  batch_extract: { archives: 0, selected_archives: 0, collapsed_volumes: 0, extracted: 0, failed: 0, recovered_archives: 0, outputs: [], failures: [] },
  test: { ok: true, entries_tested: 1, ...problems, recovery: null, structure: "complete" },
  export_sqz: output,
  repair_sqz: { dest: "/repaired.sqz", in_place: false, recovery: null },
  repair_zip: { dest: "/repaired.zip", in_place: false, source_entries: 1 },
  protect: { ...recovery, operation: "protect" },
  verify_recovery: { ...recovery, operation: "verify" },
  repair_recovery: { ...recovery, operation: "repair" },
  update: { archive: "/updated.zip", operations: 1 },
  checksum: { algorithm: "sha256", input_count: 1, entries_scanned: 1, files_hashed: 1, bytes_hashed: 64, items: [{ path: "/source.txt", size: 64, digest: "ab" }] },
  checksum_check: { ok: true, algorithm: "sha256", manifest: "/SHA256SUMS", checked: 1, passed: 1, failed: 0, bytes_hashed: 64, items: [{ path: "/source.txt", expected: "ab", actual: "ab", ok: true, error: null }] },
  duplicate_scan: { hash_algorithm: "blake3", input_count: 1, entries_scanned: 0, files_scanned: 0, bytes_scanned: 0, min_size: 0, candidate_files: 0, hashed_bytes: 0, duplicate_groups: 0, duplicate_files: 0, reclaimable_bytes: 0, groups: [] },
};
export const resultKinds = Object.keys(samples);

export function jobResult(kind, overrides = {}) {
  const result = structuredClone({ ...samples[kind], ...overrides, kind });
  for (const key of ["counts", "plan", "source_cleanup"]) {
    if (overrides[key]) result[key] = { ...samples[kind][key], ...overrides[key] };
  }
  if (overrides.metrics) result.metrics = { ...recoveryMetrics, ...overrides.metrics };
  if (overrides.problems && overrides.problems_total === undefined) result.problems_total = overrides.problems.length;
  if (kind === "checksum") result.items = result.items.map((item) => ({ size: 0, digest: "", ...item }));
  if (kind === "checksum_check") result.items = result.items.map((item) => ({ expected: "", actual: null, ok: true, error: null, ...item }));
  if (kind === "batch_extract") {
    result.outputs = result.outputs.map((item) => {
      const { kind: _kind, ...report } = jobResult("extract", item);
      return report;
    });
    result.failures = result.failures.map((item) => ({ error: { key: "error.io", params: {}, detail: "Unreadable archive" }, ...item }));
  }
  return result;
}
