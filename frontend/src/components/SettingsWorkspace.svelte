<script lang="ts" module>
  import type { CssVariableMap } from "../lib/css-variables";
  import type { SettingsSession, SettingsSaveState, PersistedSettingsSection } from "../lib/settings-session.svelte";
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
    ResolvedTheme,
    Screen,
  } from "../lib/ui-model";
  import type { UiMode } from "../lib/uiMode.svelte";
  import type { PasswordSaveState } from "../lib/archive.svelte";

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
    settings: SettingsSession;
    onReady?: () => void;
    screen: SettingsScreen;
    tr: (key: string, fallback?: string) => string;
    appearanceSaveState: Exclude<SettingsSaveState, "dirty">;
    modernModeSelected: boolean;
    classicModeSelected: boolean;
    setMode: (mode: UiMode) => void;
    activeThemeChoice: "system" | "light" | "dark";
    setTheme: (theme: "system" | "light" | "dark") => void;
    activeDensityChoice: DensityChoice;
    setDensity: (density: DensityChoice) => void;
    activeTheme: ResolvedTheme;
    availableLanguages: LanguageDto[];
    chooseDefaultCreateFolder: () => void | Promise<void>;
    chooseDefaultExtractFolder: () => void | Promise<void>;
    fileManagerLabel: () => string;
    openWithLabel: () => string;
    updateCheckPreview: UpdateCheckPreview;
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
    passwordSaveState: PasswordSaveState;
    passwordBookArchiveId: number | null;
    passwordBookCanSave: boolean;
    passwordBookSaved: boolean;
    savePasswordBook: (password: string) => Promise<boolean>;
    cancelPasswordBookSave: () => Promise<void>;
    clearPasswordBookSave: () => void;
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
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import { tError } from "../lib/i18n.svelte";
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
    settings,
    screen,
    onReady,
    tr,
    appearanceSaveState,
    modernModeSelected,
    classicModeSelected,
    setMode,
    activeThemeChoice,
    setTheme,
    activeDensityChoice,
    setDensity,
    activeTheme,
    availableLanguages,
    chooseDefaultCreateFolder,
    chooseDefaultExtractFolder,
    fileManagerLabel,
    openWithLabel,
    updateCheckPreview,
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
    passwordSaveState,
    passwordBookArchiveId,
    passwordBookCanSave,
    passwordBookSaved,
    savePasswordBook,
    cancelPasswordBookSave,
    clearPasswordBookSave,
    refreshPasswordBookPanel,
    currentArchiveName,
    formatRegistry,
    formatRegistryLoaded,
    initialIntegrationStatus,
    initialIntegrationDiagnostics,
    showMacosIntegrationDiagnostics,
    onNotice,
  }: SettingsWorkspaceProps = $props();

  let passwordValue = $state("");
  let passwordInput = $state<HTMLInputElement | null>(null);
  let passwordEmptyError = $state(false);
  let passwordFormGeneration = 0;
  const passwordFormIdentity = $derived(screen === "passwordBook" ? `password-book:${passwordBookArchiveId}` : null);
  let passwordSaveBusy = $derived(passwordSaveState.phase === "verifying" || passwordSaveState.phase === "cancelling");
  let passwordSaveError = $derived(passwordEmptyError
    ? tr("gui.password.empty_error")
    : passwordSaveState.error?.key === "error.wrong_password"
      ? tr("gui.password.wrong")
      : passwordSaveState.error ? tError(passwordSaveState.error) : "");

  $effect(() => {
    // Clear sensitive input and invalidate work when this settings surface changes.
    void passwordFormIdentity;
    untrack(() => {
      passwordValue = "";
      passwordEmptyError = false;
      passwordFormGeneration += 1;
      clearPasswordBookSave();
    });
  });

  onMount(() => onReady?.());

  onDestroy(() => {
    passwordFormGeneration += 1;
    clearPasswordBookSave();
  });

  async function submitPasswordBook(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (passwordSaveBusy || !passwordBookCanSave) return;
    passwordEmptyError = passwordValue.length === 0;
    if (passwordEmptyError) {
      passwordInput?.focus();
      return;
    }
    const generation = passwordFormGeneration;
    const pending = savePasswordBook(passwordValue);
    passwordValue = "";
    const saved = await pending;
    await tick();
    if (!saved && screen === "passwordBook" && generation === passwordFormGeneration) passwordInput?.focus();
  }

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
    buildCustomPaletteData(settings.colors.accent, activeTheme, settings.colors.contrastGuard),
  );
  const activePaletteData = $derived<Palette>(
    settings.colors.palette === "custom"
      ? customPaletteData
      : palettes.find((palette) => palette.id === settings.colors.palette) ?? palettes[0],
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
      ? tr("gui.settings.integration.format_registry")
      : tr("gui.settings.integration.preview_registry");
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
      return tr("gui.settings.integration.format_alias").replace("{format}", "ZIP");
    }
    if (format.id === "rar" && ext !== "rar") {
      return tr("gui.settings.integration.format_alias").replace("{format}", "RAR");
    }
    if (format.id.startsWith("tar.") && ext !== format.id) {
      return tr("gui.settings.integration.format_alias").replace("{format}", display);
    }
    if (format.kind === "compressor") {
      return tr("gui.settings.integration.format_stream").replace("{format}", display);
    }
    return display;
  }

  function associationAccess(
    format: FormatDto,
    extension: string,
  ): { label: string; tone: AssociationRow["accessTone"] } {
    if (format.kind === "compressor") {
      return {
        label: tr("gui.settings.integration.stream_support"),
        tone: "neutral",
      };
    }
    if (format.id === "wim" && extension.toLowerCase() === "swm") {
      return {
        label: tr("gui.settings.integration.read_write"),
        tone: "success",
      };
    }
    if (format.id === "wim") {
      return {
        label: tr("gui.settings.integration.external_tools"),
        tone: "info",
      };
    }
    if (!format.can_create && format.can_extract) {
      return {
        label: tr("gui.settings.integration.read_only"),
        tone: "info",
      };
    }
    return {
      label: tr("gui.settings.integration.read_write"),
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
        label: tr("gui.settings.integration.volume_rar_native_read"),
        tone: "info",
      };
    }
    if (format.id === "wim") {
      return {
        label: tr("gui.settings.integration.volume_split_wim_read"),
        tone: "success",
      };
    }
    if (format.id === "zip" && ext === "zip") {
      return {
        label: tr("gui.settings.integration.volume_zip_mixed"),
        tone: "success",
      };
    }
    if (format.id === "sqz") {
      return {
        label: tr("gui.settings.integration.volume_sqz"),
        tone: "success",
      };
    }
    if (format.can_split) {
      return {
        label: tr("gui.settings.integration.volume_generic"),
        tone: "success",
      };
    }
    return {
      label: tr("gui.settings.integration.volume_single"),
      tone: "neutral",
    };
  }

  function associationActionLabel(format: FormatDto, extension: string): string {
    const ext = extension.toLowerCase();
    if (format.id === "sqz") {
      return tr("gui.settings.integration.action_browse_extract_test_export");
    }
    if (format.id === "wim") {
      return ext === "swm"
        ? tr("gui.settings.integration.action_split_wim_read")
        : tr("gui.settings.integration.action_wim");
    }
    if (format.id === "rar") {
      return ext === "cbr"
        ? tr("gui.settings.integration.action_comics_7zz")
        : tr("gui.settings.integration.action_7zz_bridge");
    }
    if (longTailBridgeFormatIds.has(format.id)) {
      return tr("gui.settings.integration.action_7zz_bridge");
    }
    if (format.id === "7z") {
      return tr("gui.settings.integration.action_browse_extract_convert");
    }
    if (format.kind === "compressor") {
      return tr("gui.settings.integration.action_decompress_stream");
    }
    if (ext === "cbz") {
      return tr("gui.settings.integration.action_browse_comics_extract");
    }
    if (format.id.startsWith("tar.")) {
      return tr("gui.settings.integration.action_extract_convert");
    }
    return tr("gui.settings.integration.action_browse_extract_test");
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
        format: tr("gui.settings.integration.par2_sidecar"),
        access: tr("gui.settings.integration.sidecar"),
        accessTone: "neutral",
        volumes: tr("gui.settings.integration.volume_protects_sets"),
        volumeTone: "info",
        action: tr("gui.settings.integration.verify_repair"),
      },
      {
        ext: ".001",
        format: tr("gui.settings.integration.generic_split_volume"),
        access: tr("gui.settings.integration.read_write"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_generic_named"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_generic_split"),
      },
      {
        ext: ".z01",
        format: tr("gui.settings.integration.zip_native_volume"),
        access: tr("gui.settings.integration.read_write"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_zip_native"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_zip_native"),
      },
      {
        ext: ".r00",
        format: tr("gui.settings.integration.rar_native_volume"),
        access: tr("gui.settings.integration.read_only"),
        accessTone: "info",
        volumes: tr("gui.settings.integration.volume_rar_native"),
        volumeTone: "info",
        action: tr("gui.settings.integration.action_rar_native_read"),
      },
    );
    if (!seen.has("swm")) {
      rows.push({
        ext: ".swm",
        format: tr("gui.settings.integration.split_wim"),
        access: tr("gui.settings.integration.read_write"),
        accessTone: "success",
        volumes: tr("gui.settings.integration.volume_split_wim_read"),
        volumeTone: "success",
        action: tr("gui.settings.integration.action_split_wim_read"),
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
      tr("gui.settings.integration.summary_registry_extensions")
        .replace("{count}", String(registryFormatExtensions().length)),
      tr("gui.settings.integration.summary_archive_families")
        .replace("{count}", String(archiveFormats.length)),
      tr("gui.settings.integration.summary_writable")
        .replace("{count}", String(writableArchives)),
      tr("gui.settings.integration.summary_unpack_only")
        .replace("{count}", String(unpackOnlyArchives)),
      tr("gui.settings.integration.summary_sidecar_rules"),
    ];
  }

  function integrationActionLabel(action: DesktopIntegrationAction): string {
    const copy = externalOpenActionCopy(action);
    return tr(copy.labelKey, copy.fallbackLabel);
  }

  function integrationApplyLabel(): string {
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_actions");
    }
    if (integrationSnapshot?.health === "healthy") {
      return tr("gui.settings.integration.reinstall_actions");
    }
    if (integrationSnapshot?.health === "needs_repair" || integrationUnavailable) {
      return tr("gui.settings.integration.repair_actions");
    }
    return tr("gui.settings.integration.install_platform_actions")
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
      return tr("gui.settings.integration.checking_status")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_status")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationOperation === "removing") {
      return tr("gui.settings.integration.removing_status")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") {
      return tr("gui.settings.integration.status_unavailable")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (!integrationSnapshot) {
      return tr("gui.settings.integration.status_not_checked");
    }
    if (integrationSnapshot.health === "missing") {
      return tr("gui.settings.integration.platform_actions_not_installed")
        .replace("{fileManager}", fileManagerLabel());
    }
    const counts = integrationHealthCounts();
    if (integrationSnapshot.health === "needs_repair") {
      return tr("gui.settings.integration.actions_need_repair")
        .replace("{fileManager}", fileManagerLabel())
        .replace("{count}", String(counts.missing + counts.damaged));
    }
    return tr("gui.settings.integration.actions_verified")
      .replace("{fileManager}", fileManagerLabel())
      .replace("{count}", String(counts.healthy));
  }

  function integrationDetailLabel(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.checking_detail");
    }
    if (integrationOperation === "repairing") {
      return tr("gui.settings.integration.repairing_detail");
    }
    if (integrationOperation === "removing") {
      return tr("gui.settings.integration.removing_detail");
    }
    if (integrationUnavailable || integrationSnapshot?.health === "unavailable") {
      return tr("gui.settings.integration.status_help", "No previous result is shown. Refresh or try Repair.");
    }
    if (!integrationSnapshot || integrationSnapshot.health === "missing") {
      return tr("gui.settings.integration.install_detail");
    }
    if (integrationSnapshot.health === "needs_repair") {
      const counts = integrationHealthCounts();
      return tr("gui.settings.integration.health_counts")
        .replace("{healthy}", String(counts.healthy))
        .replace("{missing}", String(counts.missing))
        .replace("{damaged}", String(counts.damaged));
    }
    const folder = pathBaseName(integrationSnapshot.services_dir);
    return folder
      ? tr("gui.settings.integration.verified_in")
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
    if (state === "healthy") return tr("gui.settings.integration.action_healthy");
    if (state === "damaged") return tr("gui.settings.integration.action_damaged");
    if (state === "missing") return tr("gui.settings.integration.action_missing");
    if (state === "checking") return tr("gui.settings.integration.action_checking");
    return tr("gui.settings.integration.action_unavailable");
  }

  function integrationActionDetail(action: DesktopIntegrationAction): string | null {
    if (integrationActionState(action) !== "damaged") return null;
    const issue = integrationActionSnapshot(action)?.issue;
    if (issue === "script_missing") {
      return tr("gui.settings.integration.issue_script_missing");
    }
    if (issue === "launcher_missing") {
      return tr("gui.settings.integration.issue_launcher_missing");
    }
    if (issue === "script_not_executable") {
      return tr("gui.settings.integration.issue_script_not_executable");
    }
    if (issue === "script_outdated") {
      return tr("gui.settings.integration.issue_script_outdated");
    }
    if (issue === "launcher_outdated") {
      return tr("gui.settings.integration.issue_launcher_outdated");
    }
    if (issue === "registry_missing") {
      return tr("gui.settings.integration.issue_registry_missing");
    }
    if (issue === "registry_outdated") {
      return tr("gui.settings.integration.issue_registry_outdated");
    }
    return tr("gui.settings.integration.issue_unknown");
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
    ).replace("{fileManager}", fileManagerLabel());
  }

  function integrationRemoveDisabledReason(): string {
    if (integrationBusy()) {
      return tr("gui.settings.integration.wait_for_action");
    }
    if (!integrationSnapshot || integrationUnavailable) {
      return tr("gui.settings.integration.check_before_removing");
    }
    if (!integrationSnapshot.can_remove) {
      return tr("gui.settings.integration.nothing_to_remove");
    }
    return "";
  }

  function integrationApplyDisabledReason(): string {
    if (integrationBusy()) {
      return tr("gui.settings.integration.wait_for_action");
    }
    if (integrationSnapshot && !integrationSnapshot.can_repair) {
      return tr("gui.settings.integration.repair_unavailable");
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
          ? tr("gui.settings.integration.applied_title")
          : tr("gui.settings.integration.repaired_title"),
        detail: complete
          ? tr("gui.settings.integration.actions_verified")
              .replace("{count}", String(counts.healthy))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.repair_incomplete")
              .replace("{count}", String(counts.missing + counts.damaged)),
      });
      onNotice(
        complete
          ? tr("gui.settings.integration.repair_complete")
              .replace("{count}", String(counts.healthy))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.repair_incomplete")
              .replace("{count}", String(counts.missing + counts.damaged)),
      );
    } catch {
      clearIntegrationStatusSnapshot();
      const detail = tr(
        "gui.settings.integration.repair_verify_failed",
      );
      recordOperation({
        status: "info",
        title: previousHealth === "missing" || previousHealth === null
          ? tr("gui.settings.integration.applied_title")
          : tr("gui.settings.integration.repaired_title"),
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
            ? tr("gui.settings.integration.actions_verified")
                .replace("{count}", String(counts.healthy))
                .replace("{fileManager}", fileManagerLabel())
            : result.health === "needs_repair"
              ? tr("gui.settings.integration.actions_need_repair")
                  .replace("{count}", String(counts.missing + counts.damaged))
                  .replace("{fileManager}", fileManagerLabel())
              : result.health === "missing"
                ? tr("gui.settings.integration.platform_actions_not_installed_short")
                    .replace("{fileManager}", fileManagerLabel())
                : tr("gui.settings.integration.status_unavailable")
                    .replace("{fileManager}", fileManagerLabel()),
        );
      }
    } catch {
      clearIntegrationStatusSnapshot();
      if (announce) {
        onNotice(
          tr(
            "gui.settings.integration.status_check_failed",
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
        title: tr("gui.settings.integration.removed_title"),
        detail: complete
          ? tr("gui.settings.integration.removed_count")
              .replace("{count}", String(result.removed.length))
          : tr("gui.settings.integration.remove_incomplete"),
      });
      onNotice(
        complete
          ? tr("gui.settings.integration.removed_platform_count")
              .replace("{count}", String(result.removed.length))
              .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.remove_incomplete"),
      );
    } catch {
      clearIntegrationStatusSnapshot();
      const detail = tr(
        "gui.settings.integration.remove_verify_failed",
      );
      recordOperation({
        status: "info",
        title: tr("gui.settings.integration.removed_title"),
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
          ? tr("gui.settings.integration.setup_guide_opened")
          : target === "wimlib"
            ? tr("gui.settings.integration.wimlib_setup_guide_opened")
          : target === "unrar"
            ? tr("gui.settings.integration.unrar_setup_guide_opened")
            : tr("gui.settings.integration.guide_opened"),
      );
    } catch {
      onNotice(
        target === "sevenzip"
          ? tr("gui.settings.integration.setup_guide_open_failed")
          : target === "wimlib"
            ? tr("gui.settings.integration.wimlib_setup_guide_open_failed")
          : target === "unrar"
            ? tr("gui.settings.integration.unrar_setup_guide_open_failed")
            : tr("gui.settings.integration.guide_open_failed"),
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
      return tr("gui.settings.integration.backend_source_application");
    }
    if (backend?.source === "environment") {
      return tr("gui.settings.integration.backend_source_environment");
    }
    if (backend?.source === "path") {
      return tr("gui.settings.integration.backend_source_path");
    }
    return tr("gui.settings.integration.backend_source_runtime");
  }

  function integrationBackendStateLabel(backend: RuntimeBackend | undefined): string {
    if (backend?.available) return tr("gui.settings.integration.backend_found");
    if (backend?.configured) {
      return tr("gui.settings.integration.backend_misconfigured");
    }
    if (backend) return tr("gui.settings.integration.backend_missing");
    if (integrationDiagnostics !== null || integrationDiagnosticsUnavailable) {
      return tr("gui.settings.integration.system_unavailable");
    }
    return tr("gui.settings.integration.system_checking");
  }

  function integrationDefaultHandlerStateLabel(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.system_checking");
    }
    if (integrationDiagnosticsUnavailable || !integrationDiagnostics) {
      return tr("gui.settings.integration.system_unavailable");
    }
    const state = integrationDiagnostics.default_handlers.state;
    if (state === "squallz") return tr("gui.app.name");
    if (state === "mixed") {
      return tr("gui.settings.integration.default_handlers_mixed");
    }
    if (state === "other") {
      return tr("gui.settings.integration.default_handlers_other");
    }
    if (state === "unknown") {
      return tr("gui.settings.integration.default_handlers_unknown");
    }
    return tr("gui.settings.integration.system_unavailable");
  }

  function integrationDefaultHandlerDetail(): string {
    if (integrationOperation === "checking") {
      return tr("gui.settings.integration.default_handlers_checking");
    }
    const summary = integrationDiagnostics?.default_handlers;
    if (integrationDiagnosticsUnavailable || !summary || summary.state === "unavailable") {
      return tr("gui.settings.integration.default_handlers_unavailable");
    }

    let detail = tr("gui.settings.integration.default_handlers_count")
      .replace("{squallz}", String(summary.squallz))
      .replace("{total}", String(summary.total));
    if (summary.checked < summary.total) {
      detail += ` ${tr("gui.settings.integration.default_handlers_partial")
        .replace("{checked}", String(summary.checked))
        .replace("{total}", String(summary.total))}`;
    }
    const otherApplications = Array.from(new Set(summary.handlers.flatMap((handler) =>
      handler.state === "other" && handler.application_name ? [handler.application_name] : [],
    )));
    if (otherApplications.length > 0) {
      detail += ` ${tr("gui.settings.integration.default_handlers_apps")
        .replace("{apps}", otherApplications.join(", "))}`;
    }
    return detail;
  }

  function integrationVisibilityDetail(): string {
    if (integrationFinderActionsHealthy) {
      return tr("gui.settings.integration.finder_visibility_ready");
    }
    return tr("gui.settings.integration.finder_visibility_after_repair");
  }

  function integrationDiagnosticViews(): IntegrationDiagnosticView[] {
    const sevenZip = integrationDiagnostics?.backends.find((backend) => backend.id === "sevenzip");
    const sevenZipDetail = sevenZip?.available
      ? tr(
          "gui.settings.integration.sevenzip_ready_detail",
        )
          .replace("{source}", integrationBackendSourceLabel(sevenZip))
          .replace("{tool}", sevenZip.tool ?? "7zz/7z")
      : sevenZip?.configured
        ? tr(
            "gui.settings.integration.sevenzip_misconfigured_detail",
          )
        : sevenZip
          ? tr(
              "gui.settings.integration.sevenzip_missing_detail",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
              )
            : tr(
                "gui.settings.integration.sevenzip_checking_detail",
              );
    const diagnostics: IntegrationDiagnosticView[] = [{
      id: "sevenzip",
      label: tr("gui.settings.integration.sevenzip_backend_title"),
      stateLabel: integrationBackendStateLabel(sevenZip),
      detail: sevenZipDetail,
      tone: sevenZip?.available ? "info" : "warning",
      actionLabel: sevenZip && !sevenZip.available
        ? tr("gui.settings.integration.sevenzip_backend_guide")
        : null,
    }];

    const wimlib = integrationDiagnostics?.backends.find((backend) => backend.id === "wimlib");
    const wimlibDetail = wimlib?.available
        ? tr(
            "gui.settings.integration.wimlib_ready_detail",
          )
          .replace("{source}", integrationBackendSourceLabel(wimlib))
          .replace("{tool}", wimlib.tool ?? "wimlib-imagex")
      : wimlib?.configured
        ? tr(
            "gui.settings.integration.wimlib_misconfigured_detail",
          )
        : wimlib
          ? tr(
              "gui.settings.integration.wimlib_missing_detail",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
              )
            : tr(
                "gui.settings.integration.wimlib_checking_detail",
              );
    diagnostics.push({
      id: "wimlib",
      label: tr("gui.settings.integration.wimlib_backend_title"),
      stateLabel: integrationBackendStateLabel(wimlib),
      detail: wimlibDetail,
      tone: wimlib?.available ? "info" : "warning",
      actionLabel: wimlib && !wimlib.available
        ? tr("gui.settings.integration.wimlib_backend_guide")
        : null,
    });

    const unrar = integrationDiagnostics?.backends.find((backend) => backend.id === "unrar");
    const unrarReady = Boolean(unrar?.available && sevenZip?.available);
    const unrarState = unrar?.available && !sevenZip?.available
      ? tr("gui.settings.integration.unrar_waiting_sevenzip")
      : integrationBackendStateLabel(unrar);
    const unrarDetail = unrar?.available
      ? sevenZip?.available
        ? tr(
            "gui.settings.integration.unrar_ready_detail",
          )
            .replace("{source}", integrationBackendSourceLabel(unrar))
            .replace("{tool}", unrar.tool ?? "unrar")
        : tr(
            "gui.settings.integration.unrar_waiting_detail",
          ).replace("{tool}", unrar.tool ?? "unrar")
      : unrar?.configured
        ? tr(
            "gui.settings.integration.unrar_misconfigured_detail",
          )
        : unrar
          ? tr(
              "gui.settings.integration.unrar_missing_detail",
            )
          : integrationDiagnostics !== null || integrationDiagnosticsUnavailable
            ? tr(
                "gui.settings.integration.backend_unavailable_detail",
              )
            : tr(
                "gui.settings.integration.unrar_checking_detail",
              );
    diagnostics.push({
      id: "unrar",
      label: tr("gui.settings.integration.unrar_backend_title"),
      stateLabel: unrarState,
      detail: unrarDetail,
      tone: unrarReady ? "info" : unrar?.configured && !unrar.available ? "warning" : "neutral",
      actionLabel: unrar && !unrar.available
        ? tr("gui.settings.integration.unrar_backend_guide")
        : null,
    });

    if (!showMacosIntegrationDiagnostics) return diagnostics;
    const defaultHandlersState = integrationDiagnostics?.default_handlers.state;
    return diagnostics.concat([
      {
        id: "default_handlers",
        label: tr("gui.settings.integration.default_handlers_title"),
        stateLabel: integrationDefaultHandlerStateLabel(),
        detail: integrationDefaultHandlerDetail(),
        tone: integrationDiagnosticsUnavailable || defaultHandlersState === "unavailable"
          ? "warning"
          : defaultHandlersState === "mixed" || defaultHandlersState === "squallz"
            ? "info"
            : "neutral",
        actionLabel: tr("gui.settings.integration.default_handlers_guide"),
      },
      {
        id: "finder_visibility",
        label: tr("gui.settings.integration.finder_visibility_title"),
        stateLabel: tr("gui.settings.integration.finder_visibility_manual"),
        detail: integrationVisibilityDetail(),
        tone: "neutral",
        actionLabel: tr("gui.settings.integration.finder_visibility_guide"),
      },
    ]);
  }

  function settingsSaveStateLabel(state: SettingsSaveState): string {
    if (state === "saving") return tr("gui.settings.save_state.saving");
    if (state === "dirty") return tr("gui.settings.save_state.dirty");
    if (state === "session") return tr("gui.settings.save_state.session");
    if (state === "error") return tr("gui.settings.save_state.error");
    return tr("gui.settings.save_state.saved");
  }

  function settingsSaveStatusLabel(
    section: PersistedSettingsSection,
    state: SettingsSaveState,
  ): string {
    return state !== "saved" && settings.saveTarget !== null && settings.saveTarget !== section
      ? tr("gui.settings.save_in_progress")
      : settingsSaveStateLabel(state);
  }

  function settingsSaveDisabledReason(
    section: PersistedSettingsSection,
    dirty: boolean,
    validationError = "",
  ): string {
    if (validationError) return validationError;
    if (!dirty) return tr("gui.settings.no_changes");
    if (settings.saveTarget !== null && settings.saveTarget !== section) {
      return tr("gui.settings.save_in_progress");
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

  function activePaletteName(): string {
    return paletteName(activePaletteData);
  }

  function customThemePreviewVariables(theme: ResolvedTheme): CssVariableMap {
    return deriveCustomPaletteTokens(settings.colors.accent, theme, settings.colors.contrastGuard);
  }

  function colorWheelCssVariables(): CssVariableMap {
    return {
      ...customThemePreviewVariables(activeTheme),
      ...colorWheelVariablesForAccent(settings.colors.accent),
    };
  }

  function updateCustomAccentFromWheel(event: PointerEvent | MouseEvent): void {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const size = Math.min(rect.width, rect.height);
    const x = Math.max(0, Math.min(size, event.clientX - rect.left));
    const y = Math.max(0, Math.min(size, event.clientY - rect.top));
    settings.updateCustomAccent(
      colorFromWheelPointForAccent(settings.colors.accent, x, y, size),
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
    const hsl = colorWheelHslForAccent(settings.colors.accent);
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
    settings.updateCustomAccent(colorToHex(hslToRgb(next)), "color");
  }

  function onCustomAccentHexInput(event: Event): void {
    settings.updateCustomAccent((event.currentTarget as HTMLInputElement).value, "hex");
  }

  function customAccentStatusLabel(): string {
    if (!settings.customAccentValid) {
      return tr("gui.colors.invalid_hex");
    }
    return settings.colors.contrastGuard
      ? tr("gui.colors.light_dark_auto")
      : tr("gui.colors.direct_accent");
  }

  function formattedDraftNumber(value: NumericSetting): string {
    return value === null ? "—" : numberFormatter.format(value);
  }

  function streamBufferKiBLabel(value: number): string {
    return tr("gui.settings.performance.buffer_kib_value")
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
        <span class="eyebrow">{tr("gui.appearance.eyebrow")}</span>
        <h1>{tr("gui.appearance.title")}</h1>
        <p>{tr("gui.appearance.subtitle")}</p>
      </div>
      <div class={`settings-live-state state-${appearanceSaveState}`} role="status" aria-live="polite">
        <Icon
          name={appearanceSaveState === "error" ? "x-circle" : appearanceSaveState === "saving" ? "hourglass" : appearanceSaveState === "session" ? "info" : "check-circle"}
          size={16}
        />
        {appearanceSaveState === "saved"
          ? tr("gui.appearance.changes_apply_immediately")
          : settingsSaveStateLabel(appearanceSaveState)}
      </div>
    </div>

    <div class="appearance-layout interface-layout">
      <section class="display-settings-panel main-display-panel">
        <div class="panel-title"><Icon name="list" size={16} />{tr("gui.appearance.display_settings")}</div>
        <div class="setting-list">
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.interface_mode")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.interface_mode")}>
              <button class:active={modernModeSelected} aria-pressed={modernModeSelected} onclick={() => setMode("modern")}>{tr("gui.mode.modern")}</button>
              <button class:active={classicModeSelected} aria-pressed={classicModeSelected} onclick={() => setMode("classic")}>{tr("gui.mode.classic")}</button>
            </div>
          </div>
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.theme")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.theme_preference")}>
              <button class:active={activeThemeChoice === "light"} aria-pressed={activeThemeChoice === "light"} onclick={() => setTheme("light")}>{tr("gui.theme.light")}</button>
              <button class:active={activeThemeChoice === "dark"} aria-pressed={activeThemeChoice === "dark"} onclick={() => setTheme("dark")}>{tr("gui.theme.dark")}</button>
              <button class:active={activeThemeChoice === "system"} aria-pressed={activeThemeChoice === "system"} onclick={() => setTheme("system")}>{tr("gui.theme.system")}</button>
            </div>
          </div>
          <div class="setting-row mode-setting-row">
            <span>{tr("gui.appearance.density")}</span>
            <div class="mode-segments" aria-label={tr("gui.appearance.density_preference")}>
              <button class:active={activeDensityChoice === "compact"} aria-pressed={activeDensityChoice === "compact"} onclick={() => setDensity("compact")}>{tr("gui.density.compact")}</button>
              <button class:active={activeDensityChoice === "standard"} aria-pressed={activeDensityChoice === "standard"} onclick={() => setDensity("standard")}>{tr("gui.density.standard")}</button>
              <button class:active={activeDensityChoice === "comfort"} aria-pressed={activeDensityChoice === "comfort"} onclick={() => setDensity("comfort")}>{tr("gui.density.comfort")}</button>
            </div>
          </div>
          <div><span>{tr("gui.appearance.current_colors")}</span><strong>{activePaletteName()}</strong></div>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "colors"}
  <div class="colors-view modern-colors">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.screen.colors")}</span>
        <h1>{tr("gui.colors.title")}</h1>
        <p>{tr("gui.colors.subtitle")}</p>
      </div>
      <SettingsSaveAction
        state={settings.colorsSaveState}
        statusLabel={settingsSaveStatusLabel("colors", settings.colorsSaveState)}
        actionLabel={tr("gui.colors.apply")}
        savingLabel={tr("gui.settings.save_state.saving")}
        disabledReason={settingsSaveDisabledReason(
          "colors",
          settings.colorsDirty,
          settings.paletteApplyBlocked ? tr("gui.colors.invalid_hex") : "",
        )}
        icon="sparkles"
        onSave={() => void settings.saveColors()}
      />
    </div>

    <div class="appearance-layout" role="radiogroup" aria-label={tr("gui.colors.title")}>
      <section class="palette-panel">
        <div class="panel-title"><Icon name="palette" size={16} />{tr("gui.colors.curated_palettes")}</div>
        <div class="palette-grid">
          {#each builtInPalettes as palette}
            <label
              class:selected={palette.id === settings.colors.palette}
              class={`palette-card palette-${palette.id} theme-${activeTheme}`}
            >
              <div class="palette-card-head">
                <span class="palette-choice">
                  <input type="radio" name="color-palette" value={palette.id}
                    checked={palette.id === settings.colors.palette}
                    onchange={() => settings.setPalette(palette.id)} />
                  <strong>{paletteName(palette)}</strong>
                </span>
                <span>{paletteMood(palette)}</span>
              </div>
              <div class="palette-swatches"><i></i><i></i><i></i></div>
              <p>{paletteNote(palette)}</p>
            </label>
          {/each}
        </div>
      </section>

      <aside class="color-workbench">
        <div class="panel-title"><Icon name="sparkles" size={16} />{tr("gui.colors.custom_color_wheel")}</div>
        <div class="color-wheel-wrap">
          <div class="color-wheel-picker" use:cssVariables={colorWheelCssVariables()}>
            <button
              type="button"
              class="color-wheel-button"
              aria-label={`${tr("gui.colors.custom_accent_hue_wheel")} ${settings.colors.accent}`}
              aria-describedby="custom-color-status"
              aria-keyshortcuts="ArrowLeft ArrowRight ArrowUp ArrowDown Home End"
              title={`${tr("gui.colors.custom_accent_hue_wheel")} ${settings.colors.accent}`}
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
            <strong>{settings.colors.accent}</strong>
            <span>{tr("gui.colors.accent_preview")}</span>
            <label class="palette-choice custom-select-button">
              <input type="radio" name="color-palette" value="custom"
                checked={settings.colors.palette === "custom"}
                onchange={() => settings.setPalette("custom")} />
              <span>{tr("gui.colors.use_custom")}</span>
            </label>
          </div>
        </div>
        <div class="custom-color-fields">
          <label>
            <span>{tr("gui.colors.hex_value")}</span>
            <input
              class:invalid={!settings.customAccentValid}
              value={settings.colors.input}
              maxlength="7"
              spellcheck="false"
              aria-invalid={!settings.customAccentValid}
              aria-label={tr("gui.colors.hex_value")}
              aria-describedby="custom-color-status"
              oninput={onCustomAccentHexInput}
            />
          </label>
          <button onclick={() => settings.updateCustomAccent(defaultCustomAccent, "color")}>{tr("gui.colors.reset_custom")}</button>
        </div>
        <div
          id="custom-color-status"
          class:error={settings.colors.saveError || !settings.customAccentValid}
          class="custom-color-status"
          aria-live="polite"
        >{customAccentStatusLabel()}</div>
        <label class="settings-switch contrast-guard-toggle">
          <input
            type="checkbox"
            checked={settings.colors.contrastGuard}
            aria-label={tr("gui.colors.contrast_guard_toggle")}
            aria-describedby="contrast-guard-note"
            title={settings.colors.contrastGuard ? tr("gui.colors.contrast_guard_enabled") : tr("gui.colors.contrast_guard_disabled")}
            onchange={(event) => settings.setAccentContrastGuard(event.currentTarget.checked)}
          />
          <span>{settings.colors.contrastGuard ? tr("gui.colors.contrast_guard_enabled") : tr("gui.colors.contrast_guard_disabled")}</span>
        </label>
        <div class="theme-preview-pair">
          <div class="theme-preview custom-preview theme-light" use:cssVariables={customThemePreviewVariables("light")}>
            <div class="preview-toolbar"><span></span><span></span><span class="preview-theme-pill">{tr("gui.theme.light")}</span></div>
            <div class="preview-row selected"><span>archive.7z</span><strong>{tr("common.readiness")}</strong></div>
            <div class="preview-row"><span>sidecar.par2</span><strong>{tr("gui.colors.protected_preview")}</strong></div>
          </div>
          <div class="theme-preview custom-preview theme-dark" use:cssVariables={customThemePreviewVariables("dark")}>
            <div class="preview-toolbar"><span></span><span></span><span class="preview-theme-pill">{tr("gui.theme.dark")}</span></div>
            <div class="preview-row selected"><span>archive.7z</span><strong>{tr("common.readiness")}</strong></div>
            <div class="preview-row"><span>sidecar.par2</span><strong>{tr("gui.colors.protected_preview")}</strong></div>
          </div>
        </div>
        <div id="contrast-guard-note" class="contrast-note" aria-live="polite">
          <strong>{settings.colors.contrastGuard ? tr("gui.colors.contrast_guard_on") : tr("gui.colors.contrast_guard_off")}</strong>
          <span>{tr("gui.colors.contrast_guard_body")}</span>
        </div>
      </aside>
    </div>
  </div>
{:else if screen === "settingsGeneral"}
  <div class="settings-view modern-settings-general">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.general.eyebrow")}</span>
        <h1>{tr("gui.settings.general.title")}</h1>
        <p>{tr("gui.settings.general.subtitle")}</p>
      </div>
      <SettingsSaveAction
        state={settings.generalSaveState}
        statusLabel={settingsSaveStatusLabel("general", settings.generalSaveState)}
        actionLabel={tr("gui.settings.general.apply")}
        savingLabel={tr("gui.settings.save_state.saving")}
        disabledReason={settingsSaveDisabledReason(
          "general",
          settings.generalDirty,
          settings.generalValidationError,
        )}
        icon="settings"
        onSave={() => void settings.saveGeneral()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.section.general")}</div>
        <div class="setting-list">
          <div class="setting-control-row">
            <span>{tr("gui.settings.language")}</span>
            <select
              class="settings-select"
              value={settings.general.language}
              aria-label={tr("gui.settings.language.preference_label")}
              onchange={(event) => settings.setGeneral("language", event.currentTarget.value)}
            >
              <option value="">{tr("gui.settings.language.follow_system")}</option>
              {#each availableLanguages as language}
                <option value={language.tag}>{language.name} · {language.tag}</option>
              {/each}
            </select>
          </div>
          <div class="setting-control-row folder-setting-row">
            <span>{tr("gui.settings.folder.default_create")}</span>
            <div class="settings-path-field">
              <div class="settings-path-control">
                <input
                  class:invalid={Boolean(settings.defaultCreateFolderError)}
                  class="settings-path-input"
                  value={settings.general.defaultCreateDir}
                  placeholder={tr("gui.settings.folder.ask_when_creating")}
                  aria-label={tr("gui.settings.folder.default_create")}
                  aria-invalid={Boolean(settings.defaultCreateFolderError)}
                  aria-describedby={settings.defaultCreateFolderError ? "settings-default-create-folder-error" : undefined}
                  oninput={(event) => settings.setGeneral("defaultCreateDir", event.currentTarget.value)}
                />
                <button type="button" aria-label={tr("gui.settings.folder.choose_create")} onclick={() => void chooseDefaultCreateFolder()}>
                  <Icon name="folder-open" size={15} />
                </button>
                <button type="button" class="settings-path-reset" onclick={() => settings.setGeneral("defaultCreateDir", "")}>{tr("gui.settings.folder.clear")}</button>
              </div>
              {#if settings.defaultCreateFolderError}<small id="settings-default-create-folder-error" class="settings-field-error" role="status">{settings.defaultCreateFolderError}</small>{/if}
            </div>
          </div>
          <div class="setting-control-row folder-setting-row">
            <span>{tr("gui.settings.folder.default_extract")}</span>
            <div class="settings-path-field">
              <div class="settings-path-control">
                <input
                  class:invalid={Boolean(settings.defaultExtractFolderError)}
                  class="settings-path-input"
                  value={settings.general.defaultExtractDir}
                  placeholder={tr("gui.settings.folder.next_to_archive")}
                  aria-label={tr("gui.settings.folder.default_extract")}
                  aria-invalid={Boolean(settings.defaultExtractFolderError)}
                  aria-describedby={settings.defaultExtractFolderError ? "settings-default-extract-folder-error" : undefined}
                  oninput={(event) => settings.setGeneral("defaultExtractDir", event.currentTarget.value)}
                />
                <button type="button" aria-label={tr("gui.settings.folder.choose")} onclick={() => void chooseDefaultExtractFolder()}>
                  <Icon name="folder-open" size={15} />
                </button>
                <button type="button" class="settings-path-reset" onclick={() => settings.setGeneral("defaultExtractDir", "")}>{tr("gui.settings.folder.default")}</button>
              </div>
              {#if settings.defaultExtractFolderError}<small id="settings-default-extract-folder-error" class="settings-field-error" role="status">{settings.defaultExtractFolderError}</small>{/if}
            </div>
          </div>
          <div class="setting-control-row">
            <span>{tr("gui.settings.general.reveal_after_extract")}</span>
            <label class="settings-switch">
              <input
                type="checkbox"
                checked={settings.general.revealAfterExtract}
                aria-label={tr("gui.settings.general.reveal_after_extract_aria").replace("{fileManager}", fileManagerLabel())}
                onchange={(event) => settings.setGeneral("revealAfterExtract", event.currentTarget.checked)}
              />
              <span>{settings.general.revealAfterExtract ? tr("common.on") : tr("common.off")} · {tr("gui.settings.general.reveal_after_extract_hint").replace("{fileManager}", fileManagerLabel())}</span>
            </label>
          </div>
          <div class="setting-control-row">
            <span>{tr("gui.settings.general.automatic_update_checks")}</span>
            <label class="settings-switch">
              <input
                type="checkbox"
                checked={settings.general.automaticUpdateChecks}
                aria-label={tr("gui.settings.general.automatic_update_checks_aria")}
                onchange={(event) => settings.setGeneral("automaticUpdateChecks", event.currentTarget.checked)}
              />
              <span>{settings.general.automaticUpdateChecks ? tr("common.on") : tr("common.off")} · {tr("gui.settings.general.automatic_update_checks_hint")}</span>
            </label>
          </div>
          <div><span>{tr("gui.settings.general.open_with_policy").replace("{openWith}", openWithLabel())}</span><strong>{tr("gui.settings.general.open_with_value")}</strong></div>
        </div>
        <UpdateCheckCard
          {tr}
          preview={updateCheckPreview}
          automaticChecksEnabled={settings.general.automaticUpdateChecks}
        />
        <div class="setting-callout">
          <strong>{tr("gui.settings.general.boundary_title")}</strong>
          <span>{tr("gui.settings.general.boundary_body", "Password, recovery, unsafe path, and conflict prompts remain visible in their workflows.")}</span>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "settingsSecurity"}
  <div class="settings-view modern-settings-security">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.security.eyebrow")}</span>
        <h1>{tr("gui.settings.security.title")}</h1>
        <p>{tr("gui.settings.security.subtitle")}</p>
      </div>
      <SettingsSaveAction
        state={settings.securitySaveState}
        statusLabel={settingsSaveStatusLabel("security", settings.securitySaveState)}
        actionLabel={tr("gui.settings.security.save")}
        savingLabel={tr("gui.settings.save_state.saving")}
        disabledReason={settingsSaveDisabledReason("security", settings.securityDirty, settings.securityValidationError)}
        icon="shield-alert"
        onSave={() => void settings.saveSecurity()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="settings-metric-grid">
          <div class:invalid-setting={Boolean(settings.securityEntriesError)}><span>{tr("gui.settings.security.max_entries")}</span><strong>{formattedDraftNumber(settings.security.maxEntries)}</strong><small>{tr("gui.settings.captured_job_start")}</small></div>
          <div class:invalid-setting={Boolean(settings.securityOutputError)}><span>{tr("gui.settings.security.max_output")}</span><strong>{formattedDraftNumber(settings.security.maxOutputGiB)} GiB</strong><small>{tr("gui.settings.security.archive_bomb_guard")}</small></div>
          <div class:invalid-setting={Boolean(settings.securityRatioError)}><span>{tr("gui.settings.security.ratio_guard")}</span><strong>{formattedDraftNumber(settings.security.maxCompressionRatio)}x</strong><small>{tr("gui.settings.security.ratio_hint_short")}</small></div>
        </div>
        <div class="settings-input-grid" aria-label={tr("common.safety_limits")}>
          <label class="number-field">
            <span>{tr("gui.settings.security.max_entries")}</span>
            <input
              id="settings-security-max-entries"
              class:invalid={Boolean(settings.securityEntriesError)}
              type="number"
              min="1"
              max="10000000"
              step="1000"
              value={settings.security.maxEntries ?? ""}
              aria-invalid={Boolean(settings.securityEntriesError)}
              aria-describedby={settings.securityEntriesError ? "settings-security-max-entries-error" : undefined}
              oninput={(event) => settings.setSecurity("maxEntries", inputNumber(event))}
            />
            {#if settings.securityEntriesError}<small id="settings-security-max-entries-error" class="settings-field-error" role="status">{settings.securityEntriesError}</small>{/if}
          </label>
          <label class="number-field">
            <span>{tr("gui.settings.security.max_output_gib")}</span>
            <input
              id="settings-security-max-output"
              class:invalid={Boolean(settings.securityOutputError)}
              type="number"
              min="1"
              max="8192"
              step="1"
              value={settings.security.maxOutputGiB ?? ""}
              aria-invalid={Boolean(settings.securityOutputError)}
              aria-describedby={settings.securityOutputError ? "settings-security-max-output-error" : undefined}
              oninput={(event) => settings.setSecurity("maxOutputGiB", inputNumber(event))}
            />
            {#if settings.securityOutputError}<small id="settings-security-max-output-error" class="settings-field-error" role="status">{settings.securityOutputError}</small>{/if}
          </label>
          <label class="number-field">
            <span>{tr("gui.settings.security.ratio_guard")}</span>
            <input
              id="settings-security-ratio"
              class:invalid={Boolean(settings.securityRatioError)}
              type="number"
              min="1"
              max="100000"
              step="1"
              value={settings.security.maxCompressionRatio ?? ""}
              aria-invalid={Boolean(settings.securityRatioError)}
              aria-describedby={settings.securityRatioError ? "settings-security-ratio-error" : undefined}
              oninput={(event) => settings.setSecurity("maxCompressionRatio", inputNumber(event))}
            />
            {#if settings.securityRatioError}<small id="settings-security-ratio-error" class="settings-field-error" role="status">{settings.securityRatioError}</small>{/if}
          </label>
        </div>
        <div class="settings-actions-row">
          <button class="secondary-lite" onclick={settings.resetSecurity}>{tr("gui.settings.reset_defaults")}</button>
          <span>{settings.snapshotLabel}</span>
        </div>
        <div class="setting-callout">
          <strong>{tr("gui.settings.security.path_safety_always_on")}</strong>
          <span>{tr("gui.settings.security.path_safety_always_on_body")}</span>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "settingsPerformance"}
  <div class="settings-view modern-settings-performance">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.performance.eyebrow")}</span>
        <h1>{tr("gui.settings.performance.title")}</h1>
        <p>{tr("gui.settings.performance.subtitle")}</p>
      </div>
      <SettingsSaveAction
        state={settings.performanceSaveState}
        statusLabel={settingsSaveStatusLabel("performance", settings.performanceSaveState)}
        actionLabel={tr("gui.settings.performance.save")}
        savingLabel={tr("gui.settings.save_state.saving")}
        disabledReason={settingsSaveDisabledReason("performance", settings.performanceDirty, settings.performanceValidationError)}
        icon="hourglass"
        onSave={() => void settings.savePerformance()}
      />
    </div>

    <div class="settings-layout">
      <section class="settings-main-panel">
        <div class="settings-metric-grid performance-metric-grid">
          <div class:invalid-setting={Boolean(settings.performanceParallelJobsError)}><span>{tr("gui.settings.performance.parallel_tasks")}</span><strong>{settings.performance.parallelJobs === null ? tr("common.auto", "Auto") : formattedDraftNumber(settings.performance.parallelJobs)}</strong><small>{tr("gui.settings.performance.parallel_tasks_hint")}</small></div>
          <div class:invalid-setting={Boolean(settings.performanceThreadsError)}><span>{tr("gui.settings.performance.workers")}</span><strong>{settings.performance.threads === null ? tr("common.auto", "Auto") : formattedDraftNumber(settings.performance.threads)}</strong><small>{tr("gui.settings.performance.workers_hint")}</small></div>
          <div class:invalid-setting={Boolean(settings.performanceMemoryError)}><span>{tr("gui.settings.performance.stream_buffer")}</span><strong>{settings.performance.memoryKiB === null ? tr("common.auto", "Auto") : streamBufferKiBLabel(settings.performance.memoryKiB)}</strong><small>{tr("gui.settings.performance.copy_buffers")}</small></div>
        </div>
        <div class="level-control settings-slider">
          <div><strong>{tr("gui.settings.performance.parallel_tasks")} · {settings.performance.parallelJobs === null ? tr("common.auto", "Auto") : formattedDraftNumber(settings.performance.parallelJobs)}</strong><span>{tr("gui.settings.performance.parallel_tasks_body")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.parallel_tasks")}>
            <button class:active={settings.performance.parallelJobs === null} aria-pressed={settings.performance.parallelJobs === null} onclick={() => settings.setPerformance("parallelJobs", null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={settings.performance.parallelJobs === 1} aria-pressed={settings.performance.parallelJobs === 1} onclick={() => settings.setPerformance("parallelJobs", 1)}>1</button>
            <button class:active={settings.performance.parallelJobs === 2} aria-pressed={settings.performance.parallelJobs === 2} onclick={() => settings.setPerformance("parallelJobs", 2)}>2</button>
            <button class:active={settings.performance.parallelJobs === 4} aria-pressed={settings.performance.parallelJobs === 4} onclick={() => settings.setPerformance("parallelJobs", 4)}>4</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_parallel_jobs")}</span>
            <input
              id="settings-performance-parallel-jobs"
              class:invalid={Boolean(settings.performanceParallelJobsError)}
              type="number"
              min="1"
              max="8"
              step="1"
              value={settings.performance.parallelJobs ?? ""}
              aria-invalid={Boolean(settings.performanceParallelJobsError)}
              aria-describedby={settings.performanceParallelJobsError ? "settings-performance-parallel-jobs-error" : undefined}
              oninput={(event) => settings.setPerformance("parallelJobs", inputNumber(event))}
            />
            {#if settings.performanceParallelJobsError}<small id="settings-performance-parallel-jobs-error" class="settings-field-error" role="status">{settings.performanceParallelJobsError}</small>{/if}
          </label>
          <div><strong>{tr("gui.settings.performance.worker_threads")} · {settings.performance.threads === null ? tr("common.auto", "Auto") : formattedDraftNumber(settings.performance.threads)}</strong><span>{tr("gui.settings.performance.worker_threads_body")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.worker_threads", "Worker threads")}>
            <button class:active={settings.performance.threads === null} aria-pressed={settings.performance.threads === null} onclick={() => settings.setPerformance("threads", null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={settings.performance.threads === 4} aria-pressed={settings.performance.threads === 4} onclick={() => settings.setPerformance("threads", 4)}>4</button>
            <button class:active={settings.performance.threads === 8} aria-pressed={settings.performance.threads === 8} onclick={() => settings.setPerformance("threads", 8)}>8</button>
            <button class:active={settings.performance.threads === 16} aria-pressed={settings.performance.threads === 16} onclick={() => settings.setPerformance("threads", 16)}>16</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_threads")}</span>
            <input
              id="settings-performance-threads"
              class:invalid={Boolean(settings.performanceThreadsError)}
              type="number"
              min="1"
              max="64"
              step="1"
              value={settings.performance.threads ?? ""}
              aria-invalid={Boolean(settings.performanceThreadsError)}
              aria-describedby={settings.performanceThreadsError ? "settings-performance-threads-error" : undefined}
              oninput={(event) => settings.setPerformance("threads", inputNumber(event))}
            />
            {#if settings.performanceThreadsError}<small id="settings-performance-threads-error" class="settings-field-error" role="status">{settings.performanceThreadsError}</small>{/if}
          </label>
          <div><strong>{tr("gui.settings.performance.stream_buffer_memory")} · {settings.performance.memoryKiB === null ? tr("common.auto", "Auto") : streamBufferKiBLabel(settings.performance.memoryKiB)}</strong><span>{tr("gui.settings.performance.stream_buffer_body")}</span></div>
          <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.stream_buffer_memory")}>
            <button class:active={settings.performance.memoryKiB === null} aria-pressed={settings.performance.memoryKiB === null} onclick={() => settings.setPerformance("memoryKiB", null)}>{tr("common.auto", "Auto")}</button>
            <button class:active={settings.performance.memoryKiB === 8} aria-pressed={settings.performance.memoryKiB === 8} onclick={() => settings.setPerformance("memoryKiB", 8)}>{streamBufferKiBLabel(8)}</button>
            <button class:active={settings.performance.memoryKiB === 16} aria-pressed={settings.performance.memoryKiB === 16} onclick={() => settings.setPerformance("memoryKiB", 16)}>{streamBufferKiBLabel(16)}</button>
            <button class:active={settings.performance.memoryKiB === 32} aria-pressed={settings.performance.memoryKiB === 32} onclick={() => settings.setPerformance("memoryKiB", 32)}>{streamBufferKiBLabel(32)}</button>
            <button class:active={settings.performance.memoryKiB === 64} aria-pressed={settings.performance.memoryKiB === 64} onclick={() => settings.setPerformance("memoryKiB", 64)}>{streamBufferKiBLabel(64)}</button>
          </div>
          <label class="number-field worker-field">
            <span>{tr("gui.settings.performance.custom_buffer_kib")}</span>
            <input
              id="settings-performance-memory"
              class:invalid={Boolean(settings.performanceMemoryError)}
              type="number"
              min="8"
              max="64"
              step="1"
              value={settings.performance.memoryKiB ?? ""}
              aria-invalid={Boolean(settings.performanceMemoryError)}
              aria-describedby={settings.performanceMemoryError ? "settings-performance-memory-error" : undefined}
              oninput={(event) => settings.setPerformance("memoryKiB", inputNumber(event))}
            />
            {#if settings.performanceMemoryError}<small id="settings-performance-memory-error" class="settings-field-error" role="status">{settings.performanceMemoryError}</small>{/if}
          </label>
          <div class="settings-actions-row">
            <button class="secondary-lite" onclick={settings.resetPerformance}>{tr("gui.settings.performance.use_auto")}</button>
            <span>{settings.snapshotLabel}</span>
          </div>
        </div>
      </section>
    </div>
  </div>
{:else if screen === "passwordBook"}
  <div class="settings-view modern-password-book">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.password_book.eyebrow")}</span>
        <h1>{tr("gui.settings.password_book.title")}</h1>
        <p>{tr("gui.settings.password_book.subtitle")}</p>
      </div>
      <button
        class="sheet-action"
        disabled={Boolean(passwordBookForgetDisabledReason())}
        title={passwordBookForgetDisabledReason()}
        aria-label={labelWithDisabledReason(
          tr("gui.settings.password_book.forget_current"),
          passwordBookForgetDisabledReason(),
        )}
        onclick={() => void forgetPasswordBookPanel()}
      ><Icon name="lock" size={17} />{tr("gui.settings.password_book.forget_current")}</button>
    </div>

    <div class="password-book-layout">
      <section class="settings-main-panel password-book-panel" aria-label={tr("gui.settings.password_book.current_archive")}>
        <div class="password-book-context">
          <span>{tr("gui.settings.password_book.current_archive")}</span>
          <strong>{currentArchiveName()}</strong>
          {#if passwordBookArchiveId !== null}<span>{passwordBookCurrentLabel()}</span>{/if}
          <p id="password-book-detail">{passwordBookDetailLabel()}</p>
        </div>
        {#if passwordBookCanSave}
          <form class="password-book-form" onsubmit={submitPasswordBook}>
            <label for="password-book-value">{tr("gui.settings.password_book.password_label")}</label>
            <input id="password-book-value" class="secure-input" type="password" autocomplete="current-password"
              bind:this={passwordInput} bind:value={passwordValue} disabled={passwordSaveBusy}
              aria-invalid={Boolean(passwordSaveError)} aria-describedby={passwordSaveError ? "password-book-error password-book-help" : "password-book-help"}
              oninput={() => { passwordEmptyError = false; if (!passwordSaveBusy) clearPasswordBookSave(); }} />
            <p id="password-book-help">{tr("gui.settings.password_book.verify_help")}</p>
            <div class="password-book-actions">
              <button class="primary-lite" type="submit" disabled={passwordSaveBusy} aria-busy={passwordSaveBusy}>
                <Icon name="lock" size={17} />
                {passwordBookSaved ? tr("gui.settings.password_book.replace_password") : tr("gui.settings.password_book.save_password")}
              </button>
              {#if passwordSaveBusy}
                <button class="secondary-lite" type="button" disabled={passwordSaveState.phase === "cancelling"} onclick={() => void cancelPasswordBookSave()}>{tr("gui.common.cancel")}</button>
              {/if}
            </div>
            {#if passwordSaveError}
              <p class="password-inline-error" id="password-book-error" role="alert">{passwordSaveError}</p>
            {/if}
            {#if passwordSaveState.cancelFailed}
              <p class="password-inline-error" role="alert">{tr("gui.settings.password_book.cancel_failed")}</p>
            {/if}
            <div class="password-book-status" role="status" aria-live="polite" aria-atomic="true">
              {#if passwordSaveState.phase === "verifying"}
                <Icon name="hourglass" size={17} /><span>{tr("gui.settings.password_book.verifying")}</span>
              {:else if passwordSaveState.phase === "cancelling"}
                <Icon name="hourglass" size={17} /><span>{tr("gui.settings.password_book.cancelling")}</span>
              {:else if passwordSaveState.phase === "saved"}
                <Icon name="check" size={17} /><span>{tr("gui.password.saved")}</span>
              {:else if passwordSaveState.phase === "cancelled"}
                <span>{tr("gui.settings.password_book.save_cancelled")}</span>
              {/if}
            </div>
          </form>
        {/if}
        <div class="settings-actions-row">
          <button
            class="secondary-lite"
            disabled={Boolean(passwordBookRefreshDisabledReason())}
            aria-busy={passwordBookStatusState === "checking"}
            title={passwordBookRefreshDisabledReason()}
            aria-label={labelWithDisabledReason(
              tr("gui.settings.password_book.refresh_status"),
              passwordBookRefreshDisabledReason(),
            )}
            onclick={() => void refreshPasswordBookPanel()}
          >{tr("gui.settings.password_book.refresh_status")}</button>
          <span>{secretStoreLabel()} · {passwordBookSecretStoreLabel()}</span>
        </div>
        <details class="password-book-details">
          <summary>{tr("gui.settings.password_book.how_passwords_work")}</summary>
          <p>{tr("gui.settings.password_book.secret_store_detail").replace("{platform}", platformNameLabel()).replace("{secretStore}", secretStoreLabel())}</p>
          <div class="limits-table">
          <div><b>{tr("gui.settings.password_book.source")}</b><b>{tr("gui.settings.password_book.priority")}</b><b>{tr("gui.settings.password_book.stored_where")}</b><b>{tr("gui.settings.password_book.failure_behavior")}</b></div>
          <div><span>{tr("gui.settings.password_book.manual_prompt")}</span><span>1</span><span>{tr("gui.settings.password_book.transient_input")}</span><strong>{tr("gui.settings.password_book.retry_prompt")}</strong></div>
          <div><span>{tr("gui.settings.password_book.session_cache")}</span><span>2</span><span>{tr("gui.settings.password_book.process_memory")}</span><strong>{tr("gui.settings.password_book.cleared_on_exit")}</strong></div>
          <div><span>{secretStoreLabel()}</span><span>3</span><span>{tr("gui.settings.password_book.system_secret_store")}</span><strong>{tr("gui.settings.password_book.fallback_prompt")}</strong></div>
          </div>
        <div class="setting-callout">
          <strong>{tr("gui.settings.password_book.no_plaintext_storage_title")}</strong>
          <span>{tr("gui.settings.password_book.no_plaintext_storage_body")}</span>
        </div>
        </details>
      </section>
    </div>
  </div>
{:else if screen === "integration"}
  <div class="integration-view modern-integration">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.settings.integration.eyebrow")}</span>
        <h1>{tr("gui.settings.integration.title").replace("{fileManager}", fileManagerLabel())}</h1>
        <p>{tr("gui.settings.integration.subtitle").replace("{fileManager}", fileManagerLabel())}</p>
      </div>
      <div class="sheet-action-row integration-actions">
        <button class="sheet-action secondary-action" disabled={integrationBusy()} aria-busy={integrationOperation === "checking"} onclick={() => void refreshIntegrationStatus()}><Icon name="search" size={17} />{tr("gui.common.refresh")}</button>
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
        <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.settings.integration.file_types")}</div>
        <div class="association-tools">
          <div class="association-search">
            <Icon name="search" size={13} />
            <input
              type="search"
              value={associationFilter}
              placeholder={tr("gui.settings.integration.filter_extensions")}
              aria-label={tr("gui.settings.integration.filter_extensions")}
              oninput={(event) => (associationFilter = event.currentTarget.value)}
            />
            {#if associationFilter}
              <button type="button" aria-label={tr("gui.settings.integration.clear_filter")} title={tr("gui.settings.integration.clear_filter")} onclick={clearAssociationFilter}><Icon name="x-circle" size={13} /></button>
            {/if}
          </div>
          <div class="assoc-count">{formatRegistrySourceLabel()} · {tr("gui.settings.integration.showing_filtered_rows").replace("{shown}", String(visibleAssociationRows().length)).replace("{total}", String(associationRows().length))}</div>
        </div>
        <div class="assoc-chip-row">
          {#each associationSummary() as item}
            <span>{item}</span>
          {/each}
        </div>
        <div
          class="association-volume-guide"
          aria-label={tr("gui.settings.integration.volume_guide_label")}
        >
          <div class="native">
            <Icon name="archive" size={17} />
            <span>
              <strong>{tr("gui.settings.integration.native_volumes_title")}</strong>
              <small>{tr("gui.settings.integration.native_volumes_body", "ZIP .z01/.zip and RAR partN.rar carry format-specific disk metadata. They are not interchangeable with .001 byte splits.")}</small>
            </span>
          </div>
          <div class="generic">
            <Icon name="repeat" size={17} />
            <span>
              <strong>{tr("gui.settings.integration.generic_volumes_title")}</strong>
              <small>{tr("gui.settings.integration.generic_volumes_body", "Squallz can split supported outputs into a continuous .001/.002 sequence. This does not create native ZIP, RAR, or Split WIM volumes.")}</small>
            </span>
          </div>
        </div>
        <div class="association-table">
          <div class="assoc-head">
            <span>{tr("common.type")}</span>
            <span>{tr("common.format")}</span>
            <span>{tr("gui.settings.integration.archive_access")}</span>
            <span>{tr("gui.settings.integration.volume_support")}</span>
            <span>{tr("common.action")}</span>
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
              <strong>{tr("gui.settings.integration.no_matching_types")}</strong>
              <span>{tr("gui.settings.integration.no_matching_types_body")}</span>
              <button type="button" onclick={clearAssociationFilter}>{tr("gui.settings.integration.clear_filter")}</button>
            </div>
          {/each}
        </div>
      </section>

      <IntegrationHealthPanel
        panelTitle={tr("gui.settings.integration.file_manager_action_files").replace("{fileManager}", fileManagerLabel())}
        healthTitle={tr("gui.settings.integration.health_title", "Integration health")}
        platform={platformNameLabel()}
        scope={integrationScopeLabel()}
        summary={integrationSummaryLabel()}
        detail={integrationDetailLabel()}
        state={integrationPanelState()}
        busy={integrationBusy()}
        actions={integrationActionViews()}
        diagnosticsTitle={tr("gui.settings.integration.system_diagnostics_title")}
        diagnostics={integrationDiagnosticViews()}
        onDiagnosticAction={(target) => void openIntegrationGuide(target)}
        removeLabel={tr("gui.settings.integration.uninstall_actions")}
        removeDisabledReason={integrationRemoveDisabledReason()}
        removeAriaLabel={labelWithDisabledReason(
          tr("gui.settings.integration.uninstall_actions"),
          integrationRemoveDisabledReason(),
        )}
        onRemove={() => void removeIntegrationChanges()}
      />
    </div>
  </div>
{/if}
