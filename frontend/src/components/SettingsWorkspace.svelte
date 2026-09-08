<script lang="ts" module>
  import type { CssVariableMap } from "../lib/css-variables";
  import type { UpdateCheckPreview } from "../lib/app-update.svelte";
  import type {
    FormatDto,
    IntegrationStatusDto,
    IntegrationSystemDiagnosticsDto,
    LanguageDto,
  } from "../lib/ipc";
  import type {
    DensityChoice,
    NumericSetting,
    Palette,
    PaletteId,
    ResolvedTheme,
    Screen,
  } from "../lib/ui-model";
  import type { UiMode } from "../lib/uiMode.svelte";

  export type SettingsScreen = Extract<
    Screen,
    | "appearance"
    | "colors"
    | "settingsGeneral"
    | "settingsSecurity"
    | "settingsPerformance"
    | "passwordBook"
    | "integration"
  >;

  export type SettingsSaveState = "saved" | "dirty" | "saving" | "session" | "error";
  export type PersistedSettingsSection = "general" | "security" | "performance" | "colors";

  export interface AssociationRow {
    ext: string;
    format: string;
    access: string;
    accessTone: "success" | "info" | "warning" | "neutral";
    volumes: string;
    volumeTone: "success" | "info" | "warning" | "neutral";
    action: string;
  }

  export interface IntegrationActionView {
    id: string;
    label: string;
    state: "healthy" | "missing" | "damaged" | "checking" | "unavailable";
    stateLabel: string;
    detail: string | null;
  }

  export interface IntegrationDiagnosticView {
    id: string;
    label: string;
    stateLabel: string;
    detail: string;
    tone: "neutral" | "info" | "warning";
    actionLabel?: string | null;
  }

  export interface SettingsWorkspaceProps {
    screen: SettingsScreen;
    tr: (key: string, fallback: string) => string;
    settingsSaveTarget: PersistedSettingsSection | null;
    appearanceSaveState: Exclude<SettingsSaveState, "dirty">;
    modernModeSelected: boolean;
    classicModeSelected: boolean;
    setMode: (mode: UiMode) => void;
    activeThemeChoice: "system" | "light" | "dark";
    setTheme: (theme: "system" | "light" | "dark") => void;
    activeDensityChoice: DensityChoice;
    setDensity: (density: DensityChoice) => void;
    activePalette: PaletteId;
    activeTheme: ResolvedTheme;
    customAccent: string;
    customAccentInput: string;
    customAccentValid: boolean;
    customAccentSaveError: boolean;
    accentContrastGuard: boolean;
    colorsSaveState: SettingsSaveState;
    colorSettingsDirty: boolean;
    paletteApplyBlocked: boolean;
    savePaletteSettings: () => void | Promise<void>;
    setPalette: (palette: PaletteId) => void;
    updateCustomAccent: (value: string, source: "color" | "hex") => void;
    onCustomAccentHexInput: (event: Event) => void;
    setAccentContrastGuard: (enabled: boolean) => void;
    generalSaveState: SettingsSaveState;
    generalSettingsDirty: boolean;
    generalSettingsValidationError: string;
    saveGeneralSettings: () => void | Promise<void>;
    availableLanguages: LanguageDto[];
    generalLanguageChoice: string;
    setGeneralLanguageChoice: (value: string) => void;
    generalDefaultCreateDir: string;
    setGeneralDefaultCreateDir: (value: string) => void;
    defaultCreateFolderError: string;
    chooseDefaultCreateFolder: () => void | Promise<void>;
    clearDefaultCreateFolder: () => void;
    generalDefaultExtractDir: string;
    setGeneralDefaultExtractDir: (value: string) => void;
    defaultExtractFolderError: string;
    chooseDefaultExtractFolder: () => void | Promise<void>;
    clearDefaultExtractFolder: () => void;
    generalRevealAfterExtract: boolean;
    setGeneralRevealAfterExtract: (value: boolean) => void;
    generalAutomaticUpdateChecks: boolean;
    setGeneralAutomaticUpdateChecks: (value: boolean) => void;
    fileManagerLabel: () => string;
    openWithLabel: () => string;
    updateCheckPreview: UpdateCheckPreview;
    securitySaveState: SettingsSaveState;
    safetySettingsDirty: boolean;
    safetyValidationError: string;
    saveSafetySettings: () => void | Promise<void>;
    safetyMaxEntries: NumericSetting;
    setSafetyMaxEntries: (value: NumericSetting) => void;
    safetyMaxEntriesError: string;
    safetyMaxOutputGiB: NumericSetting;
    setSafetyMaxOutputGiB: (value: NumericSetting) => void;
    safetyMaxOutputError: string;
    safetyMaxCompressionRatio: NumericSetting;
    setSafetyMaxCompressionRatio: (value: NumericSetting) => void;
    safetyMaxCompressionRatioError: string;
    resetSafetySettings: () => void;
    settingsSnapshotLabel: string;
    performanceSaveState: SettingsSaveState;
    performanceSettingsDirty: boolean;
    performanceValidationError: string;
    savePerformanceSettings: () => void | Promise<void>;
    performanceParallelJobs: NumericSetting;
    setPerformanceParallelJobs: (value: NumericSetting) => void;
    performanceParallelJobsError: string;
    choosePerformanceParallelJobs: (value: NumericSetting) => void;
    performanceThreads: NumericSetting;
    setPerformanceThreads: (value: NumericSetting) => void;
    performanceThreadsError: string;
    choosePerformanceThreads: (value: NumericSetting) => void;
    performanceMemoryKiB: NumericSetting;
    setPerformanceMemoryKiB: (value: NumericSetting) => void;
    performanceMemoryError: string;
    choosePerformanceMemory: (value: NumericSetting) => void;
    resetPerformanceSettings: () => void;
    passwordBookForgetDisabledReason: () => string;
    labelWithDisabledReason: (label: string, reason: string) => string;
    forgetPasswordBookPanel: () => void | Promise<void>;
    passwordBookSecretStoreLabel: () => string;
    platformNameLabel: () => string;
    secretStoreLabel: () => string;
    passwordBookCurrentLabel: () => string;
    passwordBookDetailLabel: () => string;
    passwordBookRefreshDisabledReason: () => string;
    passwordBookStatusState: string;
    refreshPasswordBookPanel: () => void | Promise<void>;
    currentArchiveName: () => string;
    formatRegistry: FormatDto[];
    formatRegistryLoaded: boolean;
    initialIntegrationStatus: IntegrationStatusDto | null;
    initialIntegrationDiagnostics: IntegrationSystemDiagnosticsDto | null;
    showMacosIntegrationDiagnostics: boolean;
    onNotice: (message: string) => void;
  }
</script>

