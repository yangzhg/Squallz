import { BATCH_PROGRESS_SCALE, type ErrorDto, type JobQuestion, type JobSpec, type ProgressPhase } from "./ipc";
import { basename } from "./format";

type PreviewState = "done" | "running";
type PreviewProgress = {
  done: number;
  total: number;
  current: string;
  currentDone: number;
  currentTotal: number;
  speed: number;
};
type PreviewData = {
  spec: JobSpec;
  progress: PreviewProgress;
  phase: ProgressPhase | null;
  error: ErrorDto | null;
  result: Record<string, unknown>;
  revealPath: string | null;
  question: JobQuestion | null;
  scanEntries: number | null;
  outputPasswordRequired: boolean;
};
export type TaskPreviewSeed = Omit<PreviewData, "result"> & {
  id: number;
  state: PreviewState | "failed";
  result: Record<string, unknown> | null;
  cpuThreads: number;
  streamBufferLimitBytes: number | null;
  interruptible: boolean;
};

const sampleRoot = "/Users/alex/Squallz Samples";
const sampleOutputRoot = "/Users/alex/Squallz Exports";
const archive = `${sampleRoot}/product-backup.zip`;
const archiveOutput = `${sampleOutputRoot}/product-backup.zip`;
const extractedOutput = `${sampleOutputRoot}/product-backup`;
const photos = `${sampleRoot}/photos`;
const photoDigest = "9bc1b2a288b3f53f0c448c9a6fe2c7e97e0d8bb74f7e7f548d3f1ad4020cc714";

// This catalog owns accepted names and stable identity together with each scenario's facts.
const catalog = {
  archive_open: { offset: 23, build: (state: PreviewState) => inspectionPreview("open", state) },
  compress: { offset: 1, build: (state: PreviewState) => compressionPreview("plain", state) },
  compress_failure: { offset: 36, build: (state: PreviewState) => compressionPreview("failure", state) },
  compress_split: { offset: 8, build: (state: PreviewState) => compressionPreview("split", state) },
  compress_sfx: { offset: 9, build: (state: PreviewState) => compressionPreview("sfx", state) },
  compress_sfx_failure: { offset: 10, build: (state: PreviewState) => compressionPreview("sfxFailure", state) },
  convert_failure: { offset: 25, build: (state: PreviewState) => conversionPreview(false, state) },
  convert_encrypted_failure: { offset: 26, build: (state: PreviewState) => conversionPreview(true, state) },
  duplicate_scan: { offset: 27, build: (state: PreviewState) => duplicatePreview("groups", state) },
  duplicate_scan_clean: { offset: 28, build: (state: PreviewState) => duplicatePreview("clean", state) },
  duplicate_scan_failure: { offset: 29, build: (state: PreviewState) => duplicatePreview("failure", state) },
  recovery_cleanup_ready: { offset: 18, build: (state: PreviewState) => cleanupPreview("ready", state) },
  recovery_cleanup_unconfirmed: { offset: 19, build: (state: PreviewState) => cleanupPreview("unconfirmed", state) },
  recovery_cleanup_record: { offset: 20, build: (state: PreviewState) => cleanupPreview("record", state) },
  extract: { offset: 2, build: (state: PreviewState) => extractionPreview("plain", state) },
  extract_failure: { offset: 24, build: (state: PreviewState) => extractionPreview("failure", state) },
  extract_password: { offset: 34, build: (state: PreviewState, id: number) => extractionPreview("password", state, id) },
  extract_conflict: { offset: 35, build: (state: PreviewState, id: number) => extractionPreview("conflict", state, id) },
  extract_nested_failure: { offset: 32, build: (state: PreviewState) => nestedPreview(state) },
  update_failure: { offset: 33, build: (state: PreviewState) => updatePreview("failure", state) },
  extract_unknown_current: { offset: 4, build: (state: PreviewState) => extractionPreview("unknownCurrent", state) },
  extract_metadata: { offset: 21, build: (state: PreviewState) => extractionPreview("metadata", state) },
  batch_extract_metadata: { offset: 22, build: (state: PreviewState) => batchPreview("metadata", state) },
  batch_extract: { offset: 3, build: (state: PreviewState) => batchPreview("plain", state) },
  batch_extract_partial: { offset: 30, build: (state: PreviewState) => batchPreview("partial", state) },
  batch_extract_failure: { offset: 31, build: (state: PreviewState) => batchPreview("failure", state) },
  test: { offset: 5, build: (state: PreviewState) => inspectionPreview("test", state) },
  checksum: { offset: 6, build: (state: PreviewState) => checksumPreview(false, state) },
  checksum_check: { offset: 7, build: (state: PreviewState) => checksumPreview(true, state) },
  recovery_protect: { offset: 17, build: (state: PreviewState) => protectionPreview(state) },
  recovery_verify_repairable: { offset: 14, build: (state: PreviewState) => recoveryPreview("repairable", state) },
  recovery_verify_multi_file_repairable: { offset: 16, build: (state: PreviewState) => recoveryPreview("multiFile", state) },
  recovery_verify_over_capacity: { offset: 15, build: (state: PreviewState) => recoveryPreview("overCapacity", state) },
  update_scan: { offset: 11, build: (state: PreviewState) => updatePreview("scan", state) },
  update_verify: { offset: 12, build: (state: PreviewState) => updatePreview("verify", state) },
  update_commit: { offset: 13, build: (state: PreviewState) => updatePreview("commit", state) },
};
export type PreviewTaskKind = keyof typeof catalog;
export const taskPreviewKinds: readonly PreviewTaskKind[] = /* @__PURE__ */ Object.freeze(Object.keys(catalog) as PreviewTaskKind[]);

