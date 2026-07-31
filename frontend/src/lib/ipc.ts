// Typed wrappers over the Tauri command surface. Mirrors
// crates/squallz-gui/src/dto.rs — keep the two files in sync.

import { invoke } from "@tauri-apps/api/core";

export interface ErrorDto {
  key: string;
  params: Record<string, string>;
  detail: string;
}

export interface ArchiveInfo {
  id: number;
  path: string;
  source: string;
  name: string;
  read_only: boolean;
  format: string;
  entry_count: number;
  volumes: string[] | null;
  legacy_encoding_count: number;
  garbled_count: number;
  suggested_encoding: string | null;
  encoding_override: string | null;
}

export interface EntryDto {
  path: string;
  display: string;
  entry_type: "file" | "dir" | "symlink" | "hardlink" | "other";
  size: number;
  compressed: number | null;
  modified: number | null;
  crc: number | null;
  encrypted: boolean;
  encoding: string;
}

export interface Page {
  total: number;
  page: number;
  items: EntryDto[];
}

export interface FormatDto {
  id: string;
  extensions: string[];
  kind: "archive" | "compressor";
  can_create: boolean;
  can_extract: boolean;
  can_encrypt_data: boolean;
  can_encrypt_names: boolean;
  can_split: boolean;
  can_update: boolean;
  can_test: boolean;
}

export interface CreatePlanDto {
  input_count: number;
  entries: number;
  deduplicated_entries: number;
  files: number;
  directories: number;
  symlinks: number;
  total_bytes: number;
  output_budget_bytes: number;
  primary_output: string;
  archive_output_budget_bytes: number;
  final_output_budget_bytes: number;
  split_volume_count_budget: number | null;
  workspace_budget_bytes: number;
  system_temp_budget_bytes: number;
}

export interface ExtractPlanDto {
  requested_destination: string;
  destination: string;
  layout: "direct" | "wrap_in_folder";
  entries: number;
  files: number;
  directories: number;
  symlinks: number;
  hardlinks: number;
  other: number;
  total_bytes: number;
  estimated_conflicts: number;
}

export interface ExtractPlanPreflightDto extends ExtractPlanDto {
  input_guard: string;
  required_free_bytes: number;
  available_bytes: number;
  space_ok: boolean;
}

export interface DiskSpaceDto {
  path: string;
  required_bytes: number;
  available_bytes: number;
  ok: boolean;
}

export interface SfxCreateCapabilityDto {
  target: "macos" | "windows" | "linux";
  extension: "app" | "exe" | "run";
  available: boolean;
  status: "available" | "missing" | "invalid";
  requires_signing: boolean;
}

export interface MacosSfxPublisherStatusDto {
  available: boolean;
  status: "available" | "missing_identity" | "unsupported";
  identities: string[];
}

export interface SourceCleanupRecoveryNotice {
  generation: number;
  status: "restored" | "preserved" | "changed" | "cleared" | "completed_unknown" | "busy" | "needs_attention";
  path: string | null;
  reason: "journal_invalid" | "journal_permission_denied" | "journal_unavailable" | "recovery_failed" | null;
  journal_path: string | null;
}

export type CreateDestinationBase = "ask" | "source_parent" | "default_directory";
export type ExistingOutputPolicy = "ask" | "skip" | "overwrite" | "rename";
export type CreateCompletionAction = "none" | "reveal_output" | "open_in_squallz";
export type PostSuccessAction = "keep_source" | "trash_source";

export interface CreateDestination {
  base: CreateDestinationBase;
  existing_output: ExistingOutputPolicy;
}

export interface NestedArchivePreviewDto {
  outer_path: string;
  entry_path: string;
  format: string;
  entry_count: number;
  truncated: boolean;
  items: EntryDto[];
}

export interface EntryPreviewDto {
  outer_path: string;
  entry_path: string;
  display_name: string;
  preview_id: string;
  size: number;
  archive_like: boolean;
}

export interface CreateDestinationInspectionDto {
  conflict: boolean;
  guard: string | null;
}

