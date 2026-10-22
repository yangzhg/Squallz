/** Decode native job reports once, before updating the task store. */
type Reader<T> = (value: unknown) => T;
type ReadValue<R> = R extends Reader<infer T> ? T : never;
type Shape = Record<string, Reader<unknown>>;
type ReadShape<S extends Shape> = { [K in keyof S]: ReadValue<S[K]> };

function invalid(): never { throw new Error("Invalid job result"); }
const string: Reader<string> = (v) => typeof v === "string" ? v : invalid();
const bool: Reader<boolean> = (v) => typeof v === "boolean" ? v : invalid();
const count: Reader<number> = (v) => typeof v === "number" && Number.isFinite(v) && Number.isInteger(v) && v >= 0 ? v : invalid();
const integer: Reader<number> = (v) => typeof v === "number" && Number.isInteger(v) ? v : invalid();
const nullable = <T>(read: Reader<T>): Reader<T | null> => (v) => v === null ? null : read(v);
const optional = <T>(read: Reader<T>): Reader<T | undefined> => (v) => v === undefined ? undefined : read(v);
const array = <T>(read: Reader<T>): Reader<T[]> => (v) => Array.isArray(v) ? Array.from(v, read) : invalid();
const choice = <const T extends readonly string[]>(...values: T): Reader<T[number]> => (v) => typeof v === "string" && values.includes(v) ? v : invalid();
function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : invalid();
}
function object<S extends Shape>(shape: S): Reader<ReadShape<S>> {
  return (value) => {
    const source = record(value);
    // Every property is constructed by its reader; the assertion only restores mapped keys.
    return Object.fromEntries(Object.entries(shape).map(([key, read]) => [key, read(source[key])])) as ReadShape<S>;
  };
}
const strings = array(string);
const structure = choice("complete", "zip_local_headers_recovered");
const algorithm = choice("blake3", "md5", "sha1", "sha224", "sha256", "sha384", "sha512", "crc32");
const output = { primary_output: string, outputs: strings, preserved_outputs: strings, total_bytes: count, split: bool, volume_count: count };
const cleanup = object({ status: choice("not_requested", "completed", "blocked", "partial", "failed", "cancelled"), moved: count, kept: count, recovery_required: count });
const create = { ...output, source_cleanup: cleanup, entries_tested_after_create: nullable(count), sfx: nullable(object({ target: string, layout: string, payload_bytes: count, requires_signing: bool })) };
const extractCounts = object({ destination: string, selected_entries: count, created: count, directories: count, skipped: count, replaced: count, renamed: count, failed: count, output_bytes: count });
const plan = object({ requested_destination: string, destination: string, layout: choice("direct", "wrap_in_folder"), entries: count, files: count, directories: count, symlinks: count, hardlinks: count, other: count, total_bytes: count, estimated_conflicts: count });
const problems = { problems: strings, problems_total: count, problems_truncated: bool };
const extract = { dest: string, best_effort: bool, ...problems, plan, counts: extractCounts, structure };
const recoverySummary = object({ scheme: string, block_size: count, total_blocks: count, data_shards: count, parity_shards: count, recovery_blocks_available: count, damaged_blocks: count, repaired_blocks: count, unrepaired_blocks: count, repair_possible: bool });
const metrics = object({ all_correct: bool, repair_possible: bool, blocks_needed: count, recovery_blocks_available: count, blocks_repaired: nullable(count), files_repaired: nullable(count), no_damage: bool });
const recovery = { ok: bool, operation: choice("protect", "verify", "repair"), archive: string, recovery: string, outputs: strings, output: nullable(string), tool: string, redundancy_percent: nullable(count), source_file_count: count, status_code: nullable(integer), metrics: nullable(metrics), stdout: string, stderr: string };
const error = object({ key: string, params: (v: unknown) => Object.fromEntries(Object.entries(record(v)).map(([k, v]) => [k, string(v)])), detail: string });
const checksumItem = object({ path: string, size: count, digest: string });
const checkItem = object({ path: string, expected: string, actual: nullable(string), ok: bool, error: nullable(string) });
const duplicateGroup = object({ hash: string, hash_algorithm: choice("blake3"), size: count, count, reclaimable_bytes: count, paths: strings });