export function parseTaskPreviewKind(value: string | null): PreviewTaskKind | null {
  return value !== null && Object.hasOwn(catalog, value) ? value as PreviewTaskKind : null;
}

export function createTaskPreview(kind: PreviewTaskKind, requested: PreviewState): TaskPreviewSeed {
  const scenario = catalog[kind];
  const id = 940_000 + (requested === "running" ? 100 : 0) + scenario.offset;
  const data = scenario.build(requested, id);
  const state = data.error ? "failed" : requested;
  return {
    ...data, id, state,
    result: state === "done" ? data.result : null,
    revealPath: state === "done" ? data.revealPath : null,
    cpuThreads: data.spec.kind === "compress" ? 8 : 1,
    streamBufferLimitBytes: data.spec.kind === "compress" ? 512 * 1024 * 1024 : null,
    interruptible: data.phase !== "update_commit",
  };
}

function emptyProgress(current = ""): PreviewProgress {
  return { done: 0, total: 0, current, currentDone: 0, currentTotal: 0, speed: 0 };
}

function byteProgress(
  state: PreviewState,
  {
    total = 48_000_000,
    current = "reports/Launch plan.pdf",
    speed = 21_000_000,
    currentTotal = 4_096_000,
    runningCurrent = 1_920_000,
  }: {
    total?: number;
    current?: string;
    speed?: number;
    currentTotal?: number;
    runningCurrent?: number;
  } = {},
): PreviewProgress {
  return {
    done: state === "done" ? total : Math.floor(total * 0.4),
    total,
    current,
    currentDone: state === "done" ? currentTotal : runningCurrent,
    currentTotal,
    speed: state === "running" ? speed : 0,
  };
}

function ioError(detail: string): ErrorDto {
  return { key: "error.io", params: { detail }, detail };
}

function createResult(output = archiveOutput, totalBytes = 24_000_000): Record<string, unknown> {
  return { operation: "create", primary_output: output, outputs: [output],
    total_bytes: totalBytes, volume_count: 1, split: false };
}