<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import IntegrationHealthPanel from "./IntegrationHealthPanel.svelte";
  import SettingsSaveAction from "./SettingsSaveAction.svelte";
  import UpdateCheckCard from "./UpdateCheckCard.svelte";
  import { cssVariables } from "../lib/css-variables";
  import { basename as pathBaseName } from "../lib/format";
  import { recordOperation } from "../lib/history.svelte";
  import {
    desktopIntegrationActions,
    externalOpenActionCopy,
    type DesktopIntegrationAction,
  } from "../lib/external-tasks";
  import {
    ipc,
    type IntegrationActionHealthDto,
    type IntegrationRemoveResultDto,
  } from "../lib/ipc";
  import {
    buildCustomPaletteData,
    colorFromWheelPoint as colorFromWheelPointForAccent,
    colorToHex,
    colorWheelHsl as colorWheelHslForAccent,
    colorWheelVariables as colorWheelVariablesForAccent,
    deriveCustomPaletteTokens,
    hslToRgb,
  } from "../lib/theme";
  import {
    builtInPalettes,
    defaultCustomAccent,
    palettes,
  } from "../lib/ui-model";

  let {
    screen,
    tr,
    settingsSaveTarget,
    appearanceSaveState,
    modernModeSelected,
    classicModeSelected,
    setMode,
    activeThemeChoice,
    setTheme,
    activeDensityChoice,
    setDensity,
    activePalette,
    activeTheme,
    customAccent,
    customAccentInput,
    customAccentValid,
    customAccentSaveError,
    accentContrastGuard,
    colorsSaveState,
    colorSettingsDirty,
    paletteApplyBlocked,
    savePaletteSettings,
    setPalette,
    updateCustomAccent,
    onCustomAccentHexInput,
    setAccentContrastGuard,
    generalSaveState,
    generalSettingsDirty,
    generalSettingsValidationError,
    saveGeneralSettings,
    availableLanguages,
    generalLanguageChoice,
    setGeneralLanguageChoice,
    generalDefaultCreateDir,
    setGeneralDefaultCreateDir,
    defaultCreateFolderError,
    chooseDefaultCreateFolder,
    clearDefaultCreateFolder,
    generalDefaultExtractDir,
    setGeneralDefaultExtractDir,
    defaultExtractFolderError,
    chooseDefaultExtractFolder,
    clearDefaultExtractFolder,
    generalRevealAfterExtract,
    setGeneralRevealAfterExtract,
    generalAutomaticUpdateChecks,
    setGeneralAutomaticUpdateChecks,
    fileManagerLabel,
    openWithLabel,
    updateCheckPreview,
    securitySaveState,
    safetySettingsDirty,
    safetyValidationError,
    saveSafetySettings,
    safetyMaxEntries,
    setSafetyMaxEntries,
    safetyMaxEntriesError,
    safetyMaxOutputGiB,
    setSafetyMaxOutputGiB,
    safetyMaxOutputError,
    safetyMaxCompressionRatio,
    setSafetyMaxCompressionRatio,
    safetyMaxCompressionRatioError,
    resetSafetySettings,
    settingsSnapshotLabel,
    performanceSaveState,
    performanceSettingsDirty,
    performanceValidationError,
    savePerformanceSettings,
    performanceParallelJobs,
    setPerformanceParallelJobs,
    performanceParallelJobsError,
    choosePerformanceParallelJobs,
    performanceThreads,
    setPerformanceThreads,
    performanceThreadsError,
    choosePerformanceThreads,
    performanceMemoryKiB,
    setPerformanceMemoryKiB,
    performanceMemoryError,
    choosePerformanceMemory,
    resetPerformanceSettings,
    passwordBookForgetDisabledReason,
    labelWithDisabledReason,
    forgetPasswordBookPanel,
    passwordBookSecretStoreLabel,
    platformNameLabel,
    secretStoreLabel,
    passwordBookCurrentLabel,
    passwordBookDetailLabel,
    passwordBookRefreshDisabledReason,
    passwordBookStatusState,
    refreshPasswordBookPanel,
    currentArchiveName,
    formatRegistry,
    formatRegistryLoaded,
    initialIntegrationStatus,
    initialIntegrationDiagnostics,
    showMacosIntegrationDiagnostics,
    onNotice,
  }: SettingsWorkspaceProps = $props();

  const featuredFormatIds = ["zip", "7z", "sqz", "tar.zst", "wim", "rar", "dmg", "iso"];
  const longTailBridgeFormatIds = new Set([
    "apfs",
    "ar",
    "arj",
    "cab",
    "chm",
    "cpio",
    "cramfs",
    "dmg",
    "ext",
    "fat",
    "gpt",
    "hfs",
    "ihex",
    "iso",
    "lzh",
    "lzma",
    "mbr",
    "msi",
    "nsis",
    "ntfs",
    "qcow2",
    "rpm",
    "squashfs",
    "udf",
    "uefi",
    "vdi",
    "vhd",
    "vhdx",
    "vmdk",
    "xar",
    "z",
  ]);
  const numberFormatter = new Intl.NumberFormat("en-US");
  let associationFilter = $state("");
  let integrationOperation = $state<"idle" | "checking" | "repairing" | "removing">("idle");
  let integrationSnapshot = $state<IntegrationStatusDto | null>(
    untrack(() => initialIntegrationStatus),
  );
  let integrationDiagnostics = $state<IntegrationSystemDiagnosticsDto | null>(
    untrack(() => initialIntegrationDiagnostics),
  );
  let integrationUnavailable = $state(false);
  let integrationDiagnosticsUnavailable = $state(false);
  const customPaletteData = $derived<Palette>(
    buildCustomPaletteData(customAccent, activeTheme, accentContrastGuard),
  );
  const activePaletteData = $derived<Palette>(
    activePalette === "custom"
      ? customPaletteData
      : palettes.find((palette) => palette.id === activePalette) ?? palettes[0],
  );
  let integrationFinderActionsHealthy = $derived(integrationSnapshot?.health === "healthy");

  type RuntimeBackend = IntegrationSystemDiagnosticsDto["backends"][number];

  function registryFormatExtensions(): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const format of formatRegistry) {
      for (const extension of format.extensions) {
        const normalized = extension.toLowerCase().replace(/^\.+/, "").trim();
        if (!normalized || seen.has(normalized)) continue;
        seen.add(normalized);
        out.push(normalized);
      }
    }
    return out.sort((a, b) => b.length - a.length || a.localeCompare(b));
  }

  function archiveRegistryFormats(): FormatDto[] {
    return formatRegistry.filter((format) => format.kind === "archive");
  }

  function formatRegistrySourceLabel(): string {
    return formatRegistryLoaded
      ? tr("gui.settings.integration.format_registry", "Format registry")
      : tr("gui.settings.integration.preview_registry", "Preview registry");
  }

  function formatDisplayName(id: string): string {
    if (id === "sqz") return "SQZ";
    if (id === "tar.zst" || id === "tzst") return "TAR.ZST";
    if (id === "tar.gz" || id === "tgz") return "TAR.GZ";
    if (id === "tar.xz" || id === "txz") return "TAR.XZ";
    if (id === "tar.bz2" || id === "tbz2") return "TAR.BZ2";
    if (id === "bzip2" || id === "bz2") return "BZIP2";
    if (id === "gzip" || id === "gz") return "GZIP";
    if (id === "zstd" || id === "zst") return "ZSTD";
    if (id === "br") return "BROTLI";
    return id.toUpperCase();
  }

  function formatSortRank(format: FormatDto): string {
    const featured = featuredFormatIds.indexOf(format.id);
    if (featured >= 0) return `0-${featured.toString().padStart(2, "0")}`;
    if (format.can_create && format.can_extract) return `1-${format.id}`;
    if (format.can_extract) return `2-${format.id}`;
    return `3-${format.id}`;
  }

  function associationFormatLabel(format: FormatDto, extension: string): string {
    const display = formatDisplayName(format.id);
    const ext = extension.toLowerCase();
    if (format.id === "zip" && ext !== "zip") {
      return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", "ZIP");
    }
    if (format.id === "rar" && ext !== "rar") {
      return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", "RAR");
    }
    if (format.id.startsWith("tar.") && ext !== format.id) {
      return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", display);
    }
    if (format.kind === "compressor") {
      return tr("gui.settings.integration.format_stream", "{format} stream").replace("{format}", display);
    }
    return display;
  }

  function associationAccess(
    format: FormatDto,
    extension: string,
  ): { label: string; tone: AssociationRow["accessTone"] } {
    if (format.kind === "compressor") {
      return {
        label: tr("gui.settings.integration.stream_support", "Stream support"),
        tone: "neutral",
      };
    }
    if (format.id === "wim" && extension.toLowerCase() === "swm") {
      return {
        label: tr("gui.settings.integration.read_write", "Create + open"),
        tone: "success",
      };
    }
    if (format.id === "wim") {
      return {
        label: tr("gui.settings.integration.external_tools", "External tools"),
        tone: "info",
      };
    }
    if (!format.can_create && format.can_extract) {
      return {
        label: tr("gui.settings.integration.read_only", "Open only"),
        tone: "info",
      };
    }
    return {
      label: tr("gui.settings.integration.read_write", "Create + open"),
      tone: "success",
    };
  }

  function associationVolumeCapability(
    format: FormatDto,
    extension: string,
  ): { label: string; tone: AssociationRow["volumeTone"] } {
    const ext = extension.toLowerCase();
    if (format.id === "rar" && ext === "rar") {
      return {
        label: tr("gui.settings.integration.volume_rar_native_read", "Open native RAR volumes"),
        tone: "info",
      };
    }
    if (format.id === "wim") {
      return {
        label: tr("gui.settings.integration.volume_split_wim_read", "Create/open native Split WIM volumes"),
        tone: "success",
      };
    }
    if (format.id === "zip" && ext === "zip") {
      return {
        label: tr("gui.settings.integration.volume_zip_mixed", "Create/open .001 or native .z01"),
        tone: "success",
      };
    }
    if (format.id === "sqz") {
      return {
        label: tr("gui.settings.integration.volume_sqz", "Create/open SQZV .001"),
        tone: "success",
      };
    }
    if (format.can_split) {
      return {
        label: tr("gui.settings.integration.volume_generic", "Create/open .001"),
        tone: "success",
      };
    }
    return {
      label: tr("gui.settings.integration.volume_single", "Single archive"),
      tone: "neutral",
    };
  }

  function associationActionLabel(format: FormatDto, extension: string): string {
    const ext = extension.toLowerCase();
    if (format.id === "sqz") {
      return tr("gui.settings.integration.action_browse_extract_test_export", "Browse, extract, test, export");
    }
    if (format.id === "wim") {
      return ext === "swm"
        ? tr("gui.settings.integration.action_split_wim_read", "Create with wimlib or open any member via 7zz/7z; keep every .swm part together.")
        : tr("gui.settings.integration.action_wim", "Browse, extract via 7zz/7z; create via wimlib");
    }
    if (format.id === "rar") {
      return ext === "cbr"
        ? tr("gui.settings.integration.action_comics_7zz", "Browse comics via 7zz/7z")
        : tr("gui.settings.integration.action_7zz_bridge", "Browse, extract via 7zz/7z bridge");
    }
    if (longTailBridgeFormatIds.has(format.id)) {
      return tr("gui.settings.integration.action_7zz_bridge", "Browse, extract via 7zz/7z bridge");
    }
    if (format.id === "7z") {
      return tr("gui.settings.integration.action_browse_extract_convert", "Browse, extract, convert");
    }
    if (format.kind === "compressor") {
      return tr("gui.settings.integration.action_decompress_stream", "Decompress stream");
    }
    if (ext === "cbz") {
      return tr("gui.settings.integration.action_browse_comics_extract", "Browse comics, extract");
    }
    if (format.id.startsWith("tar.")) {
      return tr("gui.settings.integration.action_extract_convert", "Extract, convert");
    }
    return tr("gui.settings.integration.action_browse_extract_test", "Browse, extract, test");
  }

  function associationRows(): AssociationRow[] {
    const seen = new Set<string>();
    const rows: AssociationRow[] = [];
    const sortedFormats = formatRegistry
      .slice()
      .sort((a, b) => formatSortRank(a).localeCompare(formatSortRank(b)));
    for (const format of sortedFormats) {
      for (const rawExtension of format.extensions) {
        const normalized = rawExtension.toLowerCase().replace(/^\.+/, "").trim();
        if (!normalized || seen.has(normalized)) continue;
        seen.add(normalized);
        const access = associationAccess(format, normalized);
        const volumes = associationVolumeCapability(format, normalized);
        rows.push({
          ext: `.${normalized}`,
          format: associationFormatLabel(format, normalized),
          access: access.label,
          accessTone: access.tone,
          volumes: volumes.label,
          volumeTone: volumes.tone,
          action: associationActionLabel(format, normalized),
        });
      }
    }
    rows.push(
      {
        ext: ".par2",
        format: tr("gui.settings.integration.par2_sidecar", "PAR2 sidecar"),
        access: tr("gui.settings.integration.sidecar", "Sidecar"),
        accessTone: "neutral",
        volumes: tr("gui.settings.integration.volume_protects_sets", "Protects complete sets"),
        volumeTone: "info",
        action: tr("gui.settings.integration.verify_repair", "Verify, repair"),
      },
      {
        ext: ".001",
        format: tr("gui.settings.integration.generic_split_volume", "Generic byte volume"),
        access: tr("gui.settings.integration.read_write", "Create + open"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_generic_named", "Generic .001/.002"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_generic_split", "Split supported outputs; native ZIP and WIM use their dedicated layouts, while RAR remains open-only"),
      },
      {
        ext: ".z01",
        format: tr("gui.settings.integration.zip_native_volume", "Native ZIP volume"),
        access: tr("gui.settings.integration.read_write", "Create + open"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_zip_native", "Native ZIP .z01/.zip"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_zip_native", "Create native sets or open any member via 7zz/7z"),
      },
      {
        ext: ".r00",
        format: tr("gui.settings.integration.rar_native_volume", "Native RAR volume"),
        access: tr("gui.settings.integration.read_only", "Open only"),
        accessTone: "info",
        volumes: tr("gui.settings.integration.volume_rar_native", "Native RAR volume set"),
        volumeTone: "info",
        action: tr("gui.settings.integration.action_rar_native_read", "Open .r00 or partN.rar members; RAR creation is not supported"),
      },
    );
    if (!seen.has("swm")) {
      rows.push({
        ext: ".swm",
        format: tr("gui.settings.integration.split_wim", "Split WIM"),
        access: tr("gui.settings.integration.read_write", "Create + open"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_split_wim_read", "Create/open native Split WIM volumes"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_split_wim_read", "Create with wimlib or open any member via 7zz/7z; keep every .swm part together."),
      });
    }
    return rows;
  }

  function visibleAssociationRows(): AssociationRow[] {
    const query = associationFilter.trim().toLocaleLowerCase();
    if (!query) return associationRows();
    return associationRows().filter((row) =>
      [row.ext, row.format, row.access, row.volumes, row.action]
        .some((value) => value.toLocaleLowerCase().includes(query)),
    );
  }

  function clearAssociationFilter(): void {
    associationFilter = "";
  }

  function associationSummary(): string[] {
    const archiveFormats = archiveRegistryFormats();
    const writableArchives = archiveFormats.filter((format) => format.can_create).length;
    const unpackOnlyArchives = archiveFormats.filter(
      (format) => !format.can_create && format.can_extract,
    ).length;
    return [
      tr("gui.settings.integration.summary_registry_extensions", "{count} registry extensions")
        .replace("{count}", String(registryFormatExtensions().length)),
      tr("gui.settings.integration.summary_archive_families", "{count} archive families")
        .replace("{count}", String(archiveFormats.length)),
      tr("gui.settings.integration.summary_writable", "{count} writable")
        .replace("{count}", String(writableArchives)),
      tr("gui.settings.integration.summary_unpack_only", "{count} unpack-only")
        .replace("{count}", String(unpackOnlyArchives)),
      tr("gui.settings.integration.summary_sidecar_rules", "5 sidecar/volume rules"),
    ];
  }

  function integrationActionLabel(action: DesktopIntegrationAction): string {
    const copy = externalOpenActionCopy(action);
    return tr(copy.labelKey, copy.fallbackLabel);
  }

  function integrationApplyLabel(): string {
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_actions", "Repairing actions...");
    }
    if (integrationSnapshot?.health === "healthy") {
      return tr("gui.settings.integration.reinstall_actions", "Reinstall actions");
    }
    if (integrationSnapshot?.health === "needs_repair" || integrationUnavailable) {
      return tr("gui.settings.integration.repair_actions", "Repair actions");
    }
    return tr("gui.settings.integration.install_platform_actions", "Install {fileManager} actions")
      .replace("{fileManager}", fileManagerLabel());
  }

  function integrationBusy(): boolean {
    return integrationOperation !== "idle";
  }

  function integrationPanelState(): "idle" | "checking" | "repairing" | "removing" | "healthy" | "needs-repair" | "missing" | "unavailable" {
    if (integrationOperation !== "idle") return integrationOperation;
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") return "unavailable";
    if (integrationSnapshot?.health === "healthy") return "healthy";
    if (integrationSnapshot?.health === "needs_repair") return "needs-repair";
    if (integrationSnapshot?.health === "missing") return "missing";
    return "idle";
  }

  function integrationHealthCounts(
    status: IntegrationStatusDto | null = integrationSnapshot,
  ): { healthy: number; missing: number; damaged: number } {
    const actions = status?.actions ?? [];
    return {
      healthy: actions.filter((action) => action.state === "healthy").length,
      missing: actions.filter((action) => action.state === "missing").length,
      damaged: actions.filter((action) => action.state === "damaged").length,
    };
  }

  function integrationSummaryLabel(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.checking_status", "Checking {fileManager} actions...")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_status", "Repairing and checking {fileManager} actions...")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationOperation === "removing") {
      return tr("gui.settings.integration.removing_status", "Removing {fileManager} actions...")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") {
      return tr("gui.settings.integration.status_unavailable", "{fileManager} action status unavailable")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (!integrationSnapshot) {
      return tr("gui.settings.integration.status_not_checked", "Action status not checked");
    }
    if (integrationSnapshot.health === "missing") {
      return tr("gui.settings.integration.platform_actions_not_installed", "{fileManager} actions are not installed")
        .replace("{fileManager}", fileManagerLabel());
    }
    const counts = integrationHealthCounts();
    if (integrationSnapshot.health === "needs_repair") {
      return tr("gui.settings.integration.actions_need_repair", "{count} {fileManager} actions need repair")
        .replace("{fileManager}", fileManagerLabel())
        .replace("{count}", String(counts.missing + counts.damaged));
    }
    return tr("gui.settings.integration.actions_verified", "{count} {fileManager} action files verified")
      .replace("{fileManager}", fileManagerLabel())
      .replace("{count}", String(counts.healthy));
  }

  function integrationDetailLabel(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.checking_detail", "Reading the managed action files and launchers.");
    }
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_detail", "Reinstalling the managed files, then checking them again.");
    }
    if (integrationOperation === "removing") {
      return tr("gui.settings.integration.removing_detail", "Removing only the actions installed by Squallz.");
    }
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") {
      return tr("gui.settings.integration.status_help", "No previous result is shown. Refresh or try Repair.");
    }
    if (!integrationSnapshot || integrationSnapshot.health === "missing") {
      return tr("gui.settings.integration.install_detail", "Installs Checksum, Extract Here, Extract to Folder, Compress to 7Z, and Test Archive.");
    }
    if (integrationSnapshot.health === "needs_repair") {
      const counts = integrationHealthCounts();
      return tr("gui.settings.integration.health_counts", "{healthy} ready · {missing} missing · {damaged} damaged")
        .replace("{healthy}", String(counts.healthy))
        .replace("{missing}", String(counts.missing))
        .replace("{damaged}", String(counts.damaged));
    }
    const folder = pathBaseName(integrationSnapshot.services_dir);
    return folder
      ? tr("gui.settings.integration.verified_in", "Managed files match this Squallz version in {folder}.")
          .replace("{folder}", folder)
      : tr("gui.settings.integration.verified_detail", "Managed files and launchers match this version.");
  }

  function integrationActionSnapshot(
    action: DesktopIntegrationAction,
  ): IntegrationActionHealthDto | null {
    return integrationSnapshot?.actions.find((item) => item.id === action) ?? null;
  }

  function integrationActionState(
    action: DesktopIntegrationAction,
  ): "healthy" | "missing" | "damaged" | "checking" | "unavailable" {
    if (integrationBusy()) return "checking";
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") return "unavailable";
    if (!integrationSnapshot) return "checking";
    return integrationActionSnapshot(action)?.state ?? "missing";
  }

  function integrationActionStateLabel(action: DesktopIntegrationAction): string {
    const state = integrationActionState(action);
    if (state === "healthy") return tr("gui.settings.integration.action_healthy", "Verified");
    if (state === "damaged") return tr("gui.settings.integration.action_damaged", "Needs repair");
    if (state === "missing") return tr("gui.settings.integration.action_missing", "Missing");
    if (state === "checking") return tr("gui.settings.integration.action_checking", "Checking");
    return tr("gui.settings.integration.action_unavailable", "Unavailable");
  }

  function integrationActionDetail(action: DesktopIntegrationAction): string | null {
    if (integrationActionState(action) !== "damaged") return null;
    const issue = integrationActionSnapshot(action)?.issue;
    if (issue === "script_missing") {
      return tr("gui.settings.integration.issue_script_missing", "Action script is missing");
    }
    if (issue === "launcher_missing") {
      return tr("gui.settings.integration.issue_launcher_missing", "File-manager launcher is missing");
    }
    if (issue === "script_not_executable") {
      return tr("gui.settings.integration.issue_script_not_executable", "Action script cannot run");
    }
    if (issue === "script_outdated") {
      return tr("gui.settings.integration.issue_script_outdated", "Action script is changed or out of date");
    }
    if (issue === "launcher_outdated") {
      return tr("gui.settings.integration.issue_launcher_outdated", "File-manager launcher is changed or out of date");
    }
    if (issue === "registry_missing") {
      return tr("gui.settings.integration.issue_registry_missing", "Explorer registration is incomplete");
    }
    if (issue === "registry_outdated") {
      return tr("gui.settings.integration.issue_registry_outdated", "Explorer registration is changed or out of date");
    }
    return tr("gui.settings.integration.issue_unknown", "Managed action files do not match this version");
  }

  function integrationActionViews(): IntegrationActionView[] {
    return desktopIntegrationActions.map((action) => ({
      id: action,
      label: integrationActionLabel(action),
      state: integrationActionState(action),
      stateLabel: integrationActionStateLabel(action),
      detail: integrationActionDetail(action),
    }));
  }

  function integrationScopeLabel(): string {
    return tr(
      "gui.settings.integration.health_scope",
      "Repair updates only Squallz action files, launchers, and registrations. It does not change default apps; confirm menu visibility separately.",
    ).replace("{fileManager}", fileManagerLabel());
  }

  function integrationRemoveDisabledReason(): string {
    if (integrationBusy()) {
      return tr("gui.settings.integration.wait_for_action", "Wait for the current action to finish");
    }
    if (!integrationSnapshot || integrationUnavailable) {
      return tr("gui.settings.integration.check_before_removing", "Check installed actions before removing them");
    }
    if (!integrationSnapshot.can_remove) {
      return tr("gui.settings.integration.nothing_to_remove", "No installed actions to remove");
    }
    return "";
  }

  function integrationApplyDisabledReason(): string {
    if (integrationBusy()) {
      return tr("gui.settings.integration.wait_for_action", "Wait for the current action to finish");
    }
    if (integrationSnapshot && !integrationSnapshot.can_repair) {
      return tr("gui.settings.integration.repair_unavailable", "File-manager actions are not available on this platform");
    }
    return "";
  }

  function applyIntegrationStatusSnapshot(result: IntegrationStatusDto): void {
    integrationSnapshot = result;
    integrationUnavailable = result.health === "unavailable";
  }

  function clearIntegrationStatusSnapshot(): void {
    integrationSnapshot = null;
    integrationUnavailable = true;
  }

  async function refreshIntegrationDiagnostics(): Promise<void> {
    try {
      const result = await ipc.getSystemIntegrationDiagnostics();
      integrationDiagnostics = result;
      integrationDiagnosticsUnavailable = result.default_handlers.state === "unavailable";
    } catch {
      integrationDiagnostics = null;
      integrationDiagnosticsUnavailable = true;
    }
  }

  async function readIntegrationStatusWithDiagnostics(): Promise<IntegrationStatusDto> {
    const [status] = await Promise.allSettled([
      ipc.getIntegrationStatus(),
      refreshIntegrationDiagnostics(),
    ]);
    if (status.status === "rejected") throw status.reason;
    return status.value;
  }

  async function applyIntegrationChanges(): Promise<void> {
    const previousHealth = integrationSnapshot?.health ?? null;
    integrationOperation = "repairing";
    integrationUnavailable = false;
    try {
      await ipc.applyIntegrationChanges();
    } catch {
      clearIntegrationStatusSnapshot();
      onNotice(
        tr(
          "gui.settings.integration.repair_failed",
          "Could not repair {fileManager} actions. Check file permissions and try again.",
        ).replace("{fileManager}", fileManagerLabel()),
      );
      integrationOperation = "idle";
      return;
    }

    try {
      const status = await readIntegrationStatusWithDiagnostics();
      applyIntegrationStatusSnapshot(status);
      const complete = status.health === "healthy";
      const counts = integrationHealthCounts(status);
      recordOperation({
        status: complete ? "done" : "info",
        title: previousHealth === "missing" || previousHealth === null
          ? tr("gui.settings.integration.applied_title", "File-manager actions installed")
          : tr("gui.settings.integration.repaired_title", "File-manager actions repaired"),
        detail: complete
          ? tr("gui.settings.integration.actions_verified", "{count} {fileManager} action files verified")
              .replace("{count}", String(counts.healthy))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.repair_incomplete", "Repair finished, but {count} actions still need attention")
              .replace("{count}", String(counts.missing + counts.damaged)),
      });
      onNotice(
        complete
          ? tr("gui.settings.integration.repair_complete", "Verified {count} {fileManager} actions")
              .replace("{count}", String(counts.healthy))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.repair_incomplete", "Repair finished, but {count} actions still need attention")
              .replace("{count}", String(counts.missing + counts.damaged)),
      );
    } catch {
      clearIntegrationStatusSnapshot();
      const detail = tr(
        "gui.settings.integration.repair_verify_failed",
        "Actions were updated, but their status could not be verified. Refresh to check again.",
      );
      recordOperation({
        status: "info",
        title: previousHealth === "missing" || previousHealth === null
          ? tr("gui.settings.integration.applied_title", "File-manager actions installed")
          : tr("gui.settings.integration.repaired_title", "File-manager actions repaired"),
        detail,
      });
      onNotice(detail);
    } finally {
      integrationOperation = "idle";
    }
  }

  async function refreshIntegrationStatus(announce = true): Promise<void> {
    integrationOperation = "checking";
    integrationUnavailable = false;
    try {
      const result = await readIntegrationStatusWithDiagnostics();
      applyIntegrationStatusSnapshot(result);
      if (announce) {
        const counts = integrationHealthCounts(result);
        onNotice(
          result.health === "healthy"
            ? tr("gui.settings.integration.actions_verified", "{count} {fileManager} action files verified")
                .replace("{count}", String(counts.healthy))
                .replace("{fileManager}", fileManagerLabel())
            : result.health === "needs_repair"
              ? tr("gui.settings.integration.actions_need_repair", "{count} {fileManager} actions need repair")
                  .replace("{count}", String(counts.missing + counts.damaged))
                  .replace("{fileManager}", fileManagerLabel())
              : result.health === "missing"
                ? tr("gui.settings.integration.platform_actions_not_installed_short", "{fileManager} actions are not installed")
                    .replace("{fileManager}", fileManagerLabel())
                : tr("gui.settings.integration.status_unavailable", "{fileManager} action status unavailable")
                    .replace("{fileManager}", fileManagerLabel()),
        );
      }
    } catch {
      clearIntegrationStatusSnapshot();
      if (announce) {
        onNotice(
          tr(
            "gui.settings.integration.status_check_failed",
            "Could not check {fileManager} actions. Try Refresh again.",
          ).replace("{fileManager}", fileManagerLabel()),
        );
      }
    } finally {
      integrationOperation = "idle";
    }
  }

  async function removeIntegrationChanges(): Promise<void> {
    integrationOperation = "removing";
    integrationUnavailable = false;
    let result: IntegrationRemoveResultDto;
    try {
      result = await ipc.removeIntegrationChanges();
    } catch {
      clearIntegrationStatusSnapshot();
      onNotice(
        tr(
          "gui.settings.integration.remove_failed",
          "Could not remove {fileManager} actions. Check file permissions and try again.",
        ).replace("{fileManager}", fileManagerLabel()),
      );
      integrationOperation = "idle";
      return;
    }

    try {
      const status = await readIntegrationStatusWithDiagnostics();
      applyIntegrationStatusSnapshot(status);
      const complete = status.health === "missing";
      recordOperation({
        status: complete ? "done" : "info",
        title: tr("gui.settings.integration.removed_title", "Desktop integrations removed"),
        detail: complete
          ? tr("gui.settings.integration.removed_count", "{count} actions removed")
              .replace("{count}", String(result.removed.length))
          : tr("gui.settings.integration.remove_incomplete", "Removal finished, but managed action files are still present"),
      });
      onNotice(
        complete
          ? tr("gui.settings.integration.removed_platform_count", "Removed {count} {fileManager} actions")
              .replace("{count}", String(result.removed.length))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.remove_incomplete", "Removal finished, but managed action files are still present"),
      );
    } catch {
      clearIntegrationStatusSnapshot();
      const detail = tr(
        "gui.settings.integration.remove_verify_failed",
        "Removal finished, but cleanup could not be verified. Refresh to check again.",
      );
      recordOperation({
        status: "info",
        title: tr("gui.settings.integration.removed_title", "Desktop integrations removed"),
        detail,
      });
      onNotice(detail);
    } finally {
      integrationOperation = "idle";
    }
  }

  async function openIntegrationGuide(target: string): Promise<void> {
    const url = target === "default_handlers"
      ? "https://support.apple.com/guide/mac-help/mh35597/mac"
      : target === "finder_visibility"
        ? "https://support.apple.com/guide/mac-help/mchl97ff9142/mac"
        : target === "sevenzip"
          ? "https://www.7-zip.org/download.html"
          : target === "wimlib"
            ? "https://wimlib.net/downloads/"
          : target === "unrar"
            ? "https://www.rarlab.com/rar_add.htm"
            : null;
    if (!url) return;

    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(url);
      onNotice(
        target === "sevenzip"
          ? tr("gui.settings.integration.setup_guide_opened", "Opened the 7-Zip download page in your browser.")
          : target === "wimlib"
            ? tr("gui.settings.integration.wimlib_setup_guide_opened", "Opened the official wimlib download page in your browser.")
          : target === "unrar"
            ? tr("gui.settings.integration.unrar_setup_guide_opened", "Opened the official unrar download page in your browser.")
            : tr("gui.settings.integration.guide_opened", "Opened the Apple guide in your browser."),
      );
    } catch {
      onNotice(
        target === "sevenzip"
          ? tr("gui.settings.integration.setup_guide_open_failed", "Could not open the 7-Zip download page. Check your browser settings and try again.")
          : target === "wimlib"
            ? tr("gui.settings.integration.wimlib_setup_guide_open_failed", "Could not open the wimlib download page. Check your browser settings and try again.")
          : target === "unrar"
            ? tr("gui.settings.integration.unrar_setup_guide_open_failed", "Could not open the unrar download page. Check your browser settings and try again.")
            : tr("gui.settings.integration.guide_open_failed", "Could not open the Apple guide. Check your browser settings and try again."),
      );
    }
  }

  $effect(() => {
    if (
      screen === "integration"
      && !integrationSnapshot
      && !integrationUnavailable
      && integrationOperation === "idle"
    ) {
      void refreshIntegrationStatus(false);
    }
  });

  function integrationBackendSourceLabel(backend: RuntimeBackend | undefined): string {
    if (backend?.source === "application") {
      return tr("gui.settings.integration.backend_source_application", "Squallz application");
    }
    if (backend?.source === "environment") {
      return tr("gui.settings.integration.backend_source_environment", "custom configuration");
    }
    if (backend?.source === "path") {
      return tr("gui.settings.integration.backend_source_path", "system installation");
    }
    return tr("gui.settings.integration.backend_source_runtime", "runtime environment");
  }

  function integrationBackendStateLabel(backend: RuntimeBackend | undefined): string {
    if (backend?.available) return tr("gui.settings.integration.backend_found", "Found");
    if (backend?.configured) {
      return tr("gui.settings.integration.backend_misconfigured", "Configuration needs attention");
    }
    if (backend) return tr("gui.settings.integration.backend_missing", "Not installed");
    if (integrationDiagnostics !== null || integrationDiagnosticsUnavailable) {
      return tr("gui.settings.integration.system_unavailable", "Unavailable");
    }
    return tr("gui.settings.integration.system_checking", "Checking");
  }

  function integrationDefaultHandlerStateLabel(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.system_checking", "Checking");
    }
    if (integrationDiagnosticsUnavailable || !integrationDiagnostics) {
      return tr("gui.settings.integration.system_unavailable", "Unavailable");
    }
    const state = integrationDiagnostics.default_handlers.state;
    if (state === "squallz") return tr("gui.app.name", "Squallz");
    if (state === "mixed") {
      return tr("gui.settings.integration.default_handlers_mixed", "Different by format");
    }
    if (state === "other") {
      return tr("gui.settings.integration.default_handlers_other", "Other apps");
    }
    if (state === "unknown") {
      return tr("gui.settings.integration.default_handlers_unknown", "Partly unknown");
    }
    return tr("gui.settings.integration.system_unavailable", "Unavailable");
  }

  function integrationDefaultHandlerDetail(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.default_handlers_checking", "Reading current macOS default apps for registered archive types.");
    }
    const summary = integrationDiagnostics?.default_handlers;
    if (integrationDiagnosticsUnavailable || !summary || summary.state === "unavailable") {
      return tr("gui.settings.integration.default_handlers_unavailable", "macOS did not return the current default apps. Refresh to try again.");
    }

    let detail = tr("gui.settings.integration.default_handlers_count", "Squallz opens {squallz} of {total} registered archive types by default.")
      .replace("{squallz}", String(summary.squallz))
      .replace("{total}", String(summary.total));
    if (summary.checked < summary.total) {
      detail += ` ${tr("gui.settings.integration.default_handlers_partial", "Confirmed {checked} of {total} types.")
        .replace("{checked}", String(summary.checked))
        .replace("{total}", String(summary.total))}`;
    }
    const otherApplications = Array.from(new Set(summary.handlers.flatMap((handler) =>
      handler.state === "other" && handler.application_name ? [handler.application_name] : [],
    )));
    if (otherApplications.length > 0) {
      detail += ` ${tr("gui.settings.integration.default_handlers_apps", "Other defaults include {apps}.")
        .replace("{apps}", otherApplications.join(", "))}`;
    }
    return detail;
  }

  function integrationVisibilityDetail(): string {
    if (integrationFinderActionsHealthy) {
      return tr("gui.settings.integration.finder_visibility_ready", "The action files are ready. macOS does not expose a reliable visible or enabled status, so verify with a file selected in Finder.");
    }
    return tr("gui.settings.integration.finder_visibility_after_repair", "Install or repair the action files, then verify them with a file selected in Finder.");
  }

  function integrationDiagnosticViews(): IntegrationDiagnosticView[] {
    const sevenZip = integrationDiagnostics?.backends.find((backend) => backend.id === "sevenzip");
    const sevenZipDetail = sevenZip?.available
      ? tr(
          "gui.settings.integration.sevenzip_ready_detail",
          "Squallz groups and validates RAR volume sets; external {tool} from {source} provides read-only decoding, including encrypted archives. It also reads native ZIP volumes and long-tail formats. RAR creation is not supported.",
        )
          .replace("{source}", integrationBackendSourceLabel(sevenZip))
          .replace("{tool}", sevenZip.tool ?? "7zz/7z")
      : sevenZip?.configured
        ? tr(
            "gui.settings.integration.sevenzip_misconfigured_detail",
            "The SQUALLZ_7Z override cannot be used. Fix or remove it, then refresh this page.",
          )
        : sevenZip
          ? tr(
              "gui.settings.integration.sevenzip_missing_detail",
              "Squallz can identify RAR volume sets, but cannot list, test, or extract their contents until 7-Zip is installed or SQUALLZ_7Z is configured. Native ZIP volumes and long-tail formats are also unavailable. RAR creation is not supported.",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
                "Runtime diagnostics did not report this tool. Refresh to try again.",
              )
            : tr(
                "gui.settings.integration.sevenzip_checking_detail",
                "Checking the 7-Zip compatibility engine used by external read paths.",
              );
    const diagnostics: IntegrationDiagnosticView[] = [{
      id: "sevenzip",
      label: tr("gui.settings.integration.sevenzip_backend_title", "External 7-Zip engine"),
      stateLabel: integrationBackendStateLabel(sevenZip),
      detail: sevenZipDetail,
      tone: sevenZip?.available ? "info" : "warning",
      actionLabel: sevenZip && !sevenZip.available
        ? tr("gui.settings.integration.sevenzip_backend_guide", "Get 7-Zip")
        : null,
    }];

    const wimlib = integrationDiagnostics?.backends.find((backend) => backend.id === "wimlib");
    const wimlibDetail = wimlib?.available
        ? tr(
            "gui.settings.integration.wimlib_ready_detail",
            "Squallz found external {tool} from {source} and will use it to create standalone WIM images and native Split WIM (.swm) volume sets. Reading WIM and Split WIM is handled separately by the 7-Zip backend.",
          )
          .replace("{source}", integrationBackendSourceLabel(wimlib))
          .replace("{tool}", wimlib.tool ?? "wimlib-imagex")
      : wimlib?.configured
        ? tr(
            "gui.settings.integration.wimlib_misconfigured_detail",
            "The SQUALLZ_WIMLIB override cannot be used. Fix or remove it, then refresh this page.",
          )
        : wimlib
          ? tr(
              "gui.settings.integration.wimlib_missing_detail",
              "WIM and native Split WIM creation is unavailable until wimlib-imagex is installed or SQUALLZ_WIMLIB is configured. Reading WIM and Split WIM is handled separately by the 7-Zip backend.",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
                "Runtime diagnostics did not report this tool. Refresh to try again.",
              )
            : tr(
                "gui.settings.integration.wimlib_checking_detail",
                "Checking the wimlib-imagex writer used for WIM and native Split WIM creation.",
              );
    diagnostics.push({
      id: "wimlib",
      label: tr("gui.settings.integration.wimlib_backend_title", "WIM creation engine"),
      stateLabel: integrationBackendStateLabel(wimlib),
      detail: wimlibDetail,
      tone: wimlib?.available ? "info" : "warning",
      actionLabel: wimlib && !wimlib.available
        ? tr("gui.settings.integration.wimlib_backend_guide", "Get wimlib")
        : null,
    });

    const unrar = integrationDiagnostics?.backends.find((backend) => backend.id === "unrar");
    const unrarReady = Boolean(unrar?.available && sevenZip?.available);
    const unrarState = unrar?.available && !sevenZip?.available
      ? tr("gui.settings.integration.unrar_waiting_sevenzip", "Waiting for 7-Zip")
      : integrationBackendStateLabel(unrar);
    const unrarDetail = unrar?.available
      ? sevenZip?.available
        ? tr(
            "gui.settings.integration.unrar_ready_detail",
            "Optional {tool} from {source} streams only confirmed-unencrypted RAR7/v6 entries that 7-Zip cannot decode. Squallz never sends encrypted or unknown-encryption archives to it, and does not bundle it.",
          )
            .replace("{source}", integrationBackendSourceLabel(unrar))
            .replace("{tool}", unrar.tool ?? "unrar")
        : tr(
            "gui.settings.integration.unrar_waiting_detail",
            "{tool} is installed, but Squallz first needs 7-Zip to list the archive and confirm every RAR7/v6 entry is unencrypted. Configure 7-Zip to enable this fallback.",
          ).replace("{tool}", unrar.tool ?? "unrar")
      : unrar?.configured
        ? tr(
            "gui.settings.integration.unrar_misconfigured_detail",
            "The SQUALLZ_UNRAR override cannot be used. Fix or remove it, then refresh this page.",
          )
        : unrar
          ? tr(
              "gui.settings.integration.unrar_missing_detail",
              "Optional. Most RAR read paths continue through 7-Zip; install unrar only for confirmed-unencrypted RAR7/v6 archives that 7-Zip reports as unsupported.",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
                "Runtime diagnostics did not report this tool. Refresh to try again.",
              )
            : tr(
                "gui.settings.integration.unrar_checking_detail",
                "Checking the optional RAR7/v6 decoder fallback.",
              );
    diagnostics.push({
      id: "unrar",
      label: tr("gui.settings.integration.unrar_backend_title", "Optional RAR7 decoder"),
      stateLabel: unrarState,
      detail: unrarDetail,
      tone: unrarReady ? "info" : unrar?.configured && !unrar.available ? "warning" : "neutral",
      actionLabel: unrar && !unrar.available
        ? tr("gui.settings.integration.unrar_backend_guide", "Get unrar")
        : null,
    });

    if (!showMacosIntegrationDiagnostics) return diagnostics;
    const defaultHandlersState = integrationDiagnostics?.default_handlers.state;
    return diagnostics.concat([
      {
        id: "default_handlers",
        label: tr("gui.settings.integration.default_handlers_title", "Default opening apps"),
        stateLabel: integrationDefaultHandlerStateLabel(),
        detail: integrationDefaultHandlerDetail(),
        tone: integrationDiagnosticsUnavailable || defaultHandlersState === "unavailable"
          ? "warning"
          : defaultHandlersState === "mixed" || defaultHandlersState === "squallz"
            ? "info"
            : "neutral",
        actionLabel: tr("gui.settings.integration.default_handlers_guide", "How to change default apps"),
      },
      {
        id: "finder_visibility",
        label: tr("gui.settings.integration.finder_visibility_title", "Finder Quick Actions"),
        stateLabel: tr("gui.settings.integration.finder_visibility_manual", "Check in Finder"),
        detail: integrationVisibilityDetail(),
        tone: "neutral",
        actionLabel: tr("gui.settings.integration.finder_visibility_guide", "How to verify in Finder"),
      },
    ]);
  }

  function settingsSaveStateLabel(state: SettingsSaveState): string {
    if (state === "saving") return tr("gui.settings.save_state.saving", "Saving…");
    if (state === "dirty") return tr("gui.settings.save_state.dirty", "Unsaved changes");
    if (state === "session") return tr("gui.settings.save_state.session", "Session only · not saved");
    if (state === "error") return tr("gui.settings.save_state.error", "Could not save");
    return tr("gui.settings.save_state.saved", "Up to date");
  }

  function settingsSaveStatusLabel(
    section: PersistedSettingsSection,
    state: SettingsSaveState,
  ): string {
    return state !== "saved" && settingsSaveTarget !== null && settingsSaveTarget !== section
      ? tr("gui.settings.save_in_progress", "Wait for the current save to finish")
      : settingsSaveStateLabel(state);
  }

  function settingsSaveDisabledReason(
    section: PersistedSettingsSection,
    dirty: boolean,
    validationError = "",
  ): string {
    if (validationError) return validationError;
    if (!dirty) return tr("gui.settings.no_changes", "No changes to save");
    if (settingsSaveTarget !== null && settingsSaveTarget !== section) {
      return tr("gui.settings.save_in_progress", "Wait for the current save to finish");
    }
    return "";
  }

  function paletteName(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.name`, palette.name);
  }

  function paletteMood(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.mood`, palette.mood);
  }

  function paletteNote(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.note`, palette.note);
  }

  function palettePreviewContrast(palette: Palette): string {
    return activeTheme === "dark" ? (palette.darkContrast ?? palette.contrast) : palette.contrast;
  }

  function activePaletteName(): string {
    return paletteName(activePaletteData);
  }

  function customThemePreviewVariables(theme: ResolvedTheme): CssVariableMap {
    return deriveCustomPaletteTokens(customAccent, theme, accentContrastGuard);
  }

  function colorWheelCssVariables(): CssVariableMap {
    return {
      ...customThemePreviewVariables(activeTheme),
      ...colorWheelVariablesForAccent(customAccent),
    };
  }

  function updateCustomAccentFromWheel(event: PointerEvent | MouseEvent): void {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const size = Math.min(rect.width, rect.height);
    const x = Math.max(0, Math.min(size, event.clientX - rect.left));
    const y = Math.max(0, Math.min(size, event.clientY - rect.top));
    updateCustomAccent(
      colorFromWheelPointForAccent(customAccent, x, y, size),
      "color",
    );
  }

  function updateCustomAccentFromWheelClick(event: MouseEvent): void {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    if (
      event.clientX < rect.left
      || event.clientX > rect.right
      || event.clientY < rect.top
      || event.clientY > rect.bottom
    ) {
      return;
    }
    updateCustomAccentFromWheel(event);
  }

  function onColorWheelPointerDown(event: PointerEvent): void {
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    updateCustomAccentFromWheel(event);
  }

  function onColorWheelPointerMove(event: PointerEvent): void {
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      updateCustomAccentFromWheel(event);
    }
  }

  function onColorWheelPointerEnd(event: PointerEvent): void {
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
  }

  function onColorWheelKeydown(event: KeyboardEvent): void {
    const hsl = colorWheelHslForAccent(customAccent);
    const hueStep = event.shiftKey ? 12 : 4;
    const saturationStep = event.shiftKey ? 0.08 : 0.03;
    let next = hsl;

    if (event.key === "ArrowLeft") {
      next = { ...hsl, h: hsl.h - hueStep };
    } else if (event.key === "ArrowRight") {
      next = { ...hsl, h: hsl.h + hueStep };
    } else if (event.key === "ArrowUp") {
      next = { ...hsl, s: Math.min(1, hsl.s + saturationStep) };
    } else if (event.key === "ArrowDown") {
      next = { ...hsl, s: Math.max(0, hsl.s - saturationStep) };
    } else if (event.key === "Home") {
      next = { ...hsl, s: 0 };
    } else if (event.key === "End") {
      next = { ...hsl, s: 1 };
    } else {
      return;
    }

    event.preventDefault();
    updateCustomAccent(colorToHex(hslToRgb(next)), "color");
  }

  function customAccentStatusLabel(): string {
    if (!customAccentValid) {
      return tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color");
    }
    return accentContrastGuard
      ? tr("gui.colors.light_dark_auto", "Light and dark variants are generated automatically")
      : tr("gui.colors.direct_accent", "Direct accent preview · semantic colors stay locked");
  }

  function formattedDraftNumber(value: NumericSetting): string {
    return value === null ? "—" : numberFormatter.format(value);
  }

  function streamBufferKiBLabel(value: number): string {
    return tr("gui.settings.performance.buffer_kib_value", "{count} KiB")
      .replace("{count}", numberFormatter.format(value));
  }

  function inputNumber(event: Event): NumericSetting {
    const input = event.currentTarget as HTMLInputElement;
    return input.value.trim() === "" || !Number.isFinite(input.valueAsNumber)
      ? null
      : input.valueAsNumber;
  }
</script>

{#if screen === "appearance"}
  <div class="appearance-view modern-appearance">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.appearance.eyebrow", "Appearance")}</span>
        <h1>{tr("gui.appearance.title", "Interface and display")}</h1>
        <p>{tr("gui.appearance.subtitle", "Choose the interface, theme, and spacing that feel comfortable. Theme colors are available on their own page.")}</p>
      </div>
      <div class={`settings-live-state state-${appearanceSaveState}`} role="status" aria-live="polite">
        <Icon
          name={appearanceSaveState === "error" ? "x-circle" : appearanceSaveState === "saving" ? "hourglass" : appearanceSaveState === "session" ? "info" : "check-circle"}
          size={16}
        />
        {appearanceSaveState === "saved"
          ? tr("gui.appearance.changes_apply_immediately", "Changes apply immediately")
          : settingsSaveStateLabel(appearanceSaveState)}
      </div>
    </div>

    <div class="appearance-layout interface-layout">
      <section class="display-settings-panel main-display-panel">
        <div class="panel-title"><Icon name="list" size={16} />{tr("gui.appearance.display_settings", "Display settings")}</div>
        <div class="setting-list">
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.interface_mode", "Interface mode")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.interface_mode", "Interface mode")}>
              <button class:active={modernModeSelected} aria-pressed={modernModeSelected} onclick={() => setMode("modern")}>{tr("gui.mode.modern", "Modern")}</button>
              <button class:active={classicModeSelected} aria-pressed={classicModeSelected} onclick={() => setMode("classic")}>{tr("gui.mode.classic", "Classic")}</button>
            </div>
          </div>
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.theme", "Theme")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.theme_preference", "Theme preference")}>
              <button class:active={activeThemeChoice === "light"} aria-pressed={activeThemeChoice === "light"} onclick={() => setTheme("light")}>{tr("gui.theme.light", "Light")}</button>
              <button class:active={activeThemeChoice === "dark"} aria-pressed={activeThemeChoice === "dark"} onclick={() => setTheme("dark")}>{tr("gui.theme.dark", "Dark")}</button>
              <button class:active={activeThemeChoice === "system"} aria-pressed={activeThemeChoice === "system"} onclick={() => setTheme("system")}>{tr("gui.theme.system", "System")}</button>
            </div>
          </div>
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.density", "Density")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.density_preference", "Density preference")}>
              <button class:active={activeDensityChoice === "compact"} aria-pressed={activeDensityChoice === "compact"} onclick={() => setDensity("compact")}>{tr("gui.density.compact", "Compact")}</button>
              <button class:active={activeDensityChoice === "standard"} aria-pressed={activeDensityChoice === "standard"} onclick={() => setDensity("standard")}>{tr("gui.density.standard", "Standard")}</button>
              <button class:active={activeDensityChoice === "comfort"} aria-pressed={activeDensityChoice === "comfort"} onclick={() => setDensity("comfort")}>{tr("gui.density.comfort", "Comfort")}</button>
            </div>
          </div>
          <div><span>{tr("gui.appearance.current_colors", "Current theme colors")}</span><strong>{activePaletteName()}</strong></div>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "colors"}
  <div class="colors-view modern-colors">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.screen.colors", "Appearance · Theme Colors")}</span>
        <h1>{tr("gui.colors.title", "Theme colors and custom accent")}</h1>
        <p>{tr("gui.colors.subtitle", "Choose a balanced preset or create a custom accent. Safety and status colors stay recognizable.")}</p>
      </div>
      <SettingsSaveAction
        state={colorsSaveState}
        statusLabel={settingsSaveStatusLabel("colors", colorsSaveState)}
        actionLabel={tr("gui.colors.apply", "Apply theme colors")}
        savingLabel={tr("gui.settings.save_state.saving", "Saving…")}
        disabledReason={settingsSaveDisabledReason(
          "colors",
          colorSettingsDirty,
          paletteApplyBlocked ? tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color") : "",
        )}
        icon="sparkles"
        onSave={() => void savePaletteSettings()}
      />
    </div>

    <div class="appearance-layout">
      <section class="palette-panel">
        <div class="panel-title"><Icon name="palette" size={16} />{tr("gui.colors.curated_palettes", "Theme color presets")}</div>
        <div class="palette-grid">
          {#each builtInPalettes as palette}
            <button
              class:selected={palette.id === activePalette}
              class={`palette-card palette-${palette.id} theme-${activeTheme}`}
              onclick={() => setPalette(palette.id)}
            >
              <div class="palette-card-head">
                <strong>{paletteName(palette)}</strong>
                <span>{paletteMood(palette)}</span>
              </div>
              <div class="palette-swatches"><i></i><i></i><i></i></div>
              <p>{paletteNote(palette)}</p>
              <small>{tr("gui.colors.aa_contrast", "AA contrast")} {palettePreviewContrast(palette)}</small>
            </button>
          {/each}
        </div>
      </section>

      <aside class="color-workbench">
        <div class="panel-title"><Icon name="sparkles" size={16} />{tr("gui.colors.custom_color_wheel", "Custom color wheel")}</div>
        <div class="color-wheel-wrap">
          <div class="color-wheel-picker" use:cssVariables={colorWheelCssVariables()}>
            <button
              type="button"
              class="color-wheel-button"
              aria-label={`${tr("gui.colors.custom_accent_hue_wheel", "Custom accent hue wheel")} ${customAccent}`}
              aria-describedby="custom-color-status"
              aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End"
              title={`${tr("gui.colors.custom_accent_hue_wheel", "Custom accent hue wheel")} ${customAccent}`}
              onpointerdown={onColorWheelPointerDown}
              onpointermove={onColorWheelPointerMove}
              onpointerup={onColorWheelPointerEnd}
              onpointercancel={onColorWheelPointerEnd}
              onclick={updateCustomAccentFromWheelClick}
              onkeydown={onColorWheelKeydown}
            >
              <span class="color-wheel-surface"></span>
              <span class="color-wheel-marker"></span>
            </button>
          </div>
          <div class="custom-color-readout">
            <strong>{customAccent}</strong>
            <span>{tr("gui.colors.accent_preview", "Accent preview")}</span>
            <button class:active={activePalette === "custom"} class="custom-select-button" onclick={() => setPalette("custom")}>
              {activePalette === "custom" ? tr("common.current", "Current") : tr("gui.colors.use_custom", "Use custom")}
            </button>
          </div>
        </div>
        <div class="custom-color-fields">
          <label>
            <span>{tr("gui.colors.hex_value", "Hex value")}</span>
            <input
              class:invalid={!customAccentValid}
              value={customAccentInput}
              maxlength="7"
              spellcheck="false"
              aria-invalid={!customAccentValid}
              aria-label={tr("gui.colors.hex_value", "Hex value")}
              aria-describedby="custom-color-status"
              oninput={onCustomAccentHexInput}
            />
          </label>
          <button onclick={() => updateCustomAccent(defaultCustomAccent, "color")}>{tr("gui.colors.reset_custom", "Reset")}</button>
        </div>
        <div
          id="custom-color-status"
          class:error={customAccentSaveError || !customAccentValid}
          class="custom-color-status"
          aria-live="polite"
        >{customAccentStatusLabel()}</div>
        <label class="settings-switch contrast-guard-toggle">
          <input
            type="checkbox"
            checked={accentContrastGuard}
            aria-label={tr("gui.colors.contrast_guard_toggle", "Contrast guard")}
            aria-describedby="contrast-guard-note"
            title={accentContrastGuard ? tr("gui.colors.contrast_guard_enabled", "On · readable light/dark variants") : tr("gui.colors.contrast_guard_disabled", "Off · use accent more directly")}
            onchange={(event) => setAccentContrastGuard(event.currentTarget.checked)}
          />
          <span>{accentContrastGuard ? tr("gui.colors.contrast_guard_enabled", "On · readable light/dark variants") : tr("gui.colors.contrast_guard_disabled", "Off · use accent more directly")}</span>
        </label>
        <div class="theme-preview-pair">
          <div class="theme-preview custom-preview theme-light" use:cssVariables={customThemePreviewVariables("light")}>
            <div class="preview-toolbar"><span></span><span></span><span class="preview-theme-pill">{tr("gui.theme.light", "Light")}</span></div>
            <div class="preview-row selected"><span>archive.7z</span><strong>{tr("common.readiness", "Ready")}</strong></div>
            <div class="preview-row"><span>sidecar.par2</span><strong>{tr("gui.colors.protected_preview", "Protected")}</strong></div>
          </div>
          <div class="theme-preview custom-preview theme-dark" use:cssVariables={customThemePreviewVariables("dark")}>
            <div class="preview-toolbar"><span></span><span></span><span class="preview-theme-pill">{tr("gui.theme.dark", "Dark")}</span></div>
            <div class="preview-row selected"><span>archive.7z</span><strong>{tr("common.readiness", "Ready")}</strong></div>
            <div class="preview-row"><span>sidecar.par2</span><strong>{tr("gui.colors.protected_preview", "Protected")}</strong></div>
          </div>
        </div>
        <div id="contrast-guard-note" class="contrast-note" aria-live="polite">
          <strong>{accentContrastGuard ? tr("gui.colors.contrast_guard_on", "Contrast guard on") : tr("gui.colors.contrast_guard_off", "Contrast guard off")}</strong>
          <span>{tr("gui.colors.contrast_guard_body", "Error, warning, success, and recovery state colors stay semantic; custom accent only changes brand chrome and selection.")}</span>
        </div>
      </aside>
    </div>
  </div>
{:else if screen === "settingsGeneral"}
  <div class="settings-view modern-settings-general">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.general.eyebrow", "Settings / General")}</span>
        <h1>{tr("gui.settings.general.title", "General app behavior")}</h1>
        <p>{tr("gui.settings.general.subtitle", "Choose the app language, default create and extraction folders, and what happens after extraction completes.")}</p>
      </div>
      <SettingsSaveAction
        state={generalSaveState}
        statusLabel={settingsSaveStatusLabel("general", generalSaveState)}
        actionLabel={tr("gui.settings.general.apply", "Apply general")}
        savingLabel={tr("gui.settings.save_state.saving", "Saving…")}
        disabledReason={settingsSaveDisabledReason(
          "general",
          generalSettingsDirty,
          generalSettingsValidationError,
        )}
        icon="settings"
        onSave={() => void saveGeneralSettings()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.section.general", "General")}</div>
        <div class="setting-list">
          <div class="setting-control-row">
            <span>{tr("gui.settings.language", "Language")}</span>
            <select
              class="settings-select"
              value={generalLanguageChoice}
              aria-label={tr("gui.settings.language.preference_label", "Language preference")}
              onchange={(event) => setGeneralLanguageChoice(event.currentTarget.value)}
            >
              <option value="">{tr("gui.settings.language.follow_system", "Follow system")}</option>
              {#each availableLanguages as language}
                <option value={language.tag}>{language.name} · {language.tag}</option>
              {/each}
            </select>
          </div>
          <div class="setting-control-row folder-setting-row">
            <span>{tr("gui.settings.folder.default_create", "Default create folder")}</span>
            <div class="settings-path-field">
              <div class="settings-path-control">
                <input
                  class:invalid={Boolean(defaultCreateFolderError)}
                  class="settings-path-input"
                  value={generalDefaultCreateDir}
                  placeholder={tr("gui.settings.folder.ask_when_creating", "Ask when creating")}
                  aria-label={tr("gui.settings.folder.default_create", "Default create folder")}
                  aria-invalid={Boolean(defaultCreateFolderError)}
                  aria-describedby={defaultCreateFolderError ? "settings-default-create-folder-error" : undefined}
                  oninput={(event) => setGeneralDefaultCreateDir(event.currentTarget.value)}
                />
                <button type="button" aria-label={tr("gui.settings.folder.choose_create", "Choose default create folder")} onclick={() => void chooseDefaultCreateFolder()}>
                  <Icon name="folder-open" size={15} />
                </button>
                <button type="button" class="settings-path-reset" onclick={clearDefaultCreateFolder}>{tr("gui.settings.folder.clear", "Clear")}</button>
              </div>
              {#if defaultCreateFolderError}<small id="settings-default-create-folder-error" class="settings-field-error" role="status">{defaultCreateFolderError}</small>{/if}
            </div>
          </div>
          <div class="setting-control-row folder-setting-row">
            <span>{tr("gui.settings.folder.default_extract", "Default extract folder")}</span>
            <div class="settings-path-field">
              <div class="settings-path-control">
                <input
                  class:invalid={Boolean(defaultExtractFolderError)}
                  class="settings-path-input"
                  value={generalDefaultExtractDir}
                  placeholder={tr("gui.settings.folder.next_to_archive", "Next to archive")}
                  aria-label={tr("gui.settings.folder.default_extract", "Default extract folder")}
                  aria-invalid={Boolean(defaultExtractFolderError)}
                  aria-describedby={defaultExtractFolderError ? "settings-default-extract-folder-error" : undefined}
                  oninput={(event) => setGeneralDefaultExtractDir(event.currentTarget.value)}
                />
                <button type="button" aria-label={tr("gui.settings.folder.choose", "Choose default extract folder")} onclick={() => void chooseDefaultExtractFolder()}>
                  <Icon name="folder-open" size={15} />
                </button>
                <button type="button" class="settings-path-reset" onclick={clearDefaultExtractFolder}>{tr("gui.settings.folder.default", "Default")}</button>
              </div>
              {#if defaultExtractFolderError}<small id="settings-default-extract-folder-error" class="settings-field-error" role="status">{defaultExtractFolderError}</small>{/if}
            </div>
          </div>
          <div class="setting-control-row">
            <span>{tr("gui.settings.general.reveal_after_extract", "Reveal after extract")}</span>
            <label class="settings-switch">
              <input
                type="checkbox"
                checked={generalRevealAfterExtract}
                aria-label={tr("gui.settings.general.reveal_after_extract_aria", "Reveal extracted destination in {fileManager} after successful extract").replace("{fileManager}", fileManagerLabel())}
                onchange={(event) => setGeneralRevealAfterExtract(event.currentTarget.checked)}
              />
              <span>{generalRevealAfterExtract ? tr("common.on", "On") : tr("common.off", "Off")} · {tr("gui.settings.general.reveal_after_extract_hint", "Show destination in {fileManager}").replace("{fileManager}", fileManagerLabel())}</span>
            </label>
          </div>
          <div class="setting-control-row">
            <span>{tr("gui.settings.general.automatic_update_checks", "Automatic update checks")}</span>
            <label class="settings-switch">
              <input
                type="checkbox"
                checked={generalAutomaticUpdateChecks}
                aria-label={tr("gui.settings.general.automatic_update_checks_aria", "Automatically check the stable release channel for updates")}
                onchange={(event) => setGeneralAutomaticUpdateChecks(event.currentTarget.checked)}
              />
              <span>{generalAutomaticUpdateChecks ? tr("common.on", "On") : tr("common.off", "Off")} · {tr("gui.settings.general.automatic_update_checks_hint", "Check at most once every 24 hours")}</span>
            </label>
          </div>
          <div><span>{tr("gui.settings.general.open_with_policy", "{openWith} policy").replace("{openWith}", openWithLabel())}</span><strong>{tr("gui.settings.general.open_with_value", "Candidate only, never steal defaults")}</strong></div>
        </div>
        <UpdateCheckCard
          {tr}
          preview={updateCheckPreview}
          automaticChecksEnabled={generalAutomaticUpdateChecks}
        />
        <div class="setting-callout">
          <strong>{tr("gui.settings.general.boundary_title", "Safety prompts stay visible")}</strong>
          <span>{tr("gui.settings.general.boundary_body", "Password, recovery, unsafe path, and conflict prompts remain visible in their workflows.")}</span>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "settingsSecurity"}
  <div class="settings-view modern-settings-security">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.security.eyebrow", "Settings / Security")}</span>
        <h1>{tr("gui.settings.security.title", "Extraction safety and privacy")}</h1>
        <p>{tr("gui.settings.security.subtitle", "Set resource limits for extraction. Path traversal and link escapes remain blocked at every level.")}</p>
      </div>
      <SettingsSaveAction
        state={securitySaveState}
        statusLabel={settingsSaveStatusLabel("security", securitySaveState)}
        actionLabel={tr("gui.settings.security.save", "Save security")}
        savingLabel={tr("gui.settings.save_state.saving", "Saving…")}
        disabledReason={settingsSaveDisabledReason("security", safetySettingsDirty, safetyValidationError)}
        icon="shield-alert"
        onSave={() => void saveSafetySettings()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="settings-metric-grid">
          <div class:invalid-setting={Boolean(safetyMaxEntriesError)}><span>{tr("gui.settings.security.max_entries", "Max entries")}</span><strong>{formattedDraftNumber(safetyMaxEntries)}</strong><small>{tr("gui.settings.captured_job_start", "Captured when job starts")}</small></div>
          <div class:invalid-setting={Boolean(safetyMaxOutputError)}><span>{tr("gui.settings.security.max_output", "Max output")}</span><strong>{formattedDraftNumber(safetyMaxOutputGiB)} GiB</strong><small>{tr("gui.settings.security.archive_bomb_guard", "Archive bomb guard")}</small></div>
          <div class:invalid-setting={Boolean(safetyMaxCompressionRatioError)}><span>{tr("gui.settings.security.ratio_guard", "Ratio guard")}</span><strong>{formattedDraftNumber(safetyMaxCompressionRatio)}x</strong><small>{tr("gui.settings.security.ratio_hint_short", "Stops suspicious expansion")}</small></div>
        </div>
        <div class="settings-input-grid" aria-label={tr("common.safety_limits", "Safety limits")}>
          <label class="number-field">
            <span>{tr("gui.settings.security.max_entries", "Max entries")}</span>
            <input
              id="settings-security-max-entries"
              class:invalid={Boolean(safetyMaxEntriesError)}
              type="number"
              min="1"
              max="10000000"
              step="1000"
              value={safetyMaxEntries ?? ""}
              aria-invalid={Boolean(safetyMaxEntriesError)}
              aria-describedby={safetyMaxEntriesError ? "settings-security-max-entries-error" : undefined}
              oninput={(event) => setSafetyMaxEntries(inputNumber(event))}
            />
            {#if safetyMaxEntriesError}<small id="settings-security-max-entries-error" class="settings-field-error" role="status">{safetyMaxEntriesError}</small>{/if}
          </label>
          <label class="number-field">
            <span>{tr("gui.settings.security.max_output_gib", "Max output GiB")}</span>
            <input
              id="settings-security-max-output"
              class:invalid={Boolean(safetyMaxOutputError)}
              type="number"
              min="1"
              max="8192"
              step="1"
              value={safetyMaxOutputGiB ?? ""}
              aria-invalid={Boolean(safetyMaxOutputError)}
              aria-describedby={safetyMaxOutputError ? "settings-security-max-output-error" : undefined}
              oninput={(event) => setSafetyMaxOutputGiB(inputNumber(event))}
            />
            {#if safetyMaxOutputError}<small id="settings-security-max-output-error" class="settings-field-error" role="status">{safetyMaxOutputError}</small>{/if}
          </label>
          <label class="number-field">
            <span>{tr("gui.settings.security.ratio_guard", "Ratio guard")}</span>
            <input
              id="settings-security-ratio"
              class:invalid={Boolean(safetyMaxCompressionRatioError)}
              type="number"
              min="1"
              max="100000"
              step="1"
              value={safetyMaxCompressionRatio ?? ""}
              aria-invalid={Boolean(safetyMaxCompressionRatioError)}
              aria-describedby={safetyMaxCompressionRatioError ? "settings-security-ratio-error" : undefined}
              oninput={(event) => setSafetyMaxCompressionRatio(inputNumber(event))}
            />
            {#if safetyMaxCompressionRatioError}<small id="settings-security-ratio-error" class="settings-field-error" role="status">{safetyMaxCompressionRatioError}</small>{/if}
          </label>
        </div>
        <div class="settings-actions-row">
          <button class="secondary-lite" onclick={resetSafetySettings}>{tr("gui.settings.reset_defaults", "Reset defaults")}</button>
          <span>{settingsSnapshotLabel}</span>
        </div>
        <div class="setting-callout">
          <strong>{tr("gui.settings.security.path_safety_always_on", "Path safety is always on")}</strong>
          <span>{tr("gui.settings.security.path_safety_always_on_body", "Squallz blocks path traversal and link escapes independently of these resource limits.")}</span>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "settingsPerformance"}
  <div class="settings-view modern-settings-performance">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.performance.eyebrow", "Settings / Performance")}</span>
        <h1>{tr("gui.settings.performance.title", "Performance and scale behavior")}</h1>
        <p>{tr("gui.settings.performance.subtitle", "Balance simultaneous tasks, encoder threads, and Squallz-owned stream buffers. Automatic scheduling stays within the available CPU budget.")}</p>
      </div>
      <SettingsSaveAction
        state={performanceSaveState}
        statusLabel={settingsSaveStatusLabel("performance", performanceSaveState)}
        actionLabel={tr("gui.settings.performance.save", "Save performance")}
        savingLabel={tr("gui.settings.save_state.saving", "Saving…")}
        disabledReason={settingsSaveDisabledReason("performance", performanceSettingsDirty, performanceValidationError)}
        icon="hourglass"
        onSave={() => void savePerformanceSettings()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="settings-metric-grid performance-metric-grid">
          <div class:invalid-setting={Boolean(performanceParallelJobsError)}><span>{tr("gui.settings.performance.parallel_tasks", "Parallel tasks")}</span><strong>{performanceParallelJobs === null ? tr("common.auto", "Auto") : formattedDraftNumber(performanceParallelJobs)}</strong><small>{tr("gui.settings.performance.parallel_tasks_hint", "CPU-aware queue limit")}</small></div>
          <div class:invalid-setting={Boolean(performanceThreadsError)}><span>{tr("gui.settings.performance.workers", "Encoder threads")}</span><strong>{performanceThreads === null ? tr("common.auto", "Auto") : formattedDraftNumber(performanceThreads)}</strong><small>{tr("gui.settings.performance.workers_hint", "Per supported archive task")}</small></div>
          <div class:invalid-setting={Boolean(performanceMemoryError)}><span>{tr("gui.settings.performance.stream_buffer", "Stream buffer")}</span><strong>{performanceMemoryKiB === null ? tr("common.auto", "Auto") : streamBufferKiBLabel(performanceMemoryKiB)}</strong><small>{tr("gui.settings.performance.copy_buffers", "Supported Squallz buffers")}</small></div>
        </div>
        <div class="level-control settings-slider">
          <div><strong>{tr("gui.settings.performance.parallel_tasks", "Parallel tasks")} · {performanceParallelJobs === null ? tr("common.auto", "Auto") : formattedDraftNumber(performanceParallelJobs)}</strong><span>{tr("gui.settings.performance.parallel_tasks_body", "Automatic scheduling uses one task per four logical CPU threads, up to four. Heavy automatic-thread jobs run alone; lighter tasks share spare capacity.")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.parallel_tasks", "Parallel tasks")}>
            <button class:active={performanceParallelJobs === null} aria-pressed={performanceParallelJobs === null} onclick={() => choosePerformanceParallelJobs(null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={performanceParallelJobs === 1} aria-pressed={performanceParallelJobs === 1} onclick={() => choosePerformanceParallelJobs(1)}>1</button>
            <button class:active={performanceParallelJobs === 2} aria-pressed={performanceParallelJobs === 2} onclick={() => choosePerformanceParallelJobs(2)}>2</button>
            <button class:active={performanceParallelJobs === 4} aria-pressed={performanceParallelJobs === 4} onclick={() => choosePerformanceParallelJobs(4)}>4</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_parallel_jobs", "Custom parallel tasks")}</span>
            <input
              id="settings-performance-parallel-jobs"
              class:invalid={Boolean(performanceParallelJobsError)}
              type="number"
              min="1"
              max="8"
              step="1"
              value={performanceParallelJobs ?? ""}
              aria-invalid={Boolean(performanceParallelJobsError)}
              aria-describedby={performanceParallelJobsError ? "settings-performance-parallel-jobs-error" : undefined}
              oninput={(event) => setPerformanceParallelJobs(inputNumber(event))}
            />
            {#if performanceParallelJobsError}<small id="settings-performance-parallel-jobs-error" class="settings-field-error" role="status">{performanceParallelJobsError}</small>{/if}
          </label>
          <div><strong>{tr("gui.settings.performance.worker_threads", "Encoder threads per task")} · {performanceThreads === null ? tr("common.auto", "Auto") : formattedDraftNumber(performanceThreads)}</strong><span>{tr("gui.settings.performance.worker_threads_body", "Multithreaded encoders honor the manual limit; Automatic gives them available CPU while single-thread formats reserve one thread.")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.worker_threads", "Worker threads")}>
            <button class:active={performanceThreads === null} aria-pressed={performanceThreads === null} onclick={() => choosePerformanceThreads(null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={performanceThreads === 4} aria-pressed={performanceThreads === 4} onclick={() => choosePerformanceThreads(4)}>4</button>
            <button class:active={performanceThreads === 8} aria-pressed={performanceThreads === 8} onclick={() => choosePerformanceThreads(8)}>8</button>
            <button class:active={performanceThreads === 16} aria-pressed={performanceThreads === 16} onclick={() => choosePerformanceThreads(16)}>16</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_threads", "Custom threads")}</span>
            <input
              id="settings-performance-threads"
              class:invalid={Boolean(performanceThreadsError)}
              type="number"
              min="1"
              max="64"
              step="1"
              value={performanceThreads ?? ""}
              aria-invalid={Boolean(performanceThreadsError)}
              aria-describedby={performanceThreadsError ? "settings-performance-threads-error" : undefined}
              oninput={(event) => setPerformanceThreads(inputNumber(event))}
            />
            {#if performanceThreadsError}<small id="settings-performance-threads-error" class="settings-field-error" role="status">{performanceThreadsError}</small>{/if}
          </label>
          <div><strong>{tr("gui.settings.performance.stream_buffer_memory", "Stream buffer memory")} · {performanceMemoryKiB === null ? tr("common.auto", "Auto") : streamBufferKiBLabel(performanceMemoryKiB)}</strong><span>{tr("gui.settings.performance.stream_buffer_body", "Sets the 8–64 KiB cap for supported Squallz-owned copy buffers; format tools may keep their own buffers and dictionaries.")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.stream_buffer_memory", "Stream buffer memory")}>
            <button class:active={performanceMemoryKiB === null} aria-pressed={performanceMemoryKiB === null} onclick={() => choosePerformanceMemory(null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={performanceMemoryKiB === 8} aria-pressed={performanceMemoryKiB === 8} onclick={() => choosePerformanceMemory(8)}>{streamBufferKiBLabel(8)}</button>
            <button class:active={performanceMemoryKiB === 16} aria-pressed={performanceMemoryKiB === 16} onclick={() => choosePerformanceMemory(16)}>{streamBufferKiBLabel(16)}</button>
            <button class:active={performanceMemoryKiB === 32} aria-pressed={performanceMemoryKiB === 32} onclick={() => choosePerformanceMemory(32)}>{streamBufferKiBLabel(32)}</button>
            <button class:active={performanceMemoryKiB === 64} aria-pressed={performanceMemoryKiB === 64} onclick={() => choosePerformanceMemory(64)}>{streamBufferKiBLabel(64)}</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_buffer_kib", "Custom buffer KiB")}</span>
            <input
              id="settings-performance-memory"
              class:invalid={Boolean(performanceMemoryError)}
              type="number"
              min="8"
              max="64"
              step="1"
              value={performanceMemoryKiB ?? ""}
              aria-invalid={Boolean(performanceMemoryError)}
              aria-describedby={performanceMemoryError ? "settings-performance-memory-error" : undefined}
              oninput={(event) => setPerformanceMemoryKiB(inputNumber(event))}
            />
            {#if performanceMemoryError}<small id="settings-performance-memory-error" class="settings-field-error" role="status">{performanceMemoryError}</small>{/if}
          </label>
          <div class="settings-actions-row">
            <button class="secondary-lite" onclick={resetPerformanceSettings}>{tr("gui.settings.performance.use_auto", "Use auto")}</button>
            <span>{settingsSnapshotLabel}</span>
          </div>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "passwordBook"}
  <div class="settings-view modern-password-book">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.password_book.eyebrow", "Settings / Password Book")}</span>
        <h1>{tr("gui.settings.password_book.title", "Saved archive passwords")}</h1>
        <p>{tr("gui.settings.password_book.subtitle", "Squallz stores saved archive passwords only through the system secret store boundary.")}</p>
      </div>
      <button
        class="sheet-action"
        disabled={Boolean(passwordBookForgetDisabledReason())}
        title={passwordBookForgetDisabledReason()}
        aria-label={labelWithDisabledReason(
          tr("gui.settings.password_book.forget_current", "Forget current archive"),
          passwordBookForgetDisabledReason(),
        )}
        onclick={() => void forgetPasswordBookPanel()}
      ><Icon name="lock" size={17} />{tr("gui.settings.password_book.forget_current", "Forget current archive")}</button>
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="password-book-grid">
          <div><span>{tr("gui.settings.password_book.secret_store", "Secret store")}</span><strong>{passwordBookSecretStoreLabel()}</strong><small>{tr("gui.settings.password_book.secret_store_detail", "{platform} uses {secretStore} when it is available").replace("{platform}", platformNameLabel()).replace("{secretStore}", secretStoreLabel())}</small></div>
          <div><span>{tr("gui.settings.password_book.current_archive", "Current archive")}</span><strong>{passwordBookCurrentLabel()}</strong><small>{passwordBookDetailLabel()}</small></div>
          <div><span>{tr("gui.settings.password_book.saved_secret_access", "Saved secret access")}</span><strong>{tr("gui.settings.password_book.status_only", "Status only")}</strong><small>{tr("gui.settings.password_book.saved_secret_never_returns", "Saved secret values never return to the interface")}</small></div>
        </div>
        <div class="settings-actions-row">
          <button
            class="primary-lite"
            disabled={Boolean(passwordBookRefreshDisabledReason())}
            aria-busy={passwordBookStatusState === "checking"}
            title={passwordBookRefreshDisabledReason()}
            aria-label={labelWithDisabledReason(
              tr("gui.settings.password_book.refresh_status", "Refresh status"),
              passwordBookRefreshDisabledReason(),
            )}
            onclick={() => void refreshPasswordBookPanel()}
          >{tr("gui.settings.password_book.refresh_status", "Refresh status")}</button>
          <span>{currentArchiveName()}</span>
        </div>
        <div class="limits-table">
          <div><b>{tr("gui.settings.password_book.source", "Source")}</b><b>{tr("gui.settings.password_book.priority", "Priority")}</b><b>{tr("gui.settings.password_book.stored_where", "Stored where")}</b><b>{tr("gui.settings.password_book.failure_behavior", "Failure behavior")}</b></div>
          <div><span>{tr("gui.settings.password_book.manual_prompt", "Manual prompt")}</span><span>1</span><span>{tr("gui.settings.password_book.transient_input", "Transient input")}</span><strong>{tr("gui.settings.password_book.retry_prompt", "Retry prompt")}</strong></div>
          <div><span>{tr("gui.settings.password_book.session_cache", "Session cache")}</span><span>2</span><span>{tr("gui.settings.password_book.process_memory", "Process memory")}</span><strong>{tr("gui.settings.password_book.cleared_on_exit", "Cleared when Squallz exits")}</strong></div>
          <div><span>{secretStoreLabel()}</span><span>3</span><span>{tr("gui.settings.password_book.system_secret_store", "System secret store")}</span><strong>{tr("gui.settings.password_book.fallback_prompt", "Fallback to prompt")}</strong></div>
        </div>
        <div class="setting-callout">
          <strong>{tr("gui.settings.password_book.no_plaintext_storage_title", "Saved passwords stay out of app files and logs")}</strong>
          <span>{tr("gui.settings.password_book.no_plaintext_storage_body", "Squallz never displays or exports saved password material.")}</span>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "integration"}
  <div class="integration-view modern-integration">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.integration.eyebrow", "Settings / Formats & Integration")}</span>
        <h1>{tr("gui.settings.integration.title", "Archive formats and {fileManager} actions").replace("{fileManager}", fileManagerLabel())}</h1>
        <p>{tr("gui.settings.integration.subtitle", "See which formats Squallz can use and check the optional {fileManager} actions installed on this device. Default apps are not changed here.").replace("{fileManager}", fileManagerLabel())}</p>
      </div>
      <div class="sheet-action-row integration-actions">
        <button class="sheet-action secondary-action" disabled={integrationBusy()} aria-busy={integrationOperation === "checking"} onclick={() => void refreshIntegrationStatus()}><Icon name="search" size={17} />{tr("gui.common.refresh", "Refresh")}</button>
        <button
          class="primary sheet-action"
          disabled={Boolean(integrationApplyDisabledReason())}
          title={integrationApplyDisabledReason()}
          aria-label={labelWithDisabledReason(integrationApplyLabel(), integrationApplyDisabledReason())}
          aria-busy={integrationOperation === "repairing"}
          onclick={() => void applyIntegrationChanges()}
        ><Icon name="rotate-cw" size={17} />{integrationApplyLabel()}</button>
      </div>
    </div>

    <div class="integration-layout">
      <section class="association-panel">
        <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.settings.integration.file_types", "Archive format capabilities")}</div>
        <div class="association-tools">
          <div class="association-search">
            <Icon name="search" size={13} />
            <input
              type="search"
              value={associationFilter}
              placeholder={tr("gui.settings.integration.filter_extensions", "Filter extensions or formats")}
              aria-label={tr("gui.settings.integration.filter_extensions", "Filter extensions or formats")}
              oninput={(event) => (associationFilter = event.currentTarget.value)}
            />
            {#if associationFilter}
              <button type="button" aria-label={tr("gui.settings.integration.clear_filter", "Clear file type filter")} title={tr("gui.settings.integration.clear_filter", "Clear file type filter")} onclick={clearAssociationFilter}><Icon name="x-circle" size={13} /></button>
            {/if}
          </div>
          <div class="assoc-count">{formatRegistrySourceLabel()} · {tr("gui.settings.integration.showing_filtered_rows", "Showing {shown} of {total}").replace("{shown}", String(visibleAssociationRows().length)).replace("{total}", String(associationRows().length))}</div>
        </div>
        <div class="assoc-chip-row">
          {#each associationSummary() as item}
            <span>{item}</span>
          {/each}
        </div>
        <div
          class="association-volume-guide"
          aria-label={tr("gui.settings.integration.volume_guide_label", "How volume types differ")}
        >
          <div class="native">
            <Icon name="archive" size={17} />
            <span>
              <strong>{tr("gui.settings.integration.native_volumes_title", "Format-native volumes")}</strong>
              <small>{tr("gui.settings.integration.native_volumes_body", "ZIP .z01/.zip and RAR partN.rar carry format-specific disk metadata. They are not interchangeable with .001 byte splits.")}</small>
            </span>
          </div>
          <div class="generic">
            <Icon name="repeat" size={17} />
            <span>
              <strong>{tr("gui.settings.integration.generic_volumes_title", "Generic .001 byte volumes")}</strong>
              <small>{tr("gui.settings.integration.generic_volumes_body", "Squallz can split supported outputs into a continuous .001/.002 sequence. This does not create native ZIP, RAR, or Split WIM volumes.")}</small>
            </span>
          </div>
        </div>
        <div class="association-table">
          <div class="assoc-head">
            <span>{tr("common.type", "Type")}</span>
            <span>{tr("common.format", "Format")}</span>
            <span>{tr("gui.settings.integration.archive_access", "Archive access")}</span>
            <span>{tr("gui.settings.integration.volume_support", "Volume sets")}</span>
            <span>{tr("common.action", "Action")}</span>
          </div>
          {#each visibleAssociationRows() as row}
            <div class="assoc-row">
              <strong>{row.ext}</strong>
              <span>{row.format}</span>
              <span class={`assoc-capability tone-${row.accessTone}`}>{row.access}</span>
              <span class={`assoc-capability tone-${row.volumeTone}`}>{row.volumes}</span>
              <span>{row.action}</span>
            </div>
          {:else}
            <div class="association-empty" role="status">
              <strong>{tr("gui.settings.integration.no_matching_types", "No matching file types")}</strong>
              <span>{tr("gui.settings.integration.no_matching_types_body", "Try an extension such as .zip or a format name such as 7Z.")}</span>
              <button type="button" onclick={clearAssociationFilter}>{tr("gui.settings.integration.clear_filter", "Clear file type filter")}</button>
            </div>
          {/each}
        </div>
      </section>

      <IntegrationHealthPanel
        panelTitle={tr("gui.settings.integration.file_manager_action_files", "{fileManager} action files").replace("{fileManager}", fileManagerLabel())}
        healthTitle={tr("gui.settings.integration.health_title", "Integration health")}
        platform={platformNameLabel()}
        scope={integrationScopeLabel()}
        summary={integrationSummaryLabel()}
        detail={integrationDetailLabel()}
        state={integrationPanelState()}
        busy={integrationBusy()}
        actions={integrationActionViews()}
        diagnosticsTitle={tr("gui.settings.integration.system_diagnostics_title", "Runtime and system diagnostics")}
        diagnostics={integrationDiagnosticViews()}
        onDiagnosticAction={(target) => void openIntegrationGuide(target)}
        removeLabel={tr("gui.settings.integration.uninstall_actions", "Uninstall actions")}
        removeDisabledReason={integrationRemoveDisabledReason()}
        removeAriaLabel={labelWithDisabledReason(
          tr("gui.settings.integration.uninstall_actions", "Uninstall actions"),
          integrationRemoveDisabledReason(),
        )}
        onRemove={() => void removeIntegrationChanges()}
      />
    </div>
  </div>
{/if}