export type JobSpec =
  | {
      kind: "compress";
      inputs: string[];
      dest: string;
      level: number;
      password: string | null;
      encrypt_names: boolean;
      split_size: number | null;
      split_mode: "generic" | "native";
      excludes: string[];
      content_policy?: CreateContentPolicy | null;
      sqz_inner_format?: "sqz" | "zip" | "7z" | null;
      sfx_target?: "macos" | "windows" | "linux" | null;
      replace_existing?: boolean;
      replacement_guard?: string | null;
      completion?: CreateCompletionAction | null;
      post_success?: PostSuccessAction | null;
      test_after_create?: boolean | null;
    }
  | {
      kind: "publish_macos_sfx";
      source: string;
      output: string;
      identity: string;
      notary_profile: string;
    }
  | {
      kind: "extract";
      path: string;
      dest: string;
      expected_destination?: string | null;
      expected_input_guard?: string | null;
      selection: string[] | null;
      overwrite: string;
      symlinks: string;
      smart: boolean;
      encoding: string | null;
      password: string | null;
      verify_sfx?: boolean;
      best_effort: boolean;
    }
  | {
      kind: "batch_extract";
      items: Array<{
        path: string;
        dest: string;
        encoding: string | null;
        password: string | null;
        best_effort: boolean;
      }>;
      overwrite: string;
      symlinks: string;
      smart: boolean;
    }
  | {
      kind: "extract_nested";
      outer_path: string;
      entry_path: string;
      dest: string;
      overwrite: string;
      symlinks: string;
	      smart: boolean;
	      encoding: string | null;
	      password: string | null;
	      best_effort: boolean;
	    }
	  | {
	      kind: "test";
      path: string;
      encoding: string | null;
      password: string | null;
    }
  | {
      kind: "convert";
      src: string;
      dest: string;
      level: number;
      src_encoding: string | null;
      src_password: string | null;
      dest_password: string | null;
      encrypt_names: boolean;
      split_size: number | null;
      split_mode: "generic" | "native";
      replace_existing?: boolean;
      replacement_guard?: string | null;
    }
  | {
      kind: "export_sqz";
      src: string;
      dest: string;
      level: number;
      dest_password: string | null;
      replace_existing?: boolean;
      replacement_guard?: string | null;
    }
  | {
      kind: "repair_sqz";
      src: string;
      dest: string;
      level: number;
    }
  | {
      kind: "repair_zip";
      src: string;
      dest: string;
      level: number;
    }
  | {
      kind: "protect";
      path: string;
      redundancy: number;
      recovery: string | null;
    }
  | {
      kind: "verify_recovery";
      path: string;
      recovery: string | null;
    }
  | {
      kind: "repair_recovery";
      path: string;
      output: string | null;
      output_directory?: boolean;
      recovery: string | null;
    }
  | {
      kind: "update";
      path: string;
      add: string[];
      delete: string[];
      rename: Array<{ from: string; to: string }>;
      mkdir?: string[];
      excludes: string[];
      content_policy?: CreateContentPolicy | null;
      password: string | null;
      level: number;
    }
  | {
      kind: "checksum";
      inputs: string[];
      excludes: string[];
      algorithm: string;
    }
  | {
      kind: "checksum_check";
      manifest: string;
      algorithm: string;
    }
  | {
      kind: "duplicate_scan";
      inputs: string[];
      excludes: string[];
      min_size: number;
    };

export type ProgressPhase =
  | "recovery_prepare"
  | "recovery_verify"
  | "recovery_process"
  | "recovery_finalize"
  | "output_recovery"
  | "output_split"
  | "output_verify"
  | "output_commit"
  | "output_cleanup"
  | "update_recovery"
  | "update_rewrite"
  | "update_verify"
  | "update_commit"
  | "update_cleanup"
  | "sfx_publish_verify"
  | "sfx_publish_sign"
  | "sfx_publish_notarize"
  | "sfx_publish_finalize";

export interface ProgressEvent {
  id: number;
  version: number;
  done: number;
  total: number;
  current: string;
  current_done?: number;
  current_total?: number;
  scanned_entries?: number;
  speed: number;
  phase?: ProgressPhase;
  interruptible?: boolean;
}

export interface StateEvent {
  id: number;
  version: number;
  state: "queued" | "running" | "paused" | "done" | "failed" | "cancelled";
  error: ErrorDto | null;
  result?: Record<string, unknown> | null;
}

export type JobOrigin = "app" | "file_manager";
export type JobInteraction = "conflict" | "password";
export type QueueWaitReason = "parallel_limit" | "cpu_budget" | "queue_order";

export interface JobSnapshotProgress {
  done: number;
  total: number;
  current: string;
  current_done: number;
  current_total: number;
  scanned_entries?: number;
  speed: number;
  phase?: ProgressPhase;
  interruptible?: boolean;
}