function compressionPreview(variant: "plain" | "failure" | "split" | "sfx" | "sfxFailure", state: PreviewState): PreviewData {
  const failure = variant === "failure";
  const split = variant === "split";
  const sfx = variant === "sfx" || variant === "sfxFailure";
  const output = sfx ? `${sampleOutputRoot}/Installer.app` : archiveOutput;
  const total = split ? 92_760_416 : variant === "plain" ? 24_000_000 : 48_000_000;
  const spec: JobSpec = {
    kind: "compress", inputs: [`${sampleRoot}/reports`, photos], dest: output,
    level: failure ? 4 : 5, password: null, encrypt_names: false,
    split_size: failure ? 123456789 : split ? 8 * 1024 * 1024 : null,
    split_mode: failure ? "native" : "generic", excludes: failure ? ["*.bak", "cache/**"] : [],
    content_policy: failure ? "custom" : "keep_all_files", sqz_inner_format: null,
    sfx_target: sfx ? "macos" : null, replace_existing: variant !== "plain", replacement_guard: null,
    completion: "none", post_success: "keep_source", test_after_create: failure,
  };
  let result = createResult(output);
  let revealPath = output;
  if (split) {
    const outputs = Array.from({ length: 12 }, (_, index) => `${output}.${String(index + 1).padStart(3, "0")}`);
    const preserved = outputs.slice(0, 3).map((path, index) => {
      const name = basename(path);
      return `${sampleOutputRoot}/.${name}.split-backup-940008-${index}.tmp.${name}`;
    });
    result = { operation: "create", primary_output: outputs[0], outputs, preserved_outputs: preserved,
      total_bytes: total, volume_count: outputs.length, split: true };
    revealPath = outputs[0];
  } else if (variant === "sfx") {
    result = { operation: "create_sfx", primary_output: output, outputs: [output],
      preserved_outputs: [`${sampleOutputRoot}/.squallz-sfx-holder-940009-1/previous`],
      total_bytes: total, volume_count: 1, split: false, requires_signing: true,
      sfx_target: "macos", layout: "macos_app" };
  }
  const journal = `${sampleOutputRoot}/.squallz-sfx-transaction.json`;
  const holder = `${sampleOutputRoot}/.squallz-sfx-holder-940010-3`;
  const error = failure ? ioError("Could not write the archive output")
    : variant === "sfxFailure" ? {
      key: "error.sfx_recovery", params: { target: output, journal, count: "4",
        paths: [journal, holder, `${holder}/previous`, `${holder}/replacement`].join("\n") },
      detail: `SFX replacement requires manual recovery. Inspect target ${output} and the listed transaction paths.`,
    } : null;
  return {
    spec,
    progress: byteProgress(state, {
      total,
      current: split ? "product-backup.zip.002" : "reports/Launch plan.pdf",
      speed: 12_800_000,
      currentTotal: split ? 0 : 3_800_000,
      runningCurrent: split ? 0 : 1_420_000,
    }),
    phase: split ? "output_split" : null, result, revealPath, error,
    question: null, scanEntries: null, outputPasswordRequired: failure,
  };
}

function conversionPreview(encrypted: boolean, state: PreviewState): PreviewData {
  return {
    spec: { kind: "convert", src: archive, dest: `${sampleOutputRoot}/Reviewed backup.${encrypted ? "7z" : "zip"}`,
      level: 4, src_encoding: null, src_password: null, dest_password: null,
      encrypt_names: encrypted, split_size: 123456789, split_mode: encrypted ? "generic" : "native",
      replace_existing: false, replacement_guard: null },
    progress: byteProgress(state), phase: null, error: ioError("Could not write the converted archive"),
    result: createResult(), revealPath: `${sampleOutputRoot}/client-data`,
    question: null, scanEntries: null, outputPasswordRequired: true,
  };
}

