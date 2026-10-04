import { normalizeDesktopFolder, type DesktopPathPlatform } from "./desktop-path";
import { loadLocale } from "./i18n.svelte";
import { ipc, isErrorDto, type SettingsDto } from "./ipc";
import { normalizeHexColor } from "./theme";
import { defaultCustomAccent, paletteIds, type NumericSetting, type PaletteId } from "./ui-model";

export type PersistedSettingsSection = "general" | "security" | "performance" | "colors";
export type SettingsSaveState = "saved" | "dirty" | "saving" | "session" | "error";
type SaveOutcome = "idle" | "saved" | "session" | "error";
type DraftGenerations = Readonly<Record<PersistedSettingsSection, number>>;
type GeneralSettings = {
  language: string;
  defaultCreateDir: string;
  defaultExtractDir: string;
  revealAfterExtract: boolean;
  automaticUpdateChecks: boolean;
};
type SecuritySettings = {
  maxOutputGiB: NumericSetting;
  maxEntries: NumericSetting;
  maxCompressionRatio: NumericSetting;
};
type PerformanceSettings = {
  parallelJobs: NumericSetting;
  threads: NumericSetting;
  memoryKiB: NumericSetting;
};
type ColorSettings = {
  palette: PaletteId;
  accent: string;
  input: string;
  saveError: boolean;
  contrastGuard: boolean;
};

export type SettingsEffect =
  | { kind: "notice"; message: string }
  | { kind: "generalApplied"; revealAfterExtract: boolean }
  | { kind: "previewLanguage"; language: string | null }
  | { kind: "generalSaved"; settings: SettingsDto }
  | { kind: "automaticUpdates"; enabled: boolean }
  | { kind: "palettePreviewChanged" };

type SettingsHost = {
  platform: () => DesktopPathPlatform;
  tr: (key: string, fallback: string) => string;
  emit: (effect: SettingsEffect) => void;
};
type InitialSettings = { paletteOverride?: PaletteId | null; defaultExtractDir?: string | null };

const bytesPerKiB = 1024;
const bytesPerGiB = 1024 ** 3;
const defaultSecurity = { maxOutputGiB: 256, maxEntries: 1_000_000, maxCompressionRatio: 2048 };
const defaultPerformance: PerformanceSettings = { parallelJobs: null, threads: null, memoryKiB: null };
const numberFormatter = new Intl.NumberFormat("en-US");

export function isSettingsPersistenceFailure(error: unknown): boolean {
  return isErrorDto(error) && error.key === "error.settings_write";
}

function sameGeneral(left: GeneralSettings, right: GeneralSettings): boolean {
  return left.language === right.language && left.defaultCreateDir === right.defaultCreateDir
    && left.defaultExtractDir === right.defaultExtractDir && left.revealAfterExtract === right.revealAfterExtract
    && left.automaticUpdateChecks === right.automaticUpdateChecks;
}

function wholeSetting(value: NumericSetting, fallback: number, min: number, max: number): number {
  const number = typeof value === "number" && Number.isFinite(value) ? value : fallback;
  return Math.min(max, Math.max(min, Math.round(number)));
}