export interface JobSnapshot {
  id: number;
  version: number;
  spec: JobSpec;
  origin: JobOrigin;
  owned_by_requester: boolean;
  state: StateEvent["state"];
  queue_position: number | null;
  queue_wait_reason: QueueWaitReason | null;
  cpu_threads: number;
  stream_buffer_limit_bytes: number | null;
  progress: JobSnapshotProgress;
  error: ErrorDto | null;
  result: Record<string, unknown> | null;
  interaction: JobInteraction | null;
}

export interface JobSnapshotsDelta {
  revision: number;
  reset: boolean;
  upserts: JobSnapshot[];
  removed: number[];
}

export interface OperationAuditRecord {
  id: number;
  time: number;
  kind: string;
  state: string;
  title: string;
  detail: string;
  result_summary?: string;
  error_key?: string;
}

export interface AskConflictEvent {
  id: number;
  existing_path: string;
  existing_size: number;
  existing_modified: number | null;
  incoming_path: string;
  incoming_size: number;
  incoming_modified: number | null;
}

export interface AskPasswordEvent {
  id: number;
  name: string;
  wrong: boolean;
}

export interface PasswordBookStatus {
  available: boolean;
  saved: boolean;
}

export interface LanguageDto {
  tag: string;
  name: string;
}

export interface SettingsDto {
  theme: string | null;
  language: string | null;
  ui_mode: string | null;
  ui_density: string | null;
  accent_palette: string | null;
  custom_accent: string | null;
  accent_contrast_guard: boolean | null;
  default_create_dir: string | null;
  default_extract_dir: string | null;
  reveal_after_extract: boolean;
  check_updates_automatically: boolean | null;
  safety_max_output_bytes: number | null;
  safety_max_entries: number | null;
  safety_max_compression_ratio: number | null;
  performance_threads: number | null;
  performance_memory_limit_bytes: number | null;
  performance_parallel_jobs: number | null;
}

export type PresetCreateCredential =
  | { kind: "none" }
  | { kind: "prompt" };

export type PresetExtractCredential = { kind: "prompt_when_needed" };

export type PresetVolumeMode =
  | { kind: "single" }
  | { kind: "split"; size_bytes: string };

export type CreateContentPolicy =
  | "cross_platform_clean"
  | "keep_all_files"
  | "custom";

export type PresetCreateOutput =
  | { kind: "archive" }
  | {
      kind: "self_extracting";
      target: "current_platform" | "macos" | "windows" | "linux";
    };

export type PresetFormatOptions =
  | { kind: "none" }
  | { kind: "sqz"; inner_format: "entry_set" | "zip" | "seven_zip" };

export interface CreateArchivePresetOptions {
  format: string;
  level: number;
  credential: PresetCreateCredential;
  encrypt_names: boolean;
  volumes: PresetVolumeMode;
  content_policy: CreateContentPolicy;
  excludes: string[];
  output: PresetCreateOutput;
  format_options: PresetFormatOptions;
  destination: CreateDestination;
  completion: CreateCompletionAction;
  post_success: PostSuccessAction;
  test_after_create: boolean;
}

export interface ExtractArchivePresetOptions {
  destination: {
    base: "archive_parent" | "default_directory" | "ask";
    layout: "direct" | "smart" | "archive_folder";
  };
  existing_output: ExistingOutputPolicy;
  symlinks: "preserve" | "skip" | "follow";
  encoding: { kind: "auto" } | { kind: "named"; label: string };
  credential: PresetExtractCredential;
  post_success: PostSuccessAction;
}

export type NamedArchivePreset =
  | {
      kind: "create";
      id: string;
      label: string;
      built_in: boolean;
      options: CreateArchivePresetOptions;
    }
  | {
      kind: "extract";
      id: string;
      label: string;
      built_in: boolean;
      options: ExtractArchivePresetOptions;
    };

export interface ArchivePresetDocument {
  schema_version: number;
  revision: number;
  presets: NamedArchivePreset[];
  bindings: {
    app_default_create: string | null;
    app_default_extract: string | null;
    file_manager_create: string | null;
    file_manager_extract: string | null;
  };
}

export interface LocaleTable {
  lang: string;
  table: Record<string, string>;
}

export interface OpenFilesEvent {
  paths: string[];
  action?: string | null;
  output?: string | null;
}

export interface IntegrationActionDto {
  id: string;
  name: string;
  kind: string;
  path: string;
  script_path: string;
}

export type IntegrationActionHealthState = "healthy" | "missing" | "damaged";
export type IntegrationHealthState = "healthy" | "needs_repair" | "missing" | "unavailable";