function extractionResult(): Record<string, unknown> {
  const destination = "/tmp/squallz-output/product-backup";
  const problems = Array.from({ length: 20 }, (_, index) => index === 0
    ? `reports/${"annual-reports-and-supporting-documents/".repeat(6)}final  report.pdf: checksum mismatch\nThe file could not be recovered from this archive.`
    : `damaged/item-${String(index + 1).padStart(2, "0")}.bin: checksum mismatch`);
  return { operation: "extract", dest: destination, best_effort: true, skipped: 30, problems,
    problems_total: 30, problems_truncated: true,
    counts: { destination, selected_entries: 42, created: 10, directories: 2, skipped: 0,
      replaced: 0, renamed: 0, failed: 30, output_bytes: 48_000_000 } };
}

function metadataProgress(batch: boolean, state: PreviewState): PreviewProgress {
  const done = batch ? (state === "done" ? 2000 : 1000) : state === "done" ? 48_000_000 : 0;
  const total = batch ? 2000 : state === "done" ? 48_000_000 : 0;
  const current = state === "done" ? "" : `${batch ? "photos.7z: " : ""}project/design/客户交付/September release/resources`;
  return { ...emptyProgress(current), done, total };
}

function extractionPreview(variant: "plain" | "failure" | "password" | "conflict" | "unknownCurrent" | "metadata",
  state: PreviewState, id = 0): PreviewData {
  const failure = variant === "failure";
  const spec: JobSpec = { kind: "extract", path: archive,
    dest: failure ? `${sampleOutputRoot}/Reviewed files` : extractedOutput,
    expected_destination: null, expected_input_guard: null,
    selection: failure ? ["reports/", "Launch plan.pdf"] : null, overwrite: failure ? "rename" : "ask",
    symlinks: failure ? "skip" : "preserve", smart: true, encoding: null, password: null,
    verify_sfx: false, best_effort: false };
  const question: JobQuestion | null = state !== "running" ? null
    : variant === "password" ? { kind: "password", prompt: { id, version: 1, name: "product-backup.zip", wrong: false } }
    : variant === "conflict" ? { kind: "conflict", prompt: { id, version: 1,
      existing_path: `${extractedOutput}/reports/Launch plan.pdf`, existing_size: 4096000,
      existing_modified: 1781190000, incoming_path: "reports/Launch plan.pdf", incoming_size: 5120000,
      incoming_modified: 1781276400 } } : null;
  const metadata = variant === "metadata";
  const unknownCurrent = variant === "unknownCurrent";
  const report = variant === "plain" || metadata || unknownCurrent;
  return {
    spec,
    progress: metadata ? metadataProgress(false, state)
      : byteProgress(state, unknownCurrent ? { currentTotal: 0, runningCurrent: 0 } : {}),
    phase: metadata ? "extract_metadata" : null, error: failure ? ioError("Could not write the extracted file") : null,
    result: report ? extractionResult() : createResult(),
    revealPath: report ? extractedOutput : `${sampleOutputRoot}/client-data`,
    question, scanEntries: null, outputPasswordRequired: false,
  };
}

