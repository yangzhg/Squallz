import { resultOf, type ChecksumItem } from "./job-result";
import type { ArchiveInfo, JobSpec } from "./ipc";
import type { Task } from "./jobs.svelte";
import { copyTextToClipboard } from "./clipboard";
import { basename, dirname, formatBytes, parseDelimitedRules } from "./format";
import { tFallback as tr } from "./i18n.svelte";
import {
  checksumItemStatus, checksumItemText, checksumResultLine, taskChecksumResultText,
  taskDuplicateGroups, taskOutcomeStateLabel, taskStateLabel, type TaskDialogModel,
} from "./task-model";
import { recordOperation } from "./history.svelte";
import { checksumAlgorithms, type ChecksumAlgorithmId } from "./ui-model";

export type ChecksumResultKind = "checksum" | "checksum_check";
type ToolKind = ChecksumResultKind | "duplicate_scan";
type CopyKind = ChecksumResultKind | "task";
type PickerKind = "file" | "folder" | "manifest" | "duplicates";
type SizeUnit = "b" | "kib" | "mib" | "gib";
const sizeUnits: Readonly<Record<SizeUnit, number>> = { b: 1, kib: 1024, mib: 1024 ** 2, gib: 1024 ** 3 };
const checksumPageSize = 50;
type ChecksumView = { taskId: number | null; page: number; failuresOnly: boolean };
type ToolsDraft = {
  checksumPath: string;
  manifestPath: string;
  algorithm: ChecksumAlgorithmId;
  checksumExcludes: string;
  duplicatePath: string;
  minimumSize: number;
  duplicateExcludes: string;
};
type CopyFeedback = Readonly<{
  kind: CopyKind; taskId: number | null; message: string; tone: "success" | "danger";
}>;

export interface ToolsSessionPort {
  read: () => Readonly<{
    archive: ArchiveInfo | null;
    tasks: readonly Task[];
    screen: string;
    copyScope: string;
    visibleTaskId: number | null;
  }>;
  choose: (
    purpose: string,
    options: { title: string; directory?: boolean; multiple: false },
    isCurrent: () => boolean,
  ) => Promise<string | string[] | null>;
  submitJob: (spec: JobSpec) => Promise<number>;
  submissionBlocked: () => boolean;
  isSubmitBlocked: (error: unknown) => boolean;
  notice: (message: string) => void;
  copy?: typeof copyTextToClipboard;
}