export interface IntegrationActionHealthDto {
  id: string;
  name: string;
  state: IntegrationActionHealthState;
  issue: string | null;
}

export interface IntegrationApplyResultDto {
  platform: string;
  services_dir: string;
  script_dir: string;
  installed: IntegrationActionDto[];
  unsupported: string[];
}

export interface IntegrationStatusDto {
  platform: string;
  services_dir: string;
  script_dir: string;
  health: IntegrationHealthState;
  actions: IntegrationActionHealthDto[];
  can_repair: boolean;
  can_remove: boolean;
  installed: IntegrationActionDto[];
  missing: string[];
  unsupported: string[];
}

export type IntegrationDefaultHandlerState = "squallz" | "other" | "unknown";
export type IntegrationDefaultHandlersState = "squallz" | "mixed" | "other" | "unknown" | "unavailable";

export interface IntegrationDefaultHandlerDto {
  extension: string;
  state: IntegrationDefaultHandlerState;
  application_name: string | null;
}

export interface IntegrationDefaultHandlersDto {
  state: IntegrationDefaultHandlersState;
  total: number;
  checked: number;
  squallz: number;
  handlers: IntegrationDefaultHandlerDto[];
}

export interface IntegrationSystemDiagnosticsDto {
  platform: string;
  backends: Array<{
    id: string;
    available: boolean;
    configured: boolean;
    source: "application" | "environment" | "path" | null;
    tool: string | null;
  }>;
  default_handlers: IntegrationDefaultHandlersDto;
  file_manager_visibility: {
    state: "manual_check" | "unsupported";
    reason: string;
  };
}

export interface IntegrationRemoveResultDto {
  platform: string;
  services_dir: string;
  script_dir: string;
  removed: IntegrationActionDto[];
  missing: string[];
  unsupported: string[];
}

export type AppUpdateStatus = "up_to_date" | "update_available" | "ahead";
export type AppUpdateTrust = "developer_id_notarized" | "unsigned_preview" | "unavailable";
export type AppUpdateMetadataSource =
  | "github_api"
  | "latest_release_redirect"
  | "latest_release_manifest";

export interface AppUpdateCheckDto {
  status: AppUpdateStatus;
  currentVersion: string;
  latestVersion: string;
  releaseName: string;
  releaseUrl: string;
  publishedAt: string;
  platform: string;
  architecture: string;
  assetName: string | null;
  downloadUrl: string | null;
  assetSizeBytes: number | null;
  assetSha256: string | null;
  assetTrust: AppUpdateTrust;
  metadataSource: AppUpdateMetadataSource;
}

let settingsOperationQueue: Promise<void> = Promise.resolve();

function invokeSettingsOperation<T>(command: string, args: Record<string, unknown>): Promise<T> {
  const request = settingsOperationQueue.then(() => invoke<T>(command, args));
  settingsOperationQueue = request.then(
    () => undefined,
    () => undefined,
  );
  return request;
}