const readers = {
  compress: object({ kind: choice("compress"), ...create }),
  convert: object({ kind: choice("convert"), ...create }),
  publish_macos_sfx: object({ kind: choice("publish_macos_sfx"), source: string, primary_output: string, outputs: strings, target: string, layout: string, payload_bytes: count, total_bytes: count, team_id: string, submission_id: string }),
  extract: object({ kind: choice("extract"), ...extract }),
  extract_nested: object({ kind: choice("extract_nested"), ...extract }),
  batch_extract: object({ kind: choice("batch_extract"), archives: count, selected_archives: count, collapsed_volumes: count, extracted: count, failed: count, recovered_archives: count, outputs: array(object({ archive: string, ...extract })), failures: array(object({ archive: string, error })) }),
  test: object({ kind: choice("test"), ok: bool, entries_tested: count, ...problems, recovery: nullable(recoverySummary), structure: optional(structure) }),
  export_sqz: object({ kind: choice("export_sqz"), ...output }),
  repair_sqz: object({ kind: choice("repair_sqz"), dest: string, in_place: bool, recovery: nullable(recoverySummary) }),
  repair_zip: object({ kind: choice("repair_zip"), dest: string, in_place: bool, source_entries: count }),
  protect: object({ kind: choice("protect"), ...recovery, operation: choice("protect") }),
  verify_recovery: object({ kind: choice("verify_recovery"), ...recovery, operation: choice("verify") }),
  repair_recovery: object({ kind: choice("repair_recovery"), ...recovery, operation: choice("repair") }),
  update: object({ kind: choice("update"), archive: string, operations: count }),
  checksum: object({ kind: choice("checksum"), algorithm, input_count: count, entries_scanned: count, files_hashed: count, bytes_hashed: count, items: array(checksumItem) }),
  checksum_check: object({ kind: choice("checksum_check"), ok: bool, algorithm, manifest: string, checked: count, passed: count, failed: count, bytes_hashed: count, items: array(checkItem) }),
  duplicate_scan: object({ kind: choice("duplicate_scan"), hash_algorithm: choice("blake3"), input_count: count, entries_scanned: count, files_scanned: count, bytes_scanned: count, min_size: count, candidate_files: count, hashed_bytes: count, duplicate_groups: count, duplicate_files: count, reclaimable_bytes: count, groups: array(duplicateGroup) }),
};

export type JobResult = ReadValue<(typeof readers)[keyof typeof readers]>;
export type ResultOf<K extends JobResult["kind"]> = Extract<JobResult, { kind: K }>;
export type ExtractResultCounts = ReadValue<typeof extractCounts>;
export type ExtractReport = ReadShape<typeof extract>;
export type BatchExtractOutput = ResultOf<"batch_extract">["outputs"][number];
export type ChecksumItem = ReadValue<typeof checksumItem> | ReadValue<typeof checkItem>;
export type DuplicateGroup = ReadValue<typeof duplicateGroup>;
export type RecoveryMetrics = ReadValue<typeof metrics>;

export function resultOf<K extends JobResult["kind"]>(result: JobResult | null | undefined, kind: K): ResultOf<K> | null {
  return result?.kind === kind ? result as ResultOf<K> : null;
}

export function parseJobResult(value: unknown, kind: JobResult["kind"], state: string): JobResult | null {
  if (value === null && state !== "done") return null;
  if (record(value).kind !== kind) return invalid();
  const result = readers[kind](value);
  if ("problems" in result && result.problems_total < result.problems.length) return invalid();
  if (result.kind === "batch_extract" && (result.extracted !== result.outputs.length || result.failed !== result.failures.length || result.archives !== result.extracted + result.failed)) return invalid();
  return result;
}