/** Owns explicit settings drafts and saves; the host owns desktop and application effects. */
export class SettingsSession {
  #general = $state<GeneralSettings>({
    language: "", defaultCreateDir: "", defaultExtractDir: "", revealAfterExtract: false, automaticUpdateChecks: true,
  });
  #savedGeneral = $state<GeneralSettings>({ ...this.#general });
  #appliedGeneral = $state<GeneralSettings>({ ...this.#general });
  #security = $state<SecuritySettings>({ ...defaultSecurity });
  #savedSecurity = $state<SecuritySettings>({ ...defaultSecurity });
  #savedSecurityCustom = $state(false);
  #performance = $state<PerformanceSettings>({ ...defaultPerformance });
  #savedPerformance = $state<PerformanceSettings>({ ...defaultPerformance });
  #colors = $state<ColorSettings>({
    palette: "aqua", accent: defaultCustomAccent, input: defaultCustomAccent, saveError: false, contrastGuard: true,
  });
  #savedColors = $state({ palette: "aqua" as PaletteId, accent: defaultCustomAccent, contrastGuard: true });
  #saveTarget = $state<PersistedSettingsSection | null>(null);
  #outcomes = $state<Record<PersistedSettingsSection, SaveOutcome>>({
    general: "saved", security: "saved", performance: "saved", colors: "saved",
  });
  #generations: Record<PersistedSettingsSection, number> = { general: 0, security: 0, performance: 0, colors: 0 };
  #snapshotLabel = $state("");

  constructor(private readonly host: SettingsHost, private readonly initial: InitialSettings = {}) {
    const folder = initial.defaultExtractDir?.trim() ?? "";
    this.#general.defaultExtractDir = folder;
    this.#savedGeneral.defaultExtractDir = folder;
    this.#appliedGeneral.defaultExtractDir = folder;
    this.#colors.palette = initial.paletteOverride ?? "aqua";
    this.setDefaultsSnapshotLabel();
  }

  get general(): Readonly<GeneralSettings> { return this.#general; }
  get savedGeneral(): Readonly<GeneralSettings> { return this.#savedGeneral; }
  get appliedGeneral(): Readonly<GeneralSettings> { return this.#appliedGeneral; }
  get security(): Readonly<SecuritySettings> { return this.#security; }
  get performance(): Readonly<PerformanceSettings> { return this.#performance; }
  get colors(): Readonly<ColorSettings> { return this.#colors; }
  get saveTarget() { return this.#saveTarget; }
  get snapshotLabel() { return this.#snapshotLabel; }

  get generalDirty(): boolean {
    const draft = this.normalizedGeneral();
    return this.generalValidationError !== "" || !sameGeneral(draft, this.#savedGeneral)
      || !sameGeneral(draft, this.#appliedGeneral);
  }
  get securityDirty(): boolean {
    return this.#security.maxOutputGiB !== this.#savedSecurity.maxOutputGiB
      || this.#security.maxEntries !== this.#savedSecurity.maxEntries
      || this.#security.maxCompressionRatio !== this.#savedSecurity.maxCompressionRatio;
  }
  get performanceDirty(): boolean {
    return this.#performance.parallelJobs !== this.#savedPerformance.parallelJobs
      || this.#performance.threads !== this.#savedPerformance.threads
      || this.#performance.memoryKiB !== this.#savedPerformance.memoryKiB;
  }
  get colorsDirty(): boolean {
    return this.#colors.palette !== this.#savedColors.palette
      || this.#colors.input.trim().toUpperCase() !== this.#savedColors.accent
      || this.#colors.contrastGuard !== this.#savedColors.contrastGuard;
  }
  get generalSaveState() { return this.saveState("general", this.generalDirty); }
  get securitySaveState() { return this.saveState("security", this.securityDirty); }
  get performanceSaveState() { return this.saveState("performance", this.performanceDirty); }
  get colorsSaveState() { return this.saveState("colors", this.colorsDirty); }

  get defaultCreateFolderError(): string {
    return this.folderError(this.#general.defaultCreateDir, this.tr("gui.settings.folder.default_create", "Default create folder"));
  }
  get defaultExtractFolderError(): string {
    return this.folderError(this.#general.defaultExtractDir, this.tr("gui.settings.folder.default_extract", "Default extract folder"));
  }
  get generalValidationError() { return this.defaultCreateFolderError || this.defaultExtractFolderError; }
  get securityOutputError(): string {
    return this.numberError(this.#security.maxOutputGiB, 1, 8192, this.tr("gui.settings.security.max_output_gib", "Max output GiB"));
  }
  get securityEntriesError(): string {
    return this.numberError(this.#security.maxEntries, 1, 10_000_000, this.tr("gui.settings.security.max_entries", "Max entries"));
  }
  get securityRatioError(): string {
    return this.numberError(this.#security.maxCompressionRatio, 1, 100_000, this.tr("gui.settings.security.ratio_guard", "Ratio guard"));
  }
  get securityValidationError() { return this.securityOutputError || this.securityEntriesError || this.securityRatioError; }
  get performanceParallelJobsError(): string {
    return this.optionalNumberError(this.#performance.parallelJobs, 1, 8, this.tr("gui.settings.performance.custom_parallel_jobs", "Custom parallel tasks"));
  }
  get performanceThreadsError(): string {
    return this.optionalNumberError(this.#performance.threads, 1, 64, this.tr("gui.settings.performance.custom_threads", "Custom threads"));
  }
  get performanceMemoryError(): string {
    return this.optionalNumberError(this.#performance.memoryKiB, 8, 64, this.tr("gui.settings.performance.custom_buffer_kib", "Custom buffer KiB"));
  }
  get performanceValidationError() { return this.performanceParallelJobsError || this.performanceThreadsError || this.performanceMemoryError; }
  get customAccentValid() { return normalizeHexColor(this.#colors.input) !== null; }
  get paletteApplyBlocked() { return this.#colors.palette === "custom" && !this.customAccentValid; }

  setGeneral = <K extends keyof GeneralSettings>(field: K, value: GeneralSettings[K]): void => {
    this.#general[field] = value;
    this.markDraft("general");
  };
  setSecurity = <K extends keyof SecuritySettings>(field: K, value: SecuritySettings[K]): void => {
    this.#security[field] = value;
    this.markDraft("security");
  };
  setPerformance = <K extends keyof PerformanceSettings>(field: K, value: PerformanceSettings[K]): void => {
    this.#performance[field] = value;
    this.markDraft("performance");
  };
  resetSecurity = (): void => {
    this.#security = { ...defaultSecurity };
    this.markDraft("security");
  };
  resetPerformance = (): void => {
    this.#performance = { ...defaultPerformance };
    this.markDraft("performance");
  };
  setPalette = (palette: PaletteId): void => {
    this.#colors.palette = palette;
    if (palette === "custom") this.#colors.input = normalizeHexColor(this.#colors.input) ?? this.#colors.accent;
    this.#colors.saveError = false;
    this.markDraft("colors");
    this.host.emit({ kind: "palettePreviewChanged" });
  };
  updateCustomAccent = (value: string, source: "color" | "hex"): void => {
    this.#colors.palette = "custom";
    const normalized = normalizeHexColor(value);
    if (normalized) {
      this.#colors.accent = normalized;
      this.#colors.input = normalized;
      this.#colors.saveError = false;
    } else if (source === "hex") {
      this.#colors.input = value.trim().toUpperCase();
      this.#colors.saveError = false;
    }
    this.markDraft("colors");
    this.host.emit({ kind: "palettePreviewChanged" });
  };
  setAccentContrastGuard = (enabled: boolean): void => {
    this.#colors.contrastGuard = enabled;
    this.markDraft("colors");
  };

  captureGenerations(): DraftGenerations { return { ...this.#generations }; }
  isDraftCurrent(section: PersistedSettingsSection, generation: number): boolean {
    return this.#generations[section] === generation;
  }

  applySnapshot(settings: SettingsDto, requested: DraftGenerations): void {
    this.applyColors(settings, !this.isDraftCurrent("colors", requested.colors));
    this.applyGeneral(settings, !this.isDraftCurrent("general", requested.general));
    this.applySecurity(settings, !this.isDraftCurrent("security", requested.security));
    this.applyPerformance(settings, !this.isDraftCurrent("performance", requested.performance));
    this.updateSnapshotLabel();
  }

  applyPreviewLanguage(language: string | null, requested: DraftGenerations): void {
    const value = language ?? "";
    this.#savedGeneral.language = value;
    this.#appliedGeneral.language = value;
    if (this.isDraftCurrent("general", requested.general)) this.#general.language = value;
  }

  saveSecurity = async (): Promise<void> => {
    if (this.rejectInvalid(this.securityValidationError)) return;
    const { maxOutputGiB, maxEntries, maxCompressionRatio } = this.#security;
    const generation = this.beginSave("security");
    if (generation === null) return;
    const useDefaults = maxOutputGiB === defaultSecurity.maxOutputGiB && maxEntries === defaultSecurity.maxEntries
      && maxCompressionRatio === defaultSecurity.maxCompressionRatio;
    try {
      const settings = await ipc.setSafetyLimits(
        useDefaults ? null : maxOutputGiB! * bytesPerGiB,
        useDefaults ? null : maxEntries,
        useDefaults ? null : maxCompressionRatio,
      );
      this.applySecurity(settings, !this.isDraftCurrent("security", generation));
      this.updateSnapshotLabel();
      this.finishSave("security", generation, "saved");
      this.notice(this.tr("gui.settings.security.saved", "Security settings saved"));
    } catch (error) {
      this.finishSave("security", generation, "error");
      this.notice(isSettingsPersistenceFailure(error) ? this.persistenceFailureLabel() : this.applyFailureLabel());
    }
  };

  savePerformance = async (): Promise<void> => {
    if (this.rejectInvalid(this.performanceValidationError)) return;
    const { parallelJobs, threads, memoryKiB } = this.#performance;
    const generation = this.beginSave("performance");
    if (generation === null) return;
    try {
      const settings = await ipc.setPerformanceOptions(threads, memoryKiB === null ? null : memoryKiB * bytesPerKiB, parallelJobs);
      this.applyPerformance(settings, !this.isDraftCurrent("performance", generation));
      this.updateSnapshotLabel();
      this.finishSave("performance", generation, "saved");
      this.notice(this.tr("gui.settings.performance.saved", "Performance settings saved"));
    } catch (error) {
      this.finishSave("performance", generation, "error");
      this.notice(isSettingsPersistenceFailure(error) ? this.persistenceFailureLabel() : this.applyFailureLabel());
    }
  };

  saveColors = async (): Promise<void> => {
    const accent = this.#colors.palette === "custom" ? normalizeHexColor(this.#colors.input)
      : normalizeHexColor(this.#colors.accent) ?? defaultCustomAccent;
    if (!accent) {
      this.#colors.saveError = true;
      this.notice(this.tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color"));
      return;
    }
    this.#colors.accent = accent;
    this.#colors.input = accent;
    this.#colors.saveError = false;
    const { palette, contrastGuard } = this.#colors;
    const generation = this.beginSave("colors");
    if (generation === null) return;
    try {
      const settings = await ipc.setAccentPalette(palette, accent, contrastGuard);
      this.applyColors(settings, !this.isDraftCurrent("colors", generation));
      this.finishSave("colors", generation, "saved");
      this.host.emit({ kind: "palettePreviewChanged" });
      this.notice(this.tr("gui.colors.saved", "Theme colors saved"));
    } catch (error) {
      const persistenceFailed = isSettingsPersistenceFailure(error);
      this.finishSave("colors", generation, persistenceFailed ? "error" : "session");
      this.notice(persistenceFailed ? this.persistenceFailureLabel()
        : this.tr("gui.colors.saved_preview", "Theme colors apply to this session but were not saved"));
    }
  };

  saveGeneral = async (): Promise<void> => {
    if (this.rejectInvalid(this.generalValidationError)) return;
    const next = this.normalizedGeneral();
    const previousLanguage = this.#appliedGeneral.language;
    const requiresPersistence = !sameGeneral(next, this.#savedGeneral);
    const generation = this.beginSave("general");
    if (generation === null) return;
    try {
      const settings = await ipc.setGeneralOptions(next.language || null, next.defaultCreateDir || null,
        next.defaultExtractDir || null, next.revealAfterExtract, next.automaticUpdateChecks);
      this.host.emit({ kind: "previewLanguage", language: settings.language });
      this.applyGeneral(settings, !this.isDraftCurrent("general", generation));
      await loadLocale(settings.language).catch(() => undefined);
      this.updateSnapshotLabel();
      this.finishSave("general", generation, "saved");
      this.host.emit({ kind: "generalSaved", settings });
      this.notice(this.tr("gui.settings.general.saved", "General settings saved"));
      this.host.emit({ kind: "automaticUpdates", enabled: settings.check_updates_automatically !== false });
    } catch (error) {
      if (isSettingsPersistenceFailure(error)) {
        this.finishSave("general", generation, "error");
        this.notice(this.persistenceFailureLabel());
        return;
      }
      let sessionApplied = false;
      if (this.isDraftCurrent("general", generation)) {
        await loadLocale(next.language || null).catch(() => undefined);
        if (this.isDraftCurrent("general", generation)) {
          this.#appliedGeneral = next;
          this.host.emit({ kind: "previewLanguage", language: next.language || null });
          this.host.emit({ kind: "generalApplied", revealAfterExtract: next.revealAfterExtract });
          this.updateSnapshotLabel();
          sessionApplied = true;
          this.host.emit({ kind: "automaticUpdates", enabled: next.automaticUpdateChecks });
        } else {
          await loadLocale(previousLanguage || null).catch(() => undefined);
          this.updateSnapshotLabel();
        }
      }
      this.finishSave("general", generation, sessionApplied ? requiresPersistence ? "session" : "saved" : "error");
      this.notice(sessionApplied
        ? requiresPersistence
          ? this.tr("gui.settings.general.saved_preview", "General changes apply to this session but were not saved")
          : this.tr("gui.settings.general.matches_saved", "General settings now match the saved values")
        : this.tr("gui.settings.previous_apply_failed", "Earlier changes were not applied. Review the current draft and save again."));
    }
  };

  setDefaultsSnapshotLabel(): void {
    this.#snapshotLabel = this.tr("gui.settings.snapshot.defaults_active", "Saved settings · defaults");
  }

  updateSnapshotLabel(): void {
    const { parallelJobs, threads, memoryKiB } = this.#savedPerformance;
    const parallel = parallelJobs === null ? this.tr("gui.settings.snapshot.parallel_auto", "parallel auto")
      : this.tr("gui.settings.snapshot.parallel_count", "{count} parallel").replace("{count}", String(parallelJobs));
    const workers = threads === null ? this.tr("gui.settings.snapshot.workers_auto", "encoder threads auto")
      : this.tr("gui.settings.snapshot.workers_count", "{count} encoder threads").replace("{count}", String(threads));
    const memory = memoryKiB === null ? this.tr("gui.settings.snapshot.buffer_auto", "buffer auto")
      : this.tr("gui.settings.snapshot.buffer_kib", "{count} KiB buffer")
        .replace("{count}", numberFormatter.format(wholeSetting(memoryKiB, 64, 1, Number.MAX_SAFE_INTEGER)));
    this.#snapshotLabel = this.tr("gui.settings.snapshot.summary", "Saved settings · {safety} · {parallel} · {workers} · {buffer}")
      .replace("{safety}", this.#savedSecurityCustom
        ? this.tr("gui.settings.snapshot.custom_safety", "Custom safety")
        : this.tr("gui.settings.snapshot.default_safety", "Default safety"))
      .replace("{parallel}", parallel).replace("{workers}", workers).replace("{buffer}", memory);
  }

  persistenceFailureLabel(): string {
    return this.tr("gui.settings.save_failed", "Could not save settings. Check disk access and try again.");
  }

  private applyGeneral(settings: SettingsDto, preserveDraft: boolean): void {
    const values: GeneralSettings = {
      language: settings.language ?? "",
      defaultCreateDir: this.normalizeFolder(settings.default_create_dir ?? "") ?? "",
      defaultExtractDir: this.normalizeFolder(settings.default_extract_dir ?? "") ?? "",
      revealAfterExtract: settings.reveal_after_extract === true,
      automaticUpdateChecks: settings.check_updates_automatically !== false,
    };
    this.#savedGeneral = { ...values };
    this.#appliedGeneral = { ...values };
    this.host.emit({ kind: "generalApplied", revealAfterExtract: values.revealAfterExtract });
    if (preserveDraft) return;
    this.#general = values;
    this.#outcomes.general = "saved";
  }

  private applySecurity(settings: SettingsDto, preserveDraft: boolean): void {
    const values: SecuritySettings = {
      maxOutputGiB: settings.safety_max_output_bytes && settings.safety_max_output_bytes > 0
        ? Math.max(1, Math.round(settings.safety_max_output_bytes / bytesPerGiB)) : defaultSecurity.maxOutputGiB,
      maxEntries: settings.safety_max_entries && settings.safety_max_entries > 0
        ? settings.safety_max_entries : defaultSecurity.maxEntries,
      maxCompressionRatio: settings.safety_max_compression_ratio && settings.safety_max_compression_ratio > 0
        ? settings.safety_max_compression_ratio : defaultSecurity.maxCompressionRatio,
    };
    this.#savedSecurity = { ...values };
    this.#savedSecurityCustom = Boolean(settings.safety_max_output_bytes || settings.safety_max_entries || settings.safety_max_compression_ratio);
    if (preserveDraft) return;
    this.#security = values;
    this.#outcomes.security = "saved";
  }

  private applyPerformance(settings: SettingsDto, preserveDraft: boolean): void {
    const values: PerformanceSettings = {
      parallelJobs: settings.performance_parallel_jobs && settings.performance_parallel_jobs > 0
        ? Math.min(settings.performance_parallel_jobs, 8) : null,
      threads: settings.performance_threads && settings.performance_threads > 0 ? Math.min(settings.performance_threads, 64) : null,
      memoryKiB: settings.performance_memory_limit_bytes && settings.performance_memory_limit_bytes > 0
        ? wholeSetting(Math.round(settings.performance_memory_limit_bytes / bytesPerKiB), 64, 8, 64) : null,
    };
    this.#savedPerformance = { ...values };
    if (preserveDraft) return;
    this.#performance = values;
    this.#outcomes.performance = "saved";
  }

  private applyColors(settings: SettingsDto, preserveDraft: boolean): void {
    const palette = paletteIds.includes(settings.accent_palette as PaletteId) ? settings.accent_palette as PaletteId : "aqua";
    const accent = normalizeHexColor(settings.custom_accent) ?? defaultCustomAccent;
    const contrastGuard = settings.accent_contrast_guard !== false;
    this.#savedColors = { palette, accent, contrastGuard };
    if (preserveDraft) return;
    this.#colors = {
      palette: this.initial.paletteOverride ? this.#colors.palette : palette,
      accent,
      input: accent,
      saveError: false,
      contrastGuard,
    };
    this.#outcomes.colors = "saved";
  }

  private normalizedGeneral(): GeneralSettings {
    return { ...this.#general, language: this.#general.language.trim(),
      defaultCreateDir: this.normalizeFolder(this.#general.defaultCreateDir) ?? "",
      defaultExtractDir: this.normalizeFolder(this.#general.defaultExtractDir) ?? "" };
  }

  private normalizeFolder(value: string): string | null { return normalizeDesktopFolder(value, this.host.platform()); }
  private folderError(value: string, label: string): string {
    return !value.trim() || this.normalizeFolder(value) ? ""
      : this.tr("gui.settings.folder.absolute_required", "{name} must be an absolute folder path. Choose a folder or clear the field.")
        .replace("{name}", label);
  }

  private numberError(value: NumericSetting, min: number, max: number, label: string): string {
    return typeof value === "number" && Number.isFinite(value) && Number.isInteger(value) && value >= min && value <= max ? ""
      : this.tr("gui.settings.number.invalid_range", "{label} must be a whole number from {min} to {max}")
        .replace("{label}", label).replace("{min}", numberFormatter.format(min)).replace("{max}", numberFormatter.format(max));
  }
  private optionalNumberError(value: NumericSetting, min: number, max: number, label: string): string {
    return value === null ? "" : this.numberError(value, min, max, label);
  }
  private rejectInvalid(message: string): boolean {
    if (!message) return false;
    this.notice(message);
    return true;
  }
  private markDraft(section: PersistedSettingsSection): void {
    this.#generations[section] += 1;
    this.#outcomes[section] = "idle";
  }
  private beginSave(section: PersistedSettingsSection): number | null {
    if (this.#saveTarget !== null) return null;
    this.#saveTarget = section;
    this.#outcomes[section] = "idle";
    return this.#generations[section];
  }
  private finishSave(section: PersistedSettingsSection, generation: number, outcome: SaveOutcome): void {
    if (this.#saveTarget === section) this.#saveTarget = null;
    this.#outcomes[section] = this.isDraftCurrent(section, generation) || outcome !== "saved" ? outcome : "idle";
  }
  private saveState(section: PersistedSettingsSection, dirty: boolean): SettingsSaveState {
    if (this.#saveTarget === section) return "saving";
    const outcome = this.#outcomes[section];
    if (dirty && (outcome === "error" || outcome === "session")) return outcome;
    return dirty ? "dirty" : "saved";
  }
  private applyFailureLabel(): string {
    return this.tr("gui.settings.apply_failed", "Could not apply these settings. Try again from the desktop app.");
  }
  private notice(message: string): void { this.host.emit({ kind: "notice", message }); }
  private tr(key: string, fallback: string): string { return this.host.tr(key, fallback); }
}
