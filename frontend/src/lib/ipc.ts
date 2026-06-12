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
  name: string;
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

export interface CreateEstimateDto {
  input_count: number;
  entries: number;
  files: number;
  directories: number;
  symlinks: number;
  total_bytes: number;
  output_budget_bytes: number;
}

export interface DiskSpaceDto {
  path: string;
  required_bytes: number;
  available_bytes: number;
  ok: boolean;
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
  temp_path: string;
  size: number;
  archive_like: boolean;
  preview_mime: string | null;
  preview_data_url: string | null;
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
      excludes: string[];
    }
  | {
      kind: "extract";
      path: string;
      dest: string;
      selection: string[] | null;
      overwrite: string;
      symlinks: string;
      smart: boolean;
      encoding: string | null;
      password: string | null;
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
    }
  | {
      kind: "export_sqz";
      src: string;
      dest: string;
      level: number;
      dest_password: string | null;
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

export interface ProgressEvent {
  id: number;
  done: number;
  total: number;
  current: string;
  current_done?: number;
  current_total?: number;
  speed: number;
}

export interface StateEvent {
  id: number;
  state: "queued" | "running" | "paused" | "done" | "failed" | "cancelled";
  error: ErrorDto | null;
  result?: Record<string, unknown> | null;
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
  default_extract_dir: string | null;
  reveal_after_extract: boolean;
  safety_max_output_bytes: number | null;
  safety_max_entries: number | null;
  safety_max_compression_ratio: number | null;
  performance_threads: number | null;
  performance_memory_limit_bytes: number | null;
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
  installed: IntegrationActionDto[];
  missing: string[];
  unsupported: string[];
}

export interface IntegrationRemoveResultDto {
  platform: string;
  services_dir: string;
  script_dir: string;
  removed: IntegrationActionDto[];
  missing: string[];
  unsupported: string[];
}

export const ipc = {
  openArchive: (path: string, password?: string | null, encoding?: string | null) =>
    invoke<ArchiveInfo>("open_archive", { path, password, encoding }),
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
  getFormats: () => invoke<FormatDto[]>("get_formats"),
  archiveStem: (path: string) => invoke<string>("archive_stem", { path }),
  estimateCreateInputs: (inputs: string[], excludes: string[]) =>
    invoke<CreateEstimateDto>("estimate_create_inputs", { inputs, excludes }),
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
  openPreviewPath: (path: string) => invoke<void>("open_preview_path", { path }),
  revealPreviewPath: (path: string) => invoke<void>("reveal_preview_path", { path }),
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
  pauseJob: (id: number) => invoke<void>("pause_job", { id }),
  resumeJob: (id: number) => invoke<void>("resume_job", { id }),
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
  removeIntegrationChanges: () =>
    invoke<IntegrationRemoveResultDto>("remove_integration_changes"),
  getLocaleTable: (lang?: string | null) =>
    invoke<LocaleTable>("get_locale_table", { lang }),
  listLanguages: () => invoke<LanguageDto[]>("list_languages"),
  getSettings: () => invoke<SettingsDto>("get_settings"),
  setTheme: (theme: string) => invoke<SettingsDto>("set_theme", { theme }),
  setLanguage: (language: string | null) =>
    invoke<SettingsDto>("set_language", { language }),
  setGeneralOptions: (
    language: string | null,
    defaultExtractDir: string | null,
    revealAfterExtract: boolean,
  ) =>
    invoke<SettingsDto>("set_general_options", {
      language,
      defaultExtractDir,
      revealAfterExtract,
    }),
  setUiMode: (uiMode: string) =>
    invoke<SettingsDto>("set_ui_mode", { uiMode }),
  setUiDensity: (uiDensity: string) =>
    invoke<SettingsDto>("set_ui_density", { uiDensity }),
  setAccentPalette: (
    accentPalette: string,
    customAccent?: string | null,
    accentContrastGuard?: boolean | null,
  ) =>
    invoke<SettingsDto>("set_accent_palette", {
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
    invoke<SettingsDto>("set_safety_limits", {
      maxOutputBytes,
      maxEntries,
      maxCompressionRatio,
    }),
  setPerformanceOptions: (threads: number | null, memoryLimitBytes: number | null) =>
    invoke<SettingsDto>("set_performance_options", { threads, memoryLimitBytes }),
};

/** Type guard for structured backend errors. */
export function isErrorDto(e: unknown): e is ErrorDto {
  return (
    typeof e === "object" && e !== null && "key" in e && "params" in e
  );
}