function batchPreview(variant: "plain" | "metadata" | "partial" | "failure", state: PreviewState): PreviewData {
  const reviewed = variant === "partial" || variant === "failure";
  const items = reviewed ? [
    { path: `${sampleRoot}/Quarterly reports/季度归档与设计资料/Customer delivery with a complete descriptive name.zip`,
      dest: `${sampleOutputRoot}/客户交付/Quarterly reports`, encoding: "gbk", password: null, best_effort: true },
    { path: `${sampleRoot}/finished-photos.7z`, dest: `${sampleOutputRoot}/Photos`, encoding: null, password: null, best_effort: false },
    { path: `${sampleRoot}/logs.tar`, dest: `${sampleOutputRoot}/Logs`, encoding: null, password: null, best_effort: false },
  ] : [
    { path: `${sampleRoot}/client-data.zip`, dest: `${sampleOutputRoot}/client-data`, encoding: null, password: null, best_effort: false },
    { path: `${sampleRoot}/photos.7z`, dest: `${sampleOutputRoot}/photos`, encoding: null, password: null, best_effort: false },
  ];
  const spec: JobSpec = { kind: "batch_extract", items, overwrite: reviewed ? "rename" : "ask",
    symlinks: reviewed ? "skip" : "preserve", smart: !reviewed };
  const result = variant === "partial"
    ? { operation: "batch_extract", archives: 3, selected_archives: 3, collapsed_volumes: 0,
      extracted: 1, failed: 2, outputs: [{ archive: items[1].path, dest: items[1].dest }],
      failures: [items[0], items[2]].map((item) => ({ archive: item.path, error: ioError("Could not write the extracted file") })) }
    : { operation: "batch_extract", archives: 2, extracted: 2, failed: 0, skipped: 0,
      outputs: items.map((item) => ({ path: item.path, dest: item.dest })) };
  const progress = reviewed ? { ...emptyProgress(), done: 3, total: 3 }
    : variant === "metadata" ? metadataProgress(true, state)
    : { ...emptyProgress("photos/IMG_2042.dng"), done: (state === "done" ? 2 : 1) * BATCH_PROGRESS_SCALE,
      total: items.length * BATCH_PROGRESS_SCALE, currentDone: state === "done" ? 3_200_000 : 1_280_000, currentTotal: 3_200_000 };
  return {
    spec, result, progress, phase: variant === "metadata" ? "extract_metadata" : null,
    error: variant === "failure" ? ioError("Could not write the extracted file") : null,
    revealPath: variant === "partial" ? items[1].dest : `${sampleOutputRoot}/client-data`,
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function nestedPreview(state: PreviewState): PreviewData {
  return {
    spec: { kind: "extract_nested", outer_path: `${sampleRoot}/Quarterly delivery with complete project history.zip`,
      entry_path: "客户交付与设计资料/Previous versions/Design assets with a complete descriptive name.7z",
      dest: `${sampleOutputRoot}/客户交付/Reviewed inner archive`, overwrite: "rename", symlinks: "skip",
      smart: false, encoding: "gbk", password: null, best_effort: true },
    progress: byteProgress(state), phase: null, error: ioError("Could not write the extracted file"),
    result: createResult(), revealPath: `${sampleOutputRoot}/client-data`,
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function inspectionPreview(variant: "open" | "test", state: PreviewState): PreviewData {
  const opening = variant === "open";
  return {
    spec: { kind: "test", path: opening ? archive : `${sampleRoot}/inspection/damaged-backup.zip`, encoding: null, password: null },
    progress: opening ? emptyProgress(state === "done" ? "" : "product-backup.zip")
      : byteProgress(state, { speed: 17_600_000, currentTotal: 0, runningCurrent: 0 }),
    phase: opening ? "archive_open" : "archive_test", error: null,
    result: { operation: "test", ok: false, entries: 42, entries_tested: 42,
      problems: Array.from({ length: 20 }, (_, index) => `damaged/item-${String(index + 1).padStart(2, "0")}.bin: checksum mismatch`),
      problems_total: 30, problems_truncated: true },
    revealPath: null, question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function updatePreview(variant: "failure" | "scan" | "verify" | "commit", state: PreviewState): PreviewData {
  const failure = variant === "failure";
  const spec: JobSpec = failure
    ? { kind: "update", path: `${sampleRoot}/客户交付/Quarterly delivery with complete project history.zip`, expected_archive_id: null,
      add: [`${sampleRoot}/Revised documents/完整项目说明.txt`], mkdir: ["交付文档/审核记录/"], delete: ["Previous versions/旧资料[1].txt"],
      rename: [{ from: "Previous versions/Design assets with a complete descriptive name.pdf", to: "交付文档/Design assets with a complete descriptive name.pdf" }],
      encoding: "gbk", content_policy: "custom", excludes: ["*.bak", ".DS_Store"], level: 3, password: null }
    : { kind: "update", path: archive, expected_archive_id: null, encoding: null,
      add: [`${sampleRoot}/incoming-assets`], delete: [], rename: [], mkdir: [], excludes: [".DS_Store"],
      content_policy: "keep_all_files", password: null, level: 5 };
  const progress = failure ? byteProgress(state)
    : variant === "scan" ? emptyProgress("incoming-assets/icons/app-icon@2x.png")
    : variant === "verify" ? { ...emptyProgress("product-backup.zip"), done: 438_000_000, total: 730_000_000, speed: 820_000_000 }
    : emptyProgress("product-backup.zip");
  return {
    spec, progress, phase: variant === "verify" ? "update_verify" : variant === "commit" ? "update_commit" : null,
    error: failure ? ioError("Could not replace the archive") : null,
    result: failure ? createResult() : { operation: "update" }, revealPath: failure ? `${sampleOutputRoot}/client-data` : spec.path,
    question: null, scanEntries: variant === "scan" && state === "running" ? 128 : null, outputPasswordRequired: false,
  };
}

function protectionPreview(state: PreviewState): PreviewData {
  const recovery = `${sampleOutputRoot}/product-backup.zip.par2`;
  return {
    spec: { kind: "protect", path: archive, redundancy: 10, recovery },
    progress: { ...emptyProgress("product-backup.zip.par2"), done: state === "done" ? 1 : 0, total: 1 },
    phase: "recovery_finalize", error: null, revealPath: recovery,
    result: { operation: "protect", ok: true, archive, recovery,
      outputs: [recovery, ...["00+01", "01+02", "03+04", "07+08", "15+16"].map((part) => `${sampleOutputRoot}/product-backup.zip.vol${part}.par2`)],
      source_file_count: 1, redundancy_percent: 10 },
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function recoveryPreview(variant: "repairable" | "multiFile" | "overCapacity", state: PreviewState): PreviewData {
  const recovery = `${sampleRoot}/product-backup.zip.par2`;
  const overCapacity = variant === "overCapacity";
  return {
    spec: { kind: "verify_recovery", path: archive, recovery },
    progress: { ...emptyProgress("product-backup.zip"), done: state === "done" ? 1000 : 380, total: 1000 },
    phase: "recovery_verify", error: null, revealPath: null,
    result: { operation: "verify", ok: false, archive, recovery, output: null, tool: "rust-par2", redundancy_percent: null,
      source_file_count: variant === "multiFile" ? 2 : 1, status_code: 1,
      metrics: { all_correct: false, repair_possible: !overCapacity, blocks_needed: overCapacity ? 12 : 3,
        recovery_blocks_available: overCapacity ? 4 : 8, blocks_repaired: null, files_repaired: null, no_damage: false },
      stdout: "", stderr: "damage found" },
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function cleanupPreview(variant: "ready" | "unconfirmed" | "record", state: PreviewState): PreviewData {
  const target = `${sampleOutputRoot}/product-backup.repaired.zip`;
  const workspace = `${sampleOutputRoot}/.product-backup.repaired.zip.sqz-par2-repair-940018-1.work`;
  const journal = `${sampleOutputRoot}/.squallz-par2-repair-8f3d4a9e1c7b2d5f.json`;
  const detail = variant === "ready"
    ? `PAR2 repair completed and the repaired copy is ready at ${target}, but its private workspace could not be removed; automatic recovery record: ${journal}; exact workspace: ${workspace}`
    : variant === "unconfirmed"
      ? `PAR2 repair was not confirmed, and its private workspace could not be removed; automatic recovery record: ${journal}; exact workspace: ${workspace}`
      : `The target-bound PAR2 recovery record at ${journal} is damaged; no workspace path was trusted or removed.`;
  return {
    spec: { kind: "repair_recovery", path: archive, output: target, output_directory: false, recovery: `${sampleRoot}/product-backup.zip.par2` },
    progress: byteProgress(state), phase: "recovery_finalize",
    error: { key: variant === "ready" ? "error.recovery_cleanup_output_ready"
      : variant === "unconfirmed" ? "error.recovery_cleanup_unconfirmed" : "error.recovery_cleanup_record",
      params: variant === "record" ? { target, journal } : { target, workspace, journal }, detail },
    result: createResult(), revealPath: `${sampleOutputRoot}/client-data`,
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function duplicatePreview(variant: "groups" | "clean" | "failure", state: PreviewState): PreviewData {
  const clean = variant === "clean";
  const groups = clean ? [] : Array.from({ length: 25 }, (_, index) => {
    const paths = Array.from({ length: index === 0 ? 60 : 2 }, (_, copy) =>
      `${sampleRoot}/Archive review/Project ${index + 1}/Copy ${copy + 1}/季度归档与设计资料/Final presentation with a descriptive file name.pdf`);
    return { hash: (index + 1).toString(16).padStart(64, "0"), hash_algorithm: "blake3", size: 2048,
      count: paths.length, reclaimable_bytes: (paths.length - 1) * 2048, paths };
  });
  const count = groups.reduce((total, group) => total + group.paths.length, 0);
  const progressBytes = state === "done" ? (clean ? 4 : 112) * 2048 : 0;
  return {
    spec: { kind: "duplicate_scan", inputs: [`${sampleRoot}/${clean ? "Inbox" : "Archive review"}`], excludes: ["cache"], min_size: 1024 },
    progress: { ...emptyProgress(), done: progressBytes, total: progressBytes }, phase: null,
    error: variant === "failure" ? ioError("Could not read the scan folder") : null,
    result: { operation: "duplicates", hash_algorithm: "blake3", input_count: 1, min_size: 1024,
      files_scanned: count + 4, bytes_scanned: (count + 4) * 2048, candidate_files: count, hashed_bytes: count * 2048,
      duplicate_groups: groups.length, duplicate_files: count, reclaimable_bytes: (count - groups.length) * 2048, groups },
    revealPath: null, question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

function checksumPreview(check: boolean, state: PreviewState): PreviewData {
  return {
    spec: check ? { kind: "checksum_check", manifest: `${photos}/SHA256SUMS`, algorithm: "sha256" }
      : { kind: "checksum", inputs: [photos], excludes: [], algorithm: "sha256" },
    progress: byteProgress(state, {
      total: 86_000_000,
      current: "photos/DSC_1930.JPG",
      speed: check ? 22_000_000 : 24_000_000,
      currentTotal: 0,
      runningCurrent: 0,
    }),
    phase: null, error: null, revealPath: check ? `${photos}/SHA256SUMS` : photos,
    result: check ? { operation: "checksum_check", passed: 12, checked: 12, failed: 0,
      items: [{ path: `${photos}/DSC_1930.JPG`, expected: photoDigest, actual: photoDigest, ok: true }] }
      : { operation: "checksum", algorithm: "sha256", files_hashed: 12, bytes_hashed: 86_000_000,
        items: [{ path: `${photos}/DSC_1930.JPG`, size: 18_200_000, digest: photoDigest },
          { path: `${photos}/DSC_1488.JPG`, size: 9_200_000, digest: "37166b84dfd4083c0f6fb7b99d892bc3ef8ff07c9a1714ad9f323bdb37e9f9a2" }] },
    question: null, scanEntries: null, outputPasswordRequired: false,
  };
}

export function createTaskPreviewHistory(kind: "checksum" | "checksum_check") {
  const source = `${sampleRoot}/current-release`;
  const path = `${source}/release.txt`;
  const digest = "ab".repeat(64);
  const spec: JobSpec = kind === "checksum"
    ? { kind, inputs: [source], excludes: [], algorithm: "sha512" }
    : { kind, manifest: `${source}/SHA512SUMS`, algorithm: "sha512" };
  return {
    spec,
    progress: { done: 64, total: 64, current: "release.txt", currentDone: 64, currentTotal: 64 },
    revealPath: source,
    result: kind === "checksum" ? { operation: kind, algorithm: "sha512", files_hashed: 1, bytes_hashed: 64, items: [{ path, digest, size: 64 }] }
      : { operation: kind, ok: false, checked: 1, passed: 0, failed: 1, items: [{ path, expected: digest, actual: "cd".repeat(64), ok: false }] },
  };
}