export const ipc = {
  openArchive: (
    path: string,
    password: string | null,
    encoding: string | null,
    requestId: string,
  ) =>
    invoke<ArchiveInfo>("open_archive", { path, password, encoding, requestId }),
  cancelArchiveOpen: (requestId: string) =>
    invoke<void>("cancel_archive_open", { requestId }),
  closeArchive: (id: number) => invoke<void>("close_archive", { id }),
  recordValidationEvent: (event: string, payload: Record<string, unknown>) =>
    invoke<void>("record_validation_event", { event, payload }),
  takeValidationDropPaths: () => invoke<string[]>("take_validation_drop_paths"),
  listEntries: (
    id: number,
    page: number,
    dirPrefix: string,
    filter?: string | null,
    pageSize?: number,
  ) =>
    invoke<Page>("list_entries", { id, page, pageSize, dirPrefix, filter }),
  searchEntries: (
    id: number,
    page: number,
    query: string,
    pageSize: number,
    generation: number,
  ) => invoke<Page | null>("search_entries", { id, page, pageSize, query, generation }),
  cancelArchiveSearch: (id: number, generation: number) =>
    invoke<void>("cancel_archive_search", { id, generation }),
  getFormats: () => invoke<FormatDto[]>("get_formats"),
  archiveStem: (path: string) => invoke<string>("archive_stem", { path }),
  planCreate: (spec: JobSpec, requestId: string) =>
    invoke<CreatePlanDto>("plan_create", { spec, requestId }),
  planConvert: (spec: JobSpec, requestId: string) =>
    invoke<CreatePlanDto>("plan_convert", { spec, requestId }),
  cancelConvertPlan: (requestId: string) =>
    invoke<void>("cancel_convert_plan", { requestId }),
  planExtract: (
    path: string,
    displayPath: string,
    dest: string,
    selection: string[] | null,
    smart: boolean,
    encoding: string | null,
    requestId: string,
  ) => invoke<ExtractPlanPreflightDto>("plan_extract", {
    path,
    displayPath,
    dest,
    selection,
    smart,
    encoding,
    requestId,
  }),
  cancelExtractPlan: (requestId: string) =>
    invoke<void>("cancel_extract_plan", { requestId }),
  uniqueCreateDestination: (proposed: string, split: boolean) =>
    invoke<string>("unique_create_destination", { proposed, split }),
  createDestinationHasConflict: (path: string, split: boolean) =>
    invoke<boolean>("create_destination_has_conflict", { proposed: path, split }),
  inspectCreateDestination: (
    path: string,
    split: boolean,
    requestId: string,
    sfxTarget?: string | null,
  ) =>
    invoke<CreateDestinationInspectionDto>("inspect_create_destination", {
      proposed: path,
      split,
      requestId,
      sfxTarget: sfxTarget ?? null,
    }),
  cancelCreateDestinationInspection: (requestId: string) =>
    invoke<void>("cancel_create_destination_inspection", { requestId }),
  checkDiskSpace: (path: string, requiredBytes: number) =>
    invoke<DiskSpaceDto>("check_disk_space", { path, requiredBytes }),
  tempDir: () => invoke<string>("temp_dir"),
  previewNestedArchive: (
    outerPath: string,
    entryPath: string,
    password?: string | null,
    encoding?: string | null,
  ) =>
    invoke<NestedArchivePreviewDto>("preview_nested_archive", {
      outerPath,
      entryPath,
      password,
      encoding,
    }),
  previewArchiveEntry: (
    outerPath: string,
    entryPath: string,
    password?: string | null,
    encoding?: string | null,
  ) =>
    invoke<EntryPreviewDto>("preview_archive_entry", {
      outerPath,
      entryPath,
      password,
      encoding,
    }),
  openPreviewSession: (previewId: string) =>
    invoke<void>("open_preview_session", { previewId }),
  revealPreviewSession: (previewId: string) =>
    invoke<void>("reveal_preview_session", { previewId }),
  releasePreviewSession: (previewId: string) =>
    invoke<boolean>("release_preview_session", { previewId }),
  openNestedArchive: (
    outerPath: string,
    entryPath: string,
    password?: string | null,
    encoding?: string | null,
  ) =>
    invoke<ArchiveInfo>("open_nested_archive", {
      outerPath,
      entryPath,
      password,
      encoding,
    }),
  submitJob: (spec: JobSpec) => invoke<number>("submit_job", { spec }),
  getSfxCreateCapability: () =>
    invoke<SfxCreateCapabilityDto>("get_sfx_create_capability"),
  getMacosSfxPublisherStatus: () =>
    invoke<MacosSfxPublisherStatusDto>("get_macos_sfx_publisher_status"),
  jobSnapshot: (id: number) => invoke<JobSnapshot | null>("job_snapshot", { id }),
  jobSnapshots: (since: number | null) =>
    invoke<JobSnapshotsDelta>("job_snapshots", { since }),
  dismissJobSnapshots: (ids: number[]) =>
    invoke<void>("dismiss_job_snapshots", { ids }),
  getSourceCleanupRecovery: () =>
    invoke<SourceCleanupRecoveryNotice | null>("get_source_cleanup_recovery"),
  pauseJob: (id: number) => invoke<void>("pause_job", { id }),
  resumeJob: (id: number) => invoke<void>("resume_job", { id }),
  moveJobEarlier: (id: number) => invoke<void>("move_job_earlier", { id }),
  moveJobLater: (id: number) => invoke<void>("move_job_later", { id }),
  moveJobBefore: (id: number, beforeId: number | null) =>
    invoke<void>("move_job_before", { id, beforeId }),
  cancelJob: (id: number) => invoke<void>("cancel_job", { id }),
  answerConflict: (id: number, decision: string, applyAll: boolean) =>
    invoke<void>("answer_conflict", { id, decision, applyAll }),
  answerPassword: (id: number, password: string | null) =>
    invoke<void>("answer_password", { id, password }),
  archivePasswordStatus: (path: string) =>
    invoke<PasswordBookStatus>("archive_password_status", { path }),
  rememberArchivePassword: (path: string, password: string, encoding?: string | null) =>
    invoke<PasswordBookStatus>("remember_archive_password", { path, password, encoding }),
  forgetArchivePassword: (path: string) =>
    invoke<PasswordBookStatus>("forget_archive_password", { path }),
  isValidationSession: () => invoke<boolean>("is_validation_session"),
  platformKind: () => invoke<"macos" | "windows" | "linux">("platform_kind"),
  takeOpenFiles: () => invoke<OpenFilesEvent>("take_open_files"),
  openFileListenerReady: () => invoke<OpenFilesEvent>("open_file_listener_ready"),
  applyIntegrationChanges: () =>
    invoke<IntegrationApplyResultDto>("apply_integration_changes"),
  getIntegrationStatus: () =>
    invoke<IntegrationStatusDto>("get_integration_status"),
  getSystemIntegrationDiagnostics: () =>
    invoke<IntegrationSystemDiagnosticsDto>("get_system_integration_diagnostics"),
  removeIntegrationChanges: () =>
    invoke<IntegrationRemoveResultDto>("remove_integration_changes"),
  checkForUpdates: () => invoke<AppUpdateCheckDto>("check_for_updates"),
  getLocaleTable: (lang?: string | null) =>
    invoke<LocaleTable>("get_locale_table", { lang }),
  listLanguages: () => invoke<LanguageDto[]>("list_languages"),
  getSettings: () => invokeSettingsOperation<SettingsDto>("get_settings", {}),
  getArchivePresets: () => invoke<ArchivePresetDocument>("get_archive_presets"),
  saveArchivePresets: (expectedRevision: number, document: ArchivePresetDocument) =>
    invoke<ArchivePresetDocument>("save_archive_presets", { expectedRevision, document }),
  resolveExternalTaskJob: (
    action: string,
    paths: string[],
    output: string | null,
    checksumAlgorithm: string,
    checksumExcludes: string[],
  ) =>
    invoke<JobSpec | null>("resolve_external_task_job", {
      action,
      paths,
      output,
      checksumAlgorithm,
      checksumExcludes,
    }),
  setTheme: (theme: string) => invokeSettingsOperation<SettingsDto>("set_theme", { theme }),
  setLanguage: (language: string | null) =>
    invokeSettingsOperation<SettingsDto>("set_language", { language }),
  setGeneralOptions: (
    language: string | null,
    defaultCreateDir: string | null,
    defaultExtractDir: string | null,
    revealAfterExtract: boolean,
    checkUpdatesAutomatically: boolean,
  ) =>
    invokeSettingsOperation<SettingsDto>("set_general_options", {
      language,
      defaultCreateDir,
      defaultExtractDir,
      revealAfterExtract,
      checkUpdatesAutomatically,
    }),
  setUiMode: (uiMode: string) =>
    invokeSettingsOperation<SettingsDto>("set_ui_mode", { uiMode }),
  setUiDensity: (uiDensity: string) =>
    invokeSettingsOperation<SettingsDto>("set_ui_density", { uiDensity }),
  setAccentPalette: (
    accentPalette: string,
    customAccent?: string | null,
    accentContrastGuard?: boolean | null,
  ) =>
    invokeSettingsOperation<SettingsDto>("set_accent_palette", {
      accentPalette,
      customAccent,
      accentContrastGuard,
    }),
  exportOperationHistory: (path: string, contents: string) =>
    invoke<void>("export_operation_history", { path, contents }),
  getOperationAudit: (limit?: number | null) =>
    invoke<OperationAuditRecord[]>("get_operation_audit", { limit }),
  exportOperationAudit: (path: string) =>
    invoke<void>("export_operation_audit", { path }),
  setSafetyLimits: (
    maxOutputBytes: number | null,
    maxEntries: number | null,
    maxCompressionRatio: number | null,
  ) =>
    invokeSettingsOperation<SettingsDto>("set_safety_limits", {
      maxOutputBytes,
      maxEntries,
      maxCompressionRatio,
    }),
  setPerformanceOptions: (
    threads: number | null,
    memoryLimitBytes: number | null,
    parallelJobs: number | null,
  ) =>
    invokeSettingsOperation<SettingsDto>("set_performance_options", {
      threads,
      memoryLimitBytes,
      parallelJobs,
    }),
};

/** Type guard for structured backend errors. */
export function isErrorDto(e: unknown): e is ErrorDto {
  return (
    typeof e === "object" && e !== null && "key" in e && "params" in e
  );
}