/** Owns local-tool inputs, task-bound reports, pickers and clipboard requests. */
export class ToolsSession {
  #draft = $state<ToolsDraft>({
    checksumPath: "", manifestPath: "", algorithm: "sha256",
    checksumExcludes: ".git\nnode_modules\n.DS_Store", duplicatePath: "", minimumSize: 1048576,
    duplicateExcludes: ".git\nnode_modules\n.DS_Store",
  });
  #selected = $state<Partial<Record<ToolKind, number>>>({});
  #minimumSizeError = $state("");
  #minimumSizeUnit = $state<SizeUnit>("mib");
  #checksumViews = $state<Partial<Record<ChecksumResultKind, ChecksumView>>>({});
  #feedback = $state<CopyFeedback | null>(null);
  #copyRequest = $state.raw<{ kind: CopyKind; taskId: number | null; isCurrent: () => boolean } | null>(null);
  #feedbackTimer: ReturnType<typeof setTimeout> | null = null;
  #pickerGeneration = 0;
  #disposed = false;

  readonly algorithms = checksumAlgorithms;

  constructor(private readonly port: ToolsSessionPort, seed: Partial<ToolsDraft> = {}) {
    Object.assign(this.#draft, seed);
    this.#minimumSizeUnit = (["gib", "mib", "kib"] as const)
      .find((unit) => this.#draft.minimumSize >= sizeUnits[unit] && this.#draft.minimumSize % sizeUnits[unit] === 0) ?? "b";
  }

  get draft(): Readonly<ToolsDraft> { return this.#draft; }
  get selectedReports(): Readonly<Partial<Record<ToolKind, number>>> { return this.#selected; }
  get minimumSizeError(): string { return this.#minimumSizeError; }
  get minimumSizeUnit(): SizeUnit { return this.#minimumSizeUnit; }
  get minimumSizeValue(): number { return this.#draft.minimumSize / sizeUnits[this.#minimumSizeUnit]; }
  get minimumSizeLabel(): string { return formatBytes(this.#draft.minimumSize); }
  get feedback(): CopyFeedback | null { return this.#feedback; }

  edit(edit: Partial<ToolsDraft>): void {
    this.#pickerGeneration += 1;
    Object.assign(this.#draft, edit);
  }

  adoptTask(task: TaskDialogModel): void {
    if (task.id !== null && ["checksum", "checksum_check", "duplicate_scan"].includes(task.spec.kind)) {
      this.leave();
      this.#selected[task.spec.kind as ToolKind] = task.id;
    }
  }

  leave(): void {
    this.#pickerGeneration += 1;
    this.#copyRequest = null;
  }

  syncContext(): void {
    if (this.#copyRequest && !this.#copyRequest.isCurrent()) this.#copyRequest = null;
  }

  dispose(): void {
    this.leave();
    this.#disposed = true;
    this.clearCopyFeedback();
  }

  selectedTask(kind: ToolKind): Task | null {
    const tasks = this.port.read().tasks;
    return tasks.find((task) => task.id === this.#selected[kind] && task.spec.kind === kind)
      ?? [...tasks].reverse().find((task) => task.spec.kind === kind) ?? null;
  }

  target(kind: "checksum" | "duplicate_scan"): string {
    const archive = this.port.read().archive;
    return kind === "checksum"
      ? this.#draft.checksumPath.trim() || (archive && !archive.read_only ? archive.path : "")
      : this.#draft.duplicatePath.trim() || (archive ? dirname(archive.path) : "");
  }

  targetLabel(kind: "checksum" | "duplicate_scan"): string {
    return this.target(kind) || (kind === "checksum"
      ? tr("gui.checksum.choose_file_or_folder")
      : tr("gui.duplicates.choose_folder_or_open_archive", "Choose a folder or open an archive"));
  }

  targetName(kind: "checksum" | "duplicate_scan"): string {
    const target = this.target(kind);
    return target ? basename(target) || target : kind === "checksum"
      ? tr("gui.checksum.no_target_selected")
      : tr("gui.duplicates.no_folder_selected");
  }

  get manifestLabel(): string {
    return this.#draft.manifestPath.trim() || tr("gui.checksum.choose_manifest_prompt");
  }

  get hasArchive(): boolean { return this.port.read().archive !== null; }

  get currentArchiveDisabledReason(): string {
    const archive = this.port.read().archive;
    return !archive ? tr("gui.empty.open_archive_first") : archive.read_only
      ? tr("gui.checksum.nested_requires_extract") : "";
  }

  useArchive(kind: "checksum" | "duplicate_scan"): void {
    const archive = this.port.read().archive;
    const issue = kind === "checksum" ? this.currentArchiveDisabledReason : !archive
      ? tr("gui.duplicates.open_archive_or_choose_folder") : "";
    if (issue) { this.port.notice(issue); return; }
    if (!archive) return;
    this.edit(kind === "checksum" ? { checksumPath: archive.path } : { duplicatePath: dirname(archive.path) });
    this.port.notice(kind === "checksum"
      ? tr("gui.checksum.target_current_archive")
      : tr("gui.duplicates.target_archive_folder"));
  }

  excludes(kind: "checksum" | "duplicate_scan"): string[] {
    return parseDelimitedRules(kind === "checksum" ? this.#draft.checksumExcludes : this.#draft.duplicateExcludes);
  }

  updateMinimumSize(event: Event): void {
    const raw = (event.currentTarget as HTMLInputElement).value.trim();
    const value = Number(raw) * sizeUnits[this.#minimumSizeUnit];
    this.#minimumSizeError = !raw || !Number.isSafeInteger(value) || value < 0
      ? tr("gui.duplicates.min_size_invalid") : "";
    if (this.#minimumSizeError) this.port.notice(this.#minimumSizeError);
    else this.#draft.minimumSize = value;
  }

  changeMinimumSizeUnit(unit: string): void {
    if (!(unit in sizeUnits)) return;
    this.#minimumSizeUnit = unit as SizeUnit;
    this.#minimumSizeError = "";
  }

  async choose(kind: PickerKind): Promise<void> {
    const request = ++this.#pickerGeneration;
    const screen = this.port.read().screen;
    const current = () => !this.#disposed && request === this.#pickerGeneration && this.port.read().screen === screen;
    const choices = {
      file: {
        purpose: "checksum.file",
        title: tr("gui.checksum.choose_file_to_checksum"),
        unchanged: tr("gui.checksum.target_unchanged"),
        selected: tr("gui.checksum.target_selected"),
        failure: tr("gui.checksum.choose_file_requires_desktop_dialog"),
      },
      folder: {
        purpose: "checksum.folder",
        title: tr("gui.checksum.choose_folder_to_checksum"),
        unchanged: tr("gui.checksum.target_unchanged"),
        selected: tr("gui.checksum.folder_selected"),
        failure: tr("gui.checksum.choose_folder_requires_desktop_dialog"),
      },
      manifest: {
        purpose: "checksum.manifest",
        title: tr("gui.checksum.choose_manifest", "Choose checksum manifest"),
        unchanged: tr("gui.checksum.manifest_unchanged"),
        selected: tr("gui.checksum.manifest_selected"),
        failure: tr("gui.checksum.choose_manifest_requires_desktop_dialog"),
      },
      duplicates: {
        purpose: "duplicates.folder",
        title: tr("gui.duplicates.choose_folder_to_scan"),
        unchanged: tr("gui.duplicates.folder_unchanged"),
        selected: tr("gui.duplicates.folder_selected"),
        failure: tr("gui.duplicates.choose_folder_requires_desktop_dialog"),
      },
    };
    const choice = choices[kind];
    try {
      const selected = await this.port.choose(choice.purpose, {
        title: choice.title, multiple: false,
        ...(kind === "folder" || kind === "duplicates" ? { directory: true } : {}),
      }, current);
      if (!current()) return;
      if (!selected || Array.isArray(selected)) {
        this.port.notice(choice.unchanged);
        return;
      }
      this.edit(kind === "manifest" ? { manifestPath: selected } : kind === "duplicates"
        ? { duplicatePath: selected } : { checksumPath: selected });
      this.port.notice(choice.selected);
    } catch {
      if (current()) this.port.notice(choice.failure);
    }
  }

  async submit(kind: ToolKind): Promise<void> {
    const target = kind === "checksum_check" ? this.#draft.manifestPath.trim() : this.target(kind);
    const missing = kind === "checksum"
      ? tr("gui.checksum.choose_before_checksumming")
      : kind === "checksum_check"
        ? tr("gui.checksum.choose_manifest_before_verifying")
        : tr("gui.duplicates.choose_folder_before_scan");
    if (kind === "duplicate_scan" && this.#minimumSizeError) { this.port.notice(this.#minimumSizeError); return; }
    if (!target) { this.port.notice(missing); return; }
    if (this.#disposed || this.port.submissionBlocked()) return;
    const spec: JobSpec = kind === "checksum_check"
      ? { kind, manifest: target, algorithm: this.#draft.algorithm }
      : kind === "checksum"
        ? { kind, inputs: [target], excludes: this.excludes(kind), algorithm: this.#draft.algorithm }
        : { kind, inputs: [target], excludes: this.excludes(kind), min_size: Math.max(0, Math.floor(this.#draft.minimumSize)) };
    const title = kind === "checksum" ? tr("gui.checksum.queued")
      : kind === "checksum_check" ? tr("gui.checksum.verification_queued")
        : tr("gui.duplicates.queued");
    const detail = kind === "duplicate_scan"
      ? tr("gui.duplicates.operation_detail").replace("{target}", basename(target) || target).replace("{min}", formatBytes(this.#draft.minimumSize))
      : `${basename(target) || target} · ${this.#draft.algorithm}`;
    try {
      const id = await this.port.submitJob(spec);
      if (this.#disposed) return;
      this.#selected[kind] = id;
      this.port.notice(title);
      recordOperation({ status: "queued", title, detail });
    } catch (error) {
      if (this.#disposed || this.port.isSubmitBlocked(error)) return;
      this.port.notice(kind === "checksum" ? tr("gui.checksum.requires_desktop_service")
        : kind === "checksum_check" ? tr("gui.checksum.verification_requires_desktop_service")
          : tr("gui.duplicates.requires_desktop_service"));
    }
  }

  result<K extends ToolKind>(kind: K) { return resultOf(this.selectedTask(kind)?.result, kind); }

  resultLabel(value: number | null | undefined, bytes = false): string {
    return value == null ? "—" : bytes ? formatBytes(value) : value.toLocaleString();
  }

  checksumItems(kind: ChecksumResultKind): ChecksumItem[] { return this.result(kind)?.items ?? []; }

  checksumText(kind: ChecksumResultKind): string {
    return this.checksumItems(kind).map((item) => checksumResultLine(kind, item)).filter((line) => line.trim()).join("\n");
  }

  copyPending(kind: CopyKind, taskId: number | null): boolean {
    return this.#copyRequest?.kind === kind && this.#copyRequest.taskId === taskId && this.#copyRequest.isCurrent();
  }

  copyFeedback(kind: CopyKind, taskId = kind === "task" ? null : this.selectedTask(kind)?.id ?? null): CopyFeedback | null {
    return this.#feedback?.kind === kind && this.#feedback.taskId === taskId ? this.#feedback : null;
  }

  copyResults(kind: ChecksumResultKind): Promise<void> {
    return this.copyText(this.checksumText(kind), kind, this.selectedTask(kind)?.id ?? null);
  }

  copyTask(task: TaskDialogModel): Promise<void> {
    return this.copyText(taskChecksumResultText(task), "task", task.id);
  }

  async copyText(text: string, kind: CopyKind, taskId: number | null): Promise<void> {
    this.clearCopyFeedback();
    const origin = this.port.read().copyScope;
    const isCurrent = () => {
      const context = this.port.read();
      return !this.#disposed && this.#copyRequest === request && context.copyScope === origin
        && (kind === "task" ? context.visibleTaskId === taskId
          : context.screen === "checksum" && (this.selectedTask(kind)?.id ?? null) === taskId);
    };
    const request = { kind, taskId, isCurrent };
    this.#copyRequest = request;
    if (!isCurrent()) { this.#copyRequest = null; return; }
    try {
      if (!text.trim()) {
        this.showCopyFeedback(kind, taskId, tr("gui.checksum.no_copyable_results"), "danger");
        return;
      }
      const result = await (this.port.copy ?? copyTextToClipboard)(text, isCurrent);
      if (!isCurrent() || result === "superseded") return;
      this.showCopyFeedback(kind, taskId, result === "copied"
        ? tr("gui.checksum.results_copied")
        : tr("gui.checksum.copy_failed"), result === "copied" ? "success" : "danger");
    } finally { if (this.#copyRequest === request) this.#copyRequest = null; }
  }

  private clearCopyFeedback(): void {
    this.#copyRequest = null;
    if (this.#feedbackTimer !== null) clearTimeout(this.#feedbackTimer);
    this.#feedbackTimer = null;
    this.#feedback = null;
  }

  private showCopyFeedback(kind: CopyKind, taskId: number | null, message: string, tone: "success" | "danger"): void {
    this.#feedback = { kind, taskId, message, tone };
    this.port.notice(message);
    if (this.#feedbackTimer !== null) clearTimeout(this.#feedbackTimer);
    this.#feedbackTimer = setTimeout(() => this.clearCopyFeedback(), 2600);
  }

  checksumReport(kind: ChecksumResultKind) {
    const task = this.selectedTask(kind);
    const items = this.checksumItems(kind);
    const taskId = task?.id ?? null;
    const failures = kind === "checksum_check" ? items.filter((item) => "ok" in item && !item.ok) : [];
    const saved = this.#checksumViews[kind];
    const view = saved?.taskId === taskId ? saved : { taskId, page: 0, failuresOnly: failures.length > 0 };
    const filtered = view.failuresOnly ? failures : items;
    const pageCount = Math.max(1, Math.ceil(filtered.length / checksumPageSize));
    const page = Math.min(view.page, pageCount - 1);
    const start = page * checksumPageSize;
    const calculated = this.result("checksum");
    const checked = this.result("checksum_check");
    const metrics = kind === "checksum" ? [
      [calculated?.files_hashed ?? 0, tr("gui.checksum.hashed_files"), false],
      [calculated?.bytes_hashed ?? 0, tr("gui.checksum.hashed_bytes"), true],
    ] as const : [
      [checked?.passed ?? 0, tr("gui.checksum.passed"), false],
      [checked?.failed ?? 0, tr("gui.checksum.failed"), false],
      [checked?.checked ?? 0, tr("gui.checksum.checked"), false],
    ] as const;
    const report = {
      taskId,
      context: task?.spec.kind === "checksum" || task?.spec.kind === "checksum_check" ? {
        sources: task.spec.kind === "checksum" ? task.spec.inputs : [task.spec.manifest],
        algorithm: this.algorithmLabel(task.spec.algorithm),
      } : null,
      summary: metrics.map(([value, label, bytes]) => ({ label, value: this.resultLabel(value, bytes) })),
      totalRows: items.length, filteredRows: filtered.length, failedRows: failures.length,
      page, pageCount, start: filtered.length ? start + 1 : 0, end: Math.min(start + checksumPageSize, filtered.length),
      failuresOnly: view.failuresOnly,
      onPage: (next: number) => {
        if (!Number.isFinite(next) || this.selectedTask(kind)?.id !== taskId) return;
        this.#checksumViews[kind] = { taskId, failuresOnly: view.failuresOnly, page: Math.max(0, Math.min(Math.floor(next), pageCount - 1)) };
      },
      onFilter: (failuresOnly: boolean) => {
        if (this.selectedTask(kind)?.id !== taskId) return;
        this.#checksumViews[kind] = { taskId, page: 0, failuresOnly: kind === "checksum_check" && failuresOnly };
      },
      matchesInput: task?.spec.kind === "checksum"
        ? task.spec.algorithm === this.#draft.algorithm && task.spec.inputs.length === 1 && task.spec.inputs[0] === this.target("checksum")
          && JSON.stringify(task.spec.excludes) === JSON.stringify(this.excludes("checksum"))
        : task?.spec.kind === "checksum_check"
          ? task.spec.algorithm === this.#draft.algorithm && task.spec.manifest === this.#draft.manifestPath.trim() : false,
      state: task ? taskOutcomeStateLabel(task) : taskStateLabel(null),
      feedback: this.copyFeedback(kind)?.message ?? null, feedbackDanger: this.copyFeedback(kind)?.tone === "danger",
      copyPending: this.copyPending(kind, task?.id ?? null), onCopy: () => void this.copyResults(kind),
    };
    const rows = filtered.slice(start, start + checksumPageSize);
    return kind === "checksum" ? {
      ...report, kind,
      rows: rows.map((item) => ({
        name: checksumItemText(item, "path"), status: checksumItemStatus(item),
        size: formatBytes("size" in item ? item.size : 0),
        digest: checksumItemText(item, "digest"),
      })),
    } : {
      ...report, kind,
      rows: rows.map((item) => ({
        name: checksumItemText(item, "path"), status: checksumItemStatus(item),
        expected: checksumItemText(item, "expected"),
        actual: checksumItemText(item, "actual") || checksumItemText(item, "error"),
      })),
    };
  }

  get duplicateReport() {
    const task = this.selectedTask("duplicate_scan");
    const result = this.result("duplicate_scan");
    return {
      taskId: task?.id ?? null, state: task ? taskOutcomeStateLabel(task) : taskStateLabel(null),
      context: task?.spec.kind === "duplicate_scan" ? { sources: task.spec.inputs,
        minimumSize: formatBytes(task.spec.min_size), excludes: task.spec.excludes } : null,
      summary: ([
        [result?.files_scanned, tr("gui.duplicates.files_scanned"), false],
        [result?.bytes_scanned, tr("gui.duplicates.bytes_scanned"), true],
        [result?.candidate_files, tr("gui.duplicates.candidates_hashed"), false],
        [result?.hashed_bytes, tr("gui.duplicates.hashed_bytes"), true],
        [result?.duplicate_groups, tr("gui.duplicates.groups"), false],
        [result?.duplicate_files, tr("gui.duplicates.files"), false],
        [result?.reclaimable_bytes, tr("gui.duplicates.reclaimable"), true],
      ] as const).map(([value, label, bytes]) => ({ label, value: this.resultLabel(value, bytes) })),
      groups: taskDuplicateGroups(task),
      emptyMessage: !task ? tr("gui.duplicates.report_empty")
        : task.state === "done" && result?.duplicate_groups === 0 && result.groups.length === 0
          ? tr("gui.duplicates.report_clean")
          : task.state === "queued" || task.state === "running" || task.state === "paused"
            ? tr("gui.duplicates.report_pending")
            : tr("gui.duplicates.report_unavailable"),
    };
  }

  algorithmLabel(algorithm: ChecksumAlgorithmId = this.#draft.algorithm): string {
    return ({ sha256: "SHA-256", sha224: "SHA-224", sha384: "SHA-384", sha512: "SHA-512", sha1: "SHA-1", md5: "MD5", blake3: "BLAKE3", crc32: "CRC32" })[algorithm];
  }

  algorithmHint(algorithm: ChecksumAlgorithmId): string {
    const hints = { sha256: "Default", sha224: "SHA-2", sha384: "Signed releases", sha512: "Wide SHA-2", sha1: "Weak", md5: "Weak", blake3: "Fast", crc32: "ZIP CRC" };
    return tr(`gui.checksum.algorithm_${algorithm}_hint`, hints[algorithm]);
  }
}
