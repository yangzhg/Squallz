import { sameDesktopPath, type DesktopPathPlatform } from "./desktop-path";
import type { ExtractPlanInput } from "./extract-plan.svelte";
import type { ExtractArchivePresetOptions, ExtractPlanPreflightDto, JobSpec } from "./ipc";

type ExtractJob = Extract<JobSpec, { kind: "extract" }>;
export type ExtractDestinationMode = "smart" | "archive" | "same" | "choose";
export type ExtractScope = "all" | "selection";
export type ExtractOverwriteMode = ExtractJob["overwrite"];
export type ExtractSymlinkMode = ExtractJob["symlinks"];
export type ExtractTaskDraft = Pick<ExtractJob,
  "path" | "dest" | "selection" | "overwrite" | "symlinks" | "smart" | "encoding" | "verify_sfx">;
type ArchiveSource = Readonly<{ id: number; source: string; path: string; encoding_override?: string | null }>;
type ArchiveIdentity = Readonly<Pick<ArchiveSource, "id" | "source">>;
type DestinationPaths = Readonly<{ archiveFolder: string; archiveParent: string; defaultDirectory: string | null }>;
type ExtractPolicyChange =
  | { kind: "overwrite"; value: ExtractOverwriteMode }
  | { kind: "symlinks"; value: ExtractSymlinkMode };
type ExtractOptionsState = {
  destinationMode: ExtractDestinationMode;
  customDest: string;
  smartBaseOverride: string | null;
  scope: ExtractScope;
  selection: readonly string[];
  overwrite: ExtractOverwriteMode;
  symlinks: ExtractSymlinkMode;
  presetEncodingLabel: string | null;
  verifySfx: boolean;
  archive: ArchiveIdentity | null;
  touched: boolean;
};
export type ExtractRunSnapshot = Readonly<{
  input: ExtractPlanInput;
  overwrite: ExtractOverwriteMode;
  symlinks: ExtractSymlinkMode;
  verifySfx: boolean;
}>;

const presetDestinations: Record<ExtractDestinationMode, ExtractArchivePresetOptions["destination"]> = {
  smart: { base: "default_directory", layout: "smart" },
  archive: { base: "default_directory", layout: "archive_folder" },
  choose: { base: "ask", layout: "direct" },
  same: { base: "archive_parent", layout: "direct" },
};

/** Owns ordinary archive extraction options; the host admits desktop replies and owns native plans. */
export class ExtractOptionsDraft {
  #state = $state<ExtractOptionsState>({
    destinationMode: "smart", customDest: "", smartBaseOverride: null,
    scope: "all", selection: [], overwrite: "ask", symlinks: "preserve",
    presetEncodingLabel: null, verifySfx: false, archive: null, touched: false,
  });

  constructor(private readonly platform: () => DesktopPathPlatform) {}

  get state(): Readonly<ExtractOptionsState> { return this.#state; }

  syncArchive(archive: ArchiveSource | null): boolean {
    const previous = this.#state.archive;
    if ((previous?.id ?? null) === (archive?.id ?? null)) return false;
    const sameSource = archive && previous && sameDesktopPath(archive.source, previous.source, this.platform());
    this.#state.archive = archive ? { id: archive.id, source: archive.source } : null;
    this.#state.selection = [];
    if (!sameSource) {
      this.#state.smartBaseOverride = null;
      this.#state.verifySfx = false;
    }
    return true;
  }

  openScope(archive: ArchiveSource, scope: ExtractScope, selection: string[] | null, beforeOpen: () => void): boolean {
    if (scope === "selection" && (!selection || selection.length === 0)) return false;
    beforeOpen();
    this.syncArchive(archive);
    this.#state.scope = scope;
    this.#state.selection = scope === "selection" ? [...(selection ?? [])] : [];
    return true;
  }

  selectDestination(mode: Exclude<ExtractDestinationMode, "choose">): void {
    this.#state.destinationMode = mode;
    this.#state.smartBaseOverride = null;
    this.#state.touched = true;
  }

  acceptDestination(path: string): boolean {
    if (!path.trim()) return false;
    this.#state.customDest = path;
    this.#state.smartBaseOverride = null;
    this.#state.destinationMode = "choose";
    this.#state.touched = true;
    return true;
  }

  selectPolicy(change: ExtractPolicyChange): void {
    this.#restorePolicies(change.kind === "overwrite" ? change.value : this.#state.overwrite,
      change.kind === "symlinks" ? change.value : this.#state.symlinks, this.#state.presetEncodingLabel);
    this.#state.touched = true;
  }

  applyPreset(options: ExtractArchivePresetOptions): void {
    const destination = options.destination;
    if (destination.layout === "smart") this.#state.destinationMode = "smart";
    else if (destination.layout === "archive_folder") this.#state.destinationMode = "archive";
    else this.#state.destinationMode = destination.base === "ask" ? "choose" : "same";
    this.#state.smartBaseOverride = null;
    if (this.#state.destinationMode === "choose") this.#state.customDest = "";
    this.#restorePolicies(options.existing_output, options.symlinks,
      options.encoding.kind === "named" ? options.encoding.label : null);
  }

  restoreTask(archive: ArchiveSource, draft: ExtractTaskDraft, beforeRestore: () => void): boolean {
    if (!sameDesktopPath(archive.source, draft.path, this.platform())) return false;
    beforeRestore();
    this.#state.archive = { id: archive.id, source: archive.source };
    this.#state.scope = draft.selection === null ? "all" : "selection";
    this.#state.selection = [...(draft.selection ?? [])];
    this.#state.destinationMode = draft.smart ? "smart" : "choose";
    this.#state.customDest = draft.dest;
    this.#state.smartBaseOverride = draft.smart ? draft.dest : null;
    this.#restorePolicies(draft.overwrite, draft.symlinks, null);
    this.#state.verifySfx = draft.verify_sfx;
    this.#state.touched = true;
    return true;
  }

  encodingReopened(): void {
    this.#state.presetEncodingLabel = null;
    this.#state.touched = true;
  }

  selectionSnapshot(): string[] | null {
    return this.#state.scope === "selection" ? [...this.#state.selection] : null;
  }

  encodingForJob(currentEncoding: string | null): string | null {
    return this.#state.presetEncodingLabel ?? currentEncoding;
  }

  smartBase(defaultDirectory: string | null, archiveParent: string): string {
    return this.#state.smartBaseOverride ?? defaultDirectory ?? archiveParent;
  }

  requestedDestination(paths: DestinationPaths): string {
    switch (this.#state.destinationMode) {
      case "smart": return this.smartBase(paths.defaultDirectory, paths.archiveParent);
      case "same": return paths.archiveParent;
      case "choose": return this.#state.customDest.trim() || paths.archiveFolder;
      case "archive": return paths.archiveFolder;
    }
  }

  snapshotPreset(currentEncoding: string | null): ExtractArchivePresetOptions {
    const encoding = this.encodingForJob(currentEncoding);
    return {
      destination: { ...presetDestinations[this.#state.destinationMode] },
      existing_output: this.#state.overwrite, symlinks: this.#state.symlinks,
      encoding: encoding ? { kind: "named", label: encoding } : { kind: "auto" },
      credential: { kind: "prompt_when_needed" }, post_success: "keep_source",
    };
  }

  snapshotPlan(archive: ArchiveSource, destination: string): ExtractPlanInput {
    return this.#inputSnapshot(archive, destination, this.selectionSnapshot());
  }

  snapshotRun(archive: ArchiveSource, destination: string): ExtractRunSnapshot {
    return { input: this.snapshotPlan(archive, destination), overwrite: this.#state.overwrite,
      symlinks: this.#state.symlinks, verifySfx: this.#state.verifySfx };
  }

  runOptionsMatch(snapshot: ExtractRunSnapshot): boolean {
    return snapshot.overwrite === this.#state.overwrite && snapshot.symlinks === this.#state.symlinks
      && snapshot.verifySfx === this.#state.verifySfx;
  }

  extractJob(snapshot: ExtractRunSnapshot, plan: Pick<ExtractPlanPreflightDto, "destination" | "input_guard">): ExtractJob {
    return this.#jobSpec(snapshot.input, snapshot.overwrite, snapshot.symlinks,
      plan.destination, plan.input_guard, snapshot.verifySfx);
  }

  copyOutJob(archive: ArchiveSource, destination: string, selection: string[]): ExtractJob {
    const input = this.#inputSnapshot(archive, destination, [...selection]);
    return this.#jobSpec(input, this.#state.overwrite, this.#state.symlinks, null, null, false);
  }

  #restorePolicies(overwrite: ExtractOverwriteMode, symlinks: ExtractSymlinkMode, encoding: string | null): void {
    this.#state.overwrite = overwrite;
    this.#state.symlinks = symlinks;
    this.#state.presetEncodingLabel = encoding;
  }

  #inputSnapshot(archive: ArchiveSource, destination: string, selection: string[] | null): ExtractPlanInput {
    return { archiveId: archive.id, path: archive.source, displayPath: archive.path, dest: destination,
      selection, smart: this.#state.destinationMode === "smart", encoding: this.encodingForJob(archive.encoding_override ?? null) };
  }

  #jobSpec(input: ExtractPlanInput, overwrite: ExtractOverwriteMode, symlinks: ExtractSymlinkMode,
    expectedDestination: string | null, expectedInputGuard: string | null, verifySfx: boolean): ExtractJob {
    return {
      kind: "extract", path: input.path, dest: input.dest,
      expected_destination: expectedDestination, expected_input_guard: expectedInputGuard,
      selection: input.selection === null ? null : [...input.selection], overwrite, symlinks,
      smart: input.smart, encoding: input.encoding, password: null, verify_sfx: verifySfx, best_effort: false,
    };
  }
}
