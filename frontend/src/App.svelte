<script lang="ts">
  import { onMount, tick } from "svelte";
  import ArchiveReturnStrip from "./components/ArchiveReturnStrip.svelte";
  import AppIcon from "./components/AppIcon.svelte";
  import ChecksumAlgorithmPicker from "./components/ChecksumAlgorithmPicker.svelte";
  import ExcludeRulesEditor from "./components/ExcludeRulesEditor.svelte";
  import Icon from "./components/Icon.svelte";
  import SettingsRouteList from "./components/SettingsRouteList.svelte";
  import TaskProgressDialog from "./components/TaskProgressDialog.svelte";
  import {
    operationHistory,
    recordOperation,
  } from "./lib/history.svelte";
  import {
    adoptOpenedArchive,
    allRowsLoaded,
    archive,
    archivePasswordBookStatus,
    clearSelection,
    currentDirs,
    enterDir,
    forgetCurrentArchivePassword,
    gotoBreadcrumb,
    goUp,
    installArchivePreview,
    loadedRows,
    openArchive as openArchiveStore,
    reopenWithEncoding,
    refreshCurrentArchive,
    refreshArchivePasswordBookStatus,
    recentFiles,
    rememberRecent,
    PAGE_SIZE as ARCHIVE_PAGE_SIZE,
    prefetchAround,
    rowAt,
    selectedPaths,
    selectedSize,
    toggleSelect,
    totalRows,
  } from "./lib/archive.svelte";
  import {
    ipc,
    type CreateEstimateDto,
    type DiskSpaceDto,
    type EntryPreviewDto,
    type EntryDto,
    type IntegrationApplyResultDto,
    type IntegrationRemoveResultDto,
    type IntegrationStatusDto,
    type JobSpec,
    type LanguageDto,
    type NestedArchivePreviewDto,
    type SettingsDto,
    type FormatDto,
  } from "./lib/ipc";
  import {
    previewSampleForEntry,
    readRuntimePreviews,
  } from "./lib/dev-preview-data";
  import {
    basename as pathBaseName,
    dirname as pathDir,
    formatBytes,
    parseDelimitedRules,
  } from "./lib/format";
  import {
    buildExternalTaskJobSpec,
    externalOpenAction,
    type ExternalOpenAction,
  } from "./lib/external-tasks";
  import {
    taskWindowLaunchStateFromParams,
    taskWindowShellMessage,
    taskWindowShellTitle,
    taskWindowSubmitFailureStatus,
    taskWindowSubmitPlan,
    taskWindowSubmitTransition,
    type TaskWindowSubmitTransition,
  } from "./lib/task-window";
  import { allFormats, loadFormats } from "./lib/formats.svelte";
  import { currentLang, listBundledLanguages, loadLocale, t } from "./lib/i18n.svelte";
  import {
    activeTask,
    answerConflict as answerJobConflict,
    answerPassword as answerJobPassword,
    cancelTask,
    initJobEvents,
    pauseTask,
    pendingConflict,
    pendingPassword,
    resumeTask,
    installActiveTaskPreview,
    installCompletedTaskPreview,
    setRevealAfterExtractPreference,
    setTaskExpanded,
    submitJob as submitArchiveJob,
    tasks,
    titleFor as titleForJobSpec,
    type Task,
  } from "./lib/jobs.svelte";
  import {
    checksumItemStatus,
    checksumItemText,
    checksumResultLine,
    isTaskActiveState,
    taskChecksumResultText,
    taskOutputIsFolder,
    taskOutputPath,
    taskResultScreen,
    taskStateLabel,
    type TaskDialogModel,
  } from "./lib/task-dialog";
  import {
    activeUiMode,
    initUiMode,
    setUiMode as persistUiMode,
    uiModeChoice,
    type UiMode,
  } from "./lib/uiMode.svelte";
  import {
    buildCustomPaletteData,
    colorFromWheelPoint as colorFromWheelPointForAccent,
    colorToHex,
    colorWheelHsl as colorWheelHslForAccent,
    colorWheelMarkerStyle as colorWheelMarkerStyleForAccent,
    customPaletteTokenStyle,
    hslToRgb,
    normalizeHexColor,
  } from "./lib/theme";
  import {
    builtInPalettes,
    checksumAlgorithms,
    classicCommands,
    contextActions,
    createFormatIds,
    createFormats,
    createProfileIds,
    createProfiles,
    defaultCustomAccent,
    moveTargetPresets,
    nav,
    paletteIds,
    palettes,
    quickActions,
    recoveryBlocks,
    recoveryModes,
    screenIds,
    settingsSections,
  } from "./lib/ui-model";
  import type {
    ChecksumAlgorithmId,
    CreateFormatId,
    CreateProfileId,
    DensityChoice,
    NumericSetting,
    Palette,
    PaletteId,
    ResolvedTheme,
    Screen,
  } from "./lib/ui-model";

  type Mode = UiMode;
  type DialogModule = typeof import("@tauri-apps/plugin-dialog");
  type OpenDialogOptions = NonNullable<Parameters<DialogModule["open"]>[0]>;
  type SaveDialogOptions = NonNullable<Parameters<DialogModule["save"]>[0]>;
  type NativeDialogOptions = OpenDialogOptions | SaveDialogOptions;
  type PlatformKind = "macos" | "windows" | "linux";
  type ThemeChoice = "system" | "light" | "dark";
  type ExtractDestinationMode = "smart" | "same" | "choose";
  type ExtractOverwriteMode = "ask" | "skip" | "overwrite" | "rename";
  type PalettePreviewData = {
    accent: string;
    support: string;
    base: string;
    contrast: string;
  };
  type CreatePreflightPhase =
    | "idle"
    | "selecting"
    | "measuring"
    | "checkingTemp"
    | "choosingDest"
    | "checkingDest"
    | "submitting"
    | "ready"
    | "blocked";
  class JobSubmitBlockedError extends Error {
    constructor() {
      super("job-submit-blocked");
    }
  }

  const devToolsChordKeys = new Set(["i", "j", "c"]);
  type CustomCreateProfile = {
    id: string;
    name: string;
    level: number;
  };
  type FormatCapabilityCard = {
    id: string;
    name: string;
    state: string;
    create: string;
    split: string;
    encrypt: string;
    note: string;
  };
  type FormatCoverageRow = {
    label: string;
    value: string;
    detail: string;
  };
  type RenameTargetIssue = {
    blocking: string | null;
    warning: string | null;
  };
  type MovePlanItem = {
    from: string;
    to: string;
    conflict: boolean;
    reason: string | null;
    keepBothTo: string | null;
  };
  type MoveConflictReview = {
    targetDir: string;
    items: MovePlanItem[];
  };
  type CreatePreflightEvent = {
    phase?: string;
    scanned?: number;
    current?: string;
  };
  type OpenFilesPayload = {
    paths: string[];
    action?: string | null;
    output?: string | null;
  };
  type DisplayEntry = {
    name: string;
    type: string;
    size: string;
    packed: string;
    ratio: string;
    modified: string;
    crc: string;
    method: string;
    attr: string;
    source?: EntryDto;
    virtualIndex?: number;
  };
  type BatchArchiveRow = {
    name: string;
    format: string;
    entries: string;
    target: string;
    state: string;
  };
  type AssociationRow = {
    ext: string;
    format: string;
    status: string;
    action: string;
  };
  type EntryContext = {
    x: number;
    y: number;
    name: string;
    path: string | null;
    canRename: boolean;
    isDir: boolean;
  };
  type PreviewPhase = "idle" | "entry" | "nested";
  type PreviewPolicyKind = "none" | "folder" | "nested" | "inline-image" | "system-file";
  type PreviewPolicyCode =
    | "no_archive"
    | "select_one"
    | "folder"
    | "nested"
    | "inline_image"
    | "system_large_image"
    | "system_type"
    | "system_unknown"
    | "inline_ready"
    | "system_ready"
    | "nested_ready"
    | "failed";
  type PreviewPolicy = {
    kind: PreviewPolicyKind;
    label: string;
    code: PreviewPolicyCode;
    disabledReason: string;
  };
  type PreviewFailure = {
    entryPath: string;
    entryType: EntryDto["entry_type"] | null;
    displayName: string;
    policyKind: PreviewPolicyKind;
  };
  type ValidationWindow = Window & {
    __squallzValidationSetScreen?: (next: Screen) => boolean;
    __squallzValidationJobSubmitAttempts?: number;
    __squallzValidationJobSubmitBlockedWhileStarting?: number;
  };

  const params = new URLSearchParams(window.location.search);
  const INLINE_IMAGE_PREVIEW_MAX_BYTES = 16 * 1024 * 1024;
  const modeParam = params.get("mode");
  const defaultExtractDirParam = params.get("defaultExtractDir");
  const initialMode: Mode | null = modeParam === "classic" || modeParam === "modern" ? modeParam : null;
  const forceFirstRun = params.get("firstRun") === "1" || modeParam === "unset";
  const runtimePreviews = readRuntimePreviews(params, ARCHIVE_PAGE_SIZE);
  const hideHistoryParam = params.get("hideHistory") === "1";
  const createFormatParam = params.get("createFormat");
  const previewDelayMs = Math.max(0, Math.min(500, Number(params.get("previewDelayMs") ?? 0) || 0));
  initUiMode(forceFirstRun ? null : initialMode);

  let mode = $derived(activeUiMode());
  let settingsStatus = $state<"loading" | "ready" | "preview">(forceFirstRun || initialMode ? "preview" : "loading");
  let hideOperationHistory = $state(hideHistoryParam);
  let firstRunRequired = $derived(settingsStatus !== "loading" && uiModeChoice() === null);
  let currentArchive = $derived(archive());
  let archiveDirs = $derived(currentDirs());
  let passwordBookStatus = $derived(archivePasswordBookStatus());
  let jobRows = $derived(tasks());
  let activeCurrentTask = $derived(activeTask());
  let jobPasswordPrompt = $derived(pendingPassword());
  let jobConflictPrompt = $derived(pendingConflict());
  let taskDialogTaskId = $state<number | null>(null);
  let taskDialogDismissedId = $state<number | null>(null);
  const initialTaskWindowLaunchState = taskWindowLaunchStateFromParams(params);
  let taskWindowLaunchState = $state(initialTaskWindowLaunchState);
  const initialTaskWindowLaunch = initialTaskWindowLaunchState.launch;
  let taskWindowMode = $derived(taskWindowLaunchState.mode);
  let taskWindowPendingAction = $derived(taskWindowLaunchState.pendingAction);
  let taskWindowShellTitleCopy = $derived(taskWindowShellTitle(taskWindowLaunchState, tr));
  let taskWindowShellCopy = $derived(taskWindowShellMessage(taskWindowLaunchState, tr));
  let jobSubmitInFlight = $state(false);
  let submittingJobSpec = $state<JobSpec | null>(null);
  let jobPasswordValue = $state("");
  let appNotice = $state<string | null>(null);
  let checksumResultPanel = $state<HTMLElement | null>(null);
  let checksumCheckResultPanel = $state<HTMLElement | null>(null);
  let checksumCopyFeedbackKind = $state<"checksum" | "checksum_check" | "task" | null>(null);
  let checksumCopyFeedbackTaskId = $state<number | null>(null);
  let checksumCopyFeedbackMessage = $state<string | null>(null);
  let checksumCopyFeedbackTone = $state<"success" | "danger" | null>(null);
  let integrationStatus = $state<"idle" | "applying" | "installed" | "blocked">("idle");
  let integrationResult = $state<IntegrationApplyResultDto | null>(null);
  let integrationInstalledCount = $state(0);
  let integrationServicesDir = $state<string | null>(null);
  let integrationScriptDir = $state<string | null>(null);
  let browseScrollTop = $state(0);
  let browseViewportHeight = $state(0);
  const refreshedUpdateJobs = new Set<number>();
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  let checksumCopyFeedbackTimer: ReturnType<typeof setTimeout> | null = null;
  const screenParam = params.get("screen");
  let screen = $state<Screen>(
    screenIds.includes(screenParam as Screen) ? (screenParam as Screen) : "browse",
  );
  const archiveReturnScreens: Screen[] = ["checksum", "duplicates", "recovery"];
  const customCreateProfilesKey = "squallz.customCreateProfiles.v1";
  const activeCustomCreateProfileKey = "squallz.activeCustomCreateProfile";
  const previewLanguageKey = "squallz.previewLanguage.v1";
  const maxCustomCreateProfiles = 8;

  function previewStorage(): Storage | null {
    try {
      return typeof window === "undefined" ? null : window.localStorage;
    } catch {
      return null;
    }
  }

  function storedPreviewLanguage(): string | null {
    return previewStorage()?.getItem(previewLanguageKey) ?? null;
  }

  function storePreviewLanguage(language: string | null) {
    const storage = previewStorage();
    if (!storage) return;
    if (language) storage.setItem(previewLanguageKey, language);
    else storage.removeItem(previewLanguageKey);
  }

  const windowsReservedBaseNames = new Set([
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
  ]);
  const paletteParam = params.get("palette");
  const hasPaletteOverride = isPaletteId(paletteParam);
  const themeParam = params.get("theme");
  const initialThemeChoice: ThemeChoice | null = isThemeChoice(themeParam) ? themeParam : null;
  const densityParam = params.get("density");
  const hasDensityOverride = isDensityChoice(densityParam);
  let activePalette = $state<PaletteId>(
    hasPaletteOverride ? paletteParam : "aqua",
  );
  let customAccent = $state(defaultCustomAccent);
  let customAccentInput = $state(defaultCustomAccent);
  let customAccentSaveError = $state(false);
  let accentContrastGuard = $state(true);
  let activeThemeChoice = $state<ThemeChoice>(initialThemeChoice ?? "system");
  let activeDensityChoice = $state<DensityChoice>(hasDensityOverride ? densityParam : "standard");
  let activePlatform = $state<PlatformKind>(buildTargetPlatform());
  let prefersDarkTheme = $state(
    typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  let activeTheme = $derived<ResolvedTheme>(
    activeThemeChoice === "system" ? (prefersDarkTheme ? "dark" : "light") : activeThemeChoice,
  );
  const bytesPerMiB = 1024 ** 2;
  const bytesPerGiB = 1024 ** 3;
  const defaultSafety = {
    maxOutputGiB: 256,
    maxEntries: 1_000_000,
    maxCompressionRatio: 2048,
  };
  const extractDestinationModes: ExtractDestinationMode[] = ["smart", "same", "choose"];
  const extractOverwriteModes: ExtractOverwriteMode[] = ["ask", "skip", "overwrite", "rename"];
  const numberFormatter = new Intl.NumberFormat("en-US");
  let safetyMaxOutputGiB = $state<NumericSetting>(defaultSafety.maxOutputGiB);
  let safetyMaxEntries = $state<NumericSetting>(defaultSafety.maxEntries);
  let safetyMaxCompressionRatio = $state<NumericSetting>(defaultSafety.maxCompressionRatio);
  let performanceThreads = $state<NumericSetting>(null);
  let performanceMemoryMiB = $state<NumericSetting>(null);
  let settingsSnapshotLabel = $state(tr("gui.settings.snapshot.defaults_active", "Defaults active"));
  let availableLanguages = $state<LanguageDto[]>([]);
  let generalLanguageChoice = $state("");
  let generalDefaultExtractDir = $state(defaultExtractDirParam?.trim() ?? "");
  let generalRevealAfterExtract = $state(false);
  let extractDestinationMode = $state<ExtractDestinationMode>("smart");
  let extractCustomDest = $state("");
  let extractOverwriteMode = $state<ExtractOverwriteMode>("ask");
  let archiveOpenStatus = $state<"idle" | "opening">("idle");
  let openDialogModulePromise: Promise<DialogModule> | null = null;
  let batchArchivePaths = $state<string[]>(runtimePreviews.batchPaths);
  let checksumPath = $state(runtimePreviews.checksumPath);
  let checksumManifestPath = $state(runtimePreviews.checksumManifestPath);
  let checksumAlgorithm = $state<ChecksumAlgorithmId>("sha256");
  let checksumExcludeText = $state(".git\nnode_modules\n.DS_Store");
  let duplicateScanPath = $state(runtimePreviews.duplicateScanPath);
  let duplicateMinSize = $state(runtimePreviews.duplicateMinSize);
  let duplicateMinSizeError = $state("");
  let duplicateExcludeText = $state(".git\nnode_modules\n.DS_Store");
  let createDropInputs = $state<string[]>([]);
  let dragActive = $state(false);
  let lastDropKind = $state<"none" | "archives" | "create">("none");
  const initialCustomCreateProfiles = loadCustomCreateProfiles();
  const initialActiveCustomCreateProfileId = loadActiveCustomCreateProfileId(initialCustomCreateProfiles);
  const initialCustomCreateProfile = activeCustomProfileSnapshot(
    initialCustomCreateProfiles,
    initialActiveCustomCreateProfileId,
  );
  let customCreateProfiles = $state<CustomCreateProfile[]>(initialCustomCreateProfiles);
  let activeCustomCreateProfileId = $state(initialActiveCustomCreateProfileId);
  let customCreateLevel = $state(initialCustomCreateProfile.level);
  let customCreateLevelError = $state("");
  let customCreateProfileName = $state(initialCustomCreateProfile.name);
  let customCreateProfileNameError = $state("");
  let activeCreateProfile = $state<CreateProfileId>(loadCreateProfile());
  let activeCreateFormat = $state<CreateFormatId>(isCreateFormatId(createFormatParam) ? createFormatParam : loadCreateFormat());
  let createExcludeText = $state("node_modules\n.git\n*.tmp");
  let lastCreateEstimate = $state<CreateEstimateDto | null>(null);
  let lastDiskSpace = $state<DiskSpaceDto | null>(null);
  let lastTempDiskSpace = $state<DiskSpaceDto | null>(null);
  let lastCreateDest = $state<string | null>(null);
  let createPreflightPhase = $state<CreatePreflightPhase>(runtimePreviews.preflightScanned > 0 ? "measuring" : "idle");
  let createPreflightScanned = $state(runtimePreviews.preflightScanned);
  let createPreflightCurrent = $state(runtimePreviews.preflightCurrent);
  let createPreflightCleanup: (() => void) | null = null;
  let createPreflightListenPromise: Promise<void> | null = null;
  let createPreflightClosed = false;
  let nestedPreview = $state<NestedArchivePreviewDto | null>(null);
  let entryPreview = $state<EntryPreviewDto | null>(null);
  let entryPreviewFailure = $state<PreviewFailure | null>(null);
  let previewPhase = $state<PreviewPhase>("idle");
  let previewTargetName = $state("");
  let renameTargetName = $state("renamed.txt");
  let moveTargetDir = $state("moved/");
  let newFolderName = $state("New Folder");
  let moveConflictReview = $state<MoveConflictReview | null>(null);
  let historyRows = $derived(operationHistory());
  let activePopover = $state<"quickActions" | null>(null);
  let quickActionButton = $state<HTMLButtonElement | null>(null);
  let quickActionPopover = $state<HTMLDivElement | null>(null);
  let entryContext = $state<EntryContext | null>(null);
  let entryContextMenu = $state<HTMLDivElement | null>(null);
  const MODERN_ROW_HEIGHT = 42;
  const CLASSIC_ROW_HEIGHT = 29;
  const VIRTUAL_OVERSCAN_ROWS = 12;

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
  const featuredFormatIds = ["zip", "7z", "sqz", "tar.zst", "wim", "rar", "dmg", "iso"];
  const fallbackFormats = [
    archiveFormatDto("zip", ["zip", "jar", "apk", "cbz", "ipa"], {
      canCreate: true,
      canExtract: true,
      canEncryptData: true,
      canSplit: true,
      canUpdate: true,
    }),
    archiveFormatDto("tar", ["tar"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("7z", ["7z"], {
      canCreate: true,
      canExtract: true,
      canEncryptData: true,
      canEncryptNames: true,
      canSplit: true,
    }),
    archiveFormatDto("sqz", ["sqz"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("tar.zst", ["tar.zst", "tzst"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("tar.gz", ["tar.gz", "tgz"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("tar.xz", ["tar.xz", "txz"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("tar.bz2", ["tar.bz2", "tbz2"], { canCreate: true, canExtract: true, canSplit: true }),
    archiveFormatDto("wim", ["wim"], { canCreate: true, canExtract: true }),
    archiveFormatDto("rar", ["rar", "cbr"], { canExtract: true }),
    ...Array.from(longTailBridgeFormatIds).map((id) => archiveFormatDto(id, [id], { canExtract: true })),
    compressorFormat("gzip", ["gz", "gzip"]),
    compressorFormat("bzip2", ["bz2", "bzip2"]),
    compressorFormat("xz", ["xz"]),
    compressorFormat("zstd", ["zst", "zstd"]),
    compressorFormat("lz4", ["lz4"]),
    compressorFormat("brotli", ["br"]),
  ];

  function paletteName(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.name`, palette.name);
  }

  function paletteMood(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.mood`, palette.mood);
  }

  function paletteNote(palette: Palette): string {
    return tr(`gui.colors.palette.${palette.id}.note`, palette.note);
  }

  function palettePreviewData(palette: Palette, theme: ResolvedTheme = activeTheme): PalettePreviewData {
    if (theme === "dark") {
      return {
        accent: palette.darkAccent ?? palette.accent,
        support: palette.darkSupport ?? palette.support,
        base: palette.darkBase ?? palette.base,
        contrast: palette.darkContrast ?? palette.contrast,
      };
    }
    return {
      accent: palette.accent,
      support: palette.support,
      base: palette.base,
      contrast: palette.contrast,
    };
  }

  function paletteSwatchStyle(palette: Palette): string {
    const preview = palettePreviewData(palette);
    return `--swatch-a: ${preview.accent}; --swatch-b: ${preview.support}; --swatch-c: ${preview.base};`;
  }

  function activePaletteName(): string {
    return paletteName(activePaletteData);
  }

  function activePaletteMood(): string {
    return paletteMood(activePaletteData);
  }

  function contextActionLabel(action: string): string {
    return tr(`gui.settings.integration.context_action.${labelKey(action)}`, action);
  }

  function customPaletteStyle(): string {
    if (activePalette !== "custom") return "";
    return customPaletteStyleFor(activeTheme);
  }

  function customPaletteStyleFor(theme: ResolvedTheme): string {
    return customPaletteTokenStyle(customAccent, theme, accentContrastGuard);
  }

  function customThemePreviewStyle(theme: ResolvedTheme): string {
    return customPaletteTokenStyle(customAccent, theme, accentContrastGuard);
  }

  function colorWheelHsl() {
    return colorWheelHslForAccent(customAccent);
  }

  function colorWheelMarkerStyle(): string {
    return colorWheelMarkerStyleForAccent(customAccent);
  }

  function colorFromWheelPoint(x: number, y: number, size: number): string {
    return colorFromWheelPointForAccent(customAccent, x, y, size);
  }

  const customAccentValid = $derived(normalizeHexColor(customAccentInput) !== null);
  const paletteApplyBlocked = $derived(activePalette === "custom" && !customAccentValid);
  const customPaletteData = $derived<Palette>(
    buildCustomPaletteData(customAccent, activeTheme, accentContrastGuard),
  );
  const activePaletteData = $derived<Palette>(
    activePalette === "custom"
      ? customPaletteData
      : palettes.find((palette) => palette.id === activePalette) ?? palettes[0],
  );
  const activePalettePreviewData = $derived<PalettePreviewData>(
    palettePreviewData(activePaletteData),
  );

  $effect(() => {
    document.documentElement.dataset.theme = activeTheme;
    document.documentElement.dataset.palette = activePalette;
    document.documentElement.dataset.density = activeDensityChoice;
  });

  $effect(() => {
    if (jobPasswordPrompt) {
      screen = "password";
    } else if (jobConflictPrompt) {
      screen = "conflict";
    }
  });

  $effect(() => {
    const active = blockingTask();
    if (!active) return;
    taskDialogTaskId = active.id;
    taskDialogDismissedId = null;
  });

  $effect(() => {
    const completedTask = runtimePreviews.completedTask;
    if (!completedTask) return;
    const id = installCompletedTaskPreview(completedTask);
    if (id === null) return;
    taskDialogTaskId = id;
    taskDialogDismissedId = null;
  });

  $effect(() => {
    const activeTaskPreview = runtimePreviews.activeTask;
    if (!activeTaskPreview) return;
    const id = installActiveTaskPreview(activeTaskPreview);
    if (id === null) return;
    taskDialogTaskId = id;
    taskDialogDismissedId = null;
  });

  onMount(() => {
    if (!taskWindowMode || !initialTaskWindowLaunch) return;
    void submitExternalTaskWindow(
      initialTaskWindowLaunch.action,
      initialTaskWindowLaunch.paths,
      initialTaskWindowLaunch.output,
    );
  });

  $effect(() => {
    if (!taskWindowMode) return;
    const task = taskDialogTask();
    if (!task || task.id === null || task.expanded || isTaskActiveState(task.state)) return;
    if (taskResultScreen(task)) {
      setTaskExpanded(task.id, true);
    }
  });

  onMount(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const syncPreference = () => {
      prefersDarkTheme = media.matches;
    };
    syncPreference();
    media.addEventListener("change", syncPreference);

    return () => {
      media.removeEventListener("change", syncPreference);
    };
  });

  onMount(() => {
    void showNativeWindow();
  });

  onMount(() => {
    document.addEventListener("contextmenu", suppressBrowserContextMenu, { capture: true });
    window.addEventListener("keydown", suppressBrowserDebugShortcut, { capture: true });

    return () => {
      document.removeEventListener("contextmenu", suppressBrowserContextMenu, { capture: true });
      window.removeEventListener("keydown", suppressBrowserDebugShortcut, { capture: true });
    };
  });

  onMount(() => {
    const preview = runtimePreviews.archive;
    if (!preview) return;
    installArchivePreview(
      preview.info,
      preview.rows,
      {
        selected: preview.selected,
        total: preview.total,
        pages: preview.pages,
        previewRows: preview.previewRows,
      },
    );
    browseScrollTop = 0;
    nestedPreview = preview.nestedPreview;
  });

  onMount(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let listenerTimer: ReturnType<typeof setTimeout> | null = null;

    const openQueuedPaths = (payload: OpenFilesPayload) => {
      if (cancelled) return;
      void handleOpenFilesPayload(payload);
    };

    const startRealtimeOpenFileListener = async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        if (cancelled) return;
        const dispose = await listen<OpenFilesPayload>("app://open-files", (event) => {
          openQueuedPaths(event.payload);
        });
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
        const queued = await ipc.openFileListenerReady();
        openQueuedPaths(queued);
      } catch {
        // Dev preview has no native Tauri event bus.
      }
    };

    void ipc.takeOpenFiles()
      .then(async (event) => {
        if (!cancelled) await handleOpenFilesPayload(event);
      })
      .catch(() => {
        // Dev preview has no Tauri open-file queue.
      })
      .finally(() => {
        if (cancelled) return;
        listenerTimer = setTimeout(() => {
          if (!cancelled) void startRealtimeOpenFileListener();
        }, 250);
      });

    return () => {
      cancelled = true;
      if (listenerTimer !== null) clearTimeout(listenerTimer);
      unlisten?.();
    };
  });

  onMount(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;

    void initJobEvents()
      .then((dispose) => {
        if (cancelled) {
          dispose();
        } else {
          cleanup = dispose;
        }
      })
      .catch(() => {
        // Dev preview has no native Tauri job event bus.
      });

    return () => {
      cancelled = true;
      cleanup?.();
    };
  });

  onMount(() => () => {
    createPreflightClosed = true;
    createPreflightCleanup?.();
    createPreflightCleanup = null;
  });

  onMount(() => {
    void loadFormats().catch(() => {
      // Dev preview uses the release-scope fallback below.
    });
  });

  onMount(() => {
    if (hideHistoryParam) return;
    let cancelled = false;
    void ipc.isValidationSession()
      .then((enabled) => {
        if (!cancelled && enabled) hideOperationHistory = true;
      })
      .catch(() => {
        // Dev preview has no Tauri service; keep normal preview history visible.
      });
    return () => {
      cancelled = true;
    };
  });

  onMount(() => {
    let cancelled = false;
    void ipc.listLanguages()
      .then((languages) => {
        if (cancelled || languages.length === 0) return;
        availableLanguages = languages;
      })
      .catch(async () => {
        const languages = await listBundledLanguages();
        if (!cancelled) availableLanguages = languages;
      });

    return () => {
      cancelled = true;
    };
  });

  onMount(() => {
    const timer = setTimeout(() => {
      void getDialogModule().catch(() => undefined);
    }, 2200);
    return () => clearTimeout(timer);
  });

  onMount(() => {
    applyWindowChromePlatform(activePlatform);
    void ipc.platformKind()
      .then((platform) => {
        activePlatform = platform;
        applyWindowChromePlatform(platform);
        void ipc.recordValidationEvent("frontend.platform_kind", { platform }).catch(() => undefined);
      })
      .catch((error) => {
        void ipc.recordValidationEvent("frontend.platform_kind_error", {
          platform: activePlatform,
          error: error instanceof Error ? error.message : String(error),
        }).catch(() => undefined);
      });
  });

  onMount(() => {
    if (forceFirstRun) return;

    let cancelled = false;
    void ipc.getSettings()
      .then(async (settings) => {
        if (cancelled) return;
        if (!initialMode) initUiMode(settings.ui_mode);
        if (!initialThemeChoice) {
          activeThemeChoice = isThemeChoice(settings.theme) ? settings.theme : "system";
        }
        applySettingsSnapshot(settings);
        await loadLocale(settings.language).catch(() => undefined);
        if (cancelled) return;
        applySettingsSnapshot(settings);
        settingsStatus = initialMode ? "preview" : "ready";
      })
      .catch(async () => {
        if (cancelled) return;
        if (!initialMode) initUiMode(null);
        const previewLanguage = storedPreviewLanguage();
        generalLanguageChoice = previewLanguage ?? "";
        await loadLocale(previewLanguage).catch(() => undefined);
        if (cancelled) return;
        settingsSnapshotLabel = tr("gui.settings.snapshot.defaults_active", "Defaults active");
        settingsStatus = "preview";
      });

    return () => {
      cancelled = true;
    };
  });

  onMount(() => {
    if (runtimePreviews.dropPaths.length > 0) {
      void handleDroppedPaths(runtimePreviews.dropPaths, "preview");
    }
  });

  onMount(() => {
    ipc.takeValidationDropPaths()
      .then((paths) => {
        if (paths.length > 0) void handleDroppedPaths(paths, "validation");
      })
      .catch(() => {
        // Dev preview and normal sessions have no packaged validation paths.
      });
  });

  onMount(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) =>
        getCurrentWindow().onDragDropEvent((event) => {
          if (cancelled) return;
          if (event.payload.type === "enter" || event.payload.type === "over") {
            dragActive = true;
          } else if (event.payload.type === "leave") {
            dragActive = false;
          } else if (event.payload.type === "drop") {
            dragActive = false;
            void handleDroppedPaths(event.payload.paths, "native");
          }
        }),
      )
      .then((dispose) => {
        unlisten = dispose;
      })
      .catch(() => {
        // Dev preview has no Tauri native file-drop event bus.
      });

    const onDragOver = (event: DragEvent) => {
      event.preventDefault();
      dragActive = true;
    };
    const onDragLeave = (event: DragEvent) => {
      if (event.relatedTarget instanceof Node && document.body.contains(event.relatedTarget)) return;
      dragActive = false;
    };
    const onDrop = (event: DragEvent) => {
      event.preventDefault();
      dragActive = false;
      const paths = pathsFromDomDrop(event);
      if (paths.length > 0) void handleDroppedPaths(paths, "dom");
    };

    window.addEventListener("dragover", onDragOver);
    window.addEventListener("dragleave", onDragLeave);
    window.addEventListener("drop", onDrop);

    return () => {
      cancelled = true;
      unlisten?.();
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("dragleave", onDragLeave);
      window.removeEventListener("drop", onDrop);
    };
  });

  onMount(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (activePopover === "quickActions") {
        if (quickActionPopover?.contains(target) || quickActionButton?.contains(target)) return;
        closeQuickActions();
      }
      if (entryContext && !entryContextMenu?.contains(target)) {
        closeEntryContext();
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (activePopover === "quickActions") {
          event.preventDefault();
          closeQuickActions();
        }
        if (entryContext) {
          event.preventDefault();
          closeEntryContext();
        }
        if (handleWorkflowEscape()) {
          event.preventDefault();
        }
      }
    };

    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);

    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  });

  $effect(() => {
    for (const task of jobRows) {
      if (
        task.spec.kind === "update" &&
        task.state === "done" &&
        currentArchive?.path === task.spec.path &&
        !refreshedUpdateJobs.has(task.id)
      ) {
        refreshedUpdateJobs.add(task.id);
        void refreshCurrentArchive().then((ok) => {
          if (ok) showNotice(tr("gui.archive.list_refreshed", "Archive list refreshed"));
        });
      }
    }
  });

  function syncUrl(nextMode: Mode = mode) {
    const url = new URL(window.location.href);
    url.searchParams.set("mode", nextMode);
    url.searchParams.set("screen", screen);
    url.searchParams.set("palette", activePalette);
    url.searchParams.set("theme", activeThemeChoice);
    url.searchParams.set("density", activeDensityChoice);
    url.searchParams.delete("firstRun");
    if (url.href !== window.location.href) {
      window.history.replaceState(null, "", url);
    }
  }

  function suppressBrowserContextMenu(event: MouseEvent) {
    event.preventDefault();
  }

  function suppressBrowserDebugShortcut(event: KeyboardEvent) {
    if (isBrowserDebugShortcut(event)) {
      event.preventDefault();
    }
  }

  function isBrowserDebugShortcut(event: KeyboardEvent): boolean {
    const key = event.key.toLowerCase();
    if (event.key === "F12") return true;
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && devToolsChordKeys.has(key)) return true;
    if (event.metaKey && event.altKey && devToolsChordKeys.has(key)) return true;
    return (event.ctrlKey || event.metaKey) && key === "u";
  }

  function setMode(next: Mode) {
    void persistUiMode(next).catch(() => {
      showNotice(tr("gui.mode.saved_preview_desktop_unavailable", "Interface mode saved for this preview · desktop service unavailable"));
    });
    syncUrl(next);
  }

  function setScreen(next: Screen) {
    screen = next;
    syncUrl();
    void tick().then(() => {
      document.documentElement.scrollTop = 0;
      document.body.scrollTop = 0;
      for (const element of document.querySelectorAll<HTMLElement>(
        ".modern-content, .modern-content.settings-workspace > :not(.settings-workspace-rail), .settings-workspace-rail, .classic-dialog-body",
      )) {
        element.scrollTop = 0;
        element.scrollLeft = 0;
      }
    });
  }

  async function focusChecksumResultPanel(kind: "checksum" | "checksum_check" = "checksum"): Promise<void> {
    await tick();
    const panel = kind === "checksum_check"
      ? checksumCheckResultPanel ?? checksumResultPanel
      : checksumResultPanel;
    if (!panel) return;
    panel.scrollIntoView({ block: "nearest", inline: "nearest" });
    panel.focus({ preventScroll: true });
  }

  $effect(() => {
    if (!import.meta.env.DEV || !params.has("validationTrace")) return;
    const win = window as ValidationWindow;
    win.__squallzValidationJobSubmitAttempts = 0;
    win.__squallzValidationJobSubmitBlockedWhileStarting = 0;
    win.__squallzValidationSetScreen = (next: Screen) => {
      if (!screenIds.includes(next)) return false;
      setScreen(next);
      return true;
    };
    return () => {
      delete win.__squallzValidationSetScreen;
      delete win.__squallzValidationJobSubmitAttempts;
      delete win.__squallzValidationJobSubmitBlockedWhileStarting;
    };
  });

  function cancelConflictPrompt() {
    if (jobConflictPrompt) {
      answerConflictDecision("skip", false);
    } else {
      setScreen("extract");
    }
  }

  function handleWorkflowEscape(): boolean {
    if (screen === "password") {
      cancelJobPassword();
      return true;
    }
    if (screen === "conflict") {
      cancelConflictPrompt();
      return true;
    }
    if (screen === "cannotRepair") {
      setScreen("recovery");
      return true;
    }
    return false;
  }

  function applyCreatePreflightEvent(event: CreatePreflightEvent) {
    const scanned = Number(event.scanned ?? 0);
    if (Number.isFinite(scanned)) createPreflightScanned = scanned;
    createPreflightCurrent = String(event.current ?? "");
  }

  function ensureCreatePreflightListener(): Promise<void> {
    if (createPreflightCleanup || createPreflightClosed) return Promise.resolve();
    if (createPreflightListenPromise) return createPreflightListenPromise;
    createPreflightListenPromise = import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<CreatePreflightEvent>("create://preflight", (event) => {
          if (!createPreflightClosed) applyCreatePreflightEvent(event.payload);
        }),
      )
      .then((dispose) => {
        if (createPreflightClosed) {
          dispose();
        } else {
          createPreflightCleanup = dispose;
        }
      })
      .catch(() => {
        // Dev preview has no native Tauri event bus.
      })
      .finally(() => {
        createPreflightListenPromise = null;
      });
    return createPreflightListenPromise;
  }

  function setPalette(next: PaletteId) {
    activePalette = next;
    if (next === "custom") {
      customAccentInput = normalizeHexColor(customAccentInput) ?? customAccent;
      customAccentSaveError = false;
    } else {
      customAccentSaveError = false;
    }
    syncUrl();
  }

  function updateCustomAccent(value: string, source: "color" | "hex") {
    activePalette = "custom";
    const normalized = normalizeHexColor(value);
    if (normalized) {
      customAccent = normalized;
      customAccentInput = normalized;
      customAccentSaveError = false;
    } else if (source === "hex") {
      customAccentInput = value.trim().toUpperCase();
      customAccentSaveError = false;
    }
    syncUrl();
  }

  function onCustomAccentHexInput(event: Event) {
    updateCustomAccent((event.currentTarget as HTMLInputElement).value, "hex");
  }

  function updateCustomAccentFromWheel(event: PointerEvent) {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const size = Math.min(rect.width, rect.height);
    const x = Math.max(0, Math.min(size, event.clientX - rect.left));
    const y = Math.max(0, Math.min(size, event.clientY - rect.top));
    updateCustomAccent(colorFromWheelPoint(x, y, size), "color");
  }

  function updateCustomAccentFromWheelClick(event: MouseEvent) {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) {
      return;
    }
    const size = Math.min(rect.width, rect.height);
    const x = Math.max(0, Math.min(size, event.clientX - rect.left));
    const y = Math.max(0, Math.min(size, event.clientY - rect.top));
    updateCustomAccent(colorFromWheelPoint(x, y, size), "color");
  }

  function onColorWheelPointerDown(event: PointerEvent) {
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    updateCustomAccentFromWheel(event);
  }

  function onColorWheelPointerMove(event: PointerEvent) {
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      updateCustomAccentFromWheel(event);
    }
  }

  function onColorWheelPointerEnd(event: PointerEvent) {
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
  }

  function onColorWheelKeydown(event: KeyboardEvent) {
    const hsl = colorWheelHsl();
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

  function customAccentForSave(): string | null {
    const normalized = normalizeHexColor(customAccentInput);
    if (activePalette === "custom") {
      return normalized;
    }
    return normalizeHexColor(customAccent) ?? defaultCustomAccent;
  }

  function palettePayloadForSave(): { palette: PaletteId; customAccent: string; contrastGuard: boolean } | null {
    const customAccentPayload = customAccentForSave();
    if (!customAccentPayload) {
      customAccentSaveError = true;
      return null;
    }
    customAccent = customAccentPayload;
    customAccentInput = customAccentPayload;
    customAccentSaveError = false;
    return { palette: activePalette, customAccent: customAccentPayload, contrastGuard: accentContrastGuard };
  }

  function customAccentStatusLabel(): string {
    if (!customAccentValid) {
      return tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color");
    }
    return accentContrastGuard
      ? tr("gui.colors.light_dark_auto", "Light and dark variants are generated automatically")
      : tr("gui.colors.direct_accent", "Direct accent preview · semantic colors stay locked");
  }

  function setTheme(next: ThemeChoice) {
    activeThemeChoice = next;
    syncUrl();
    void ipc.setTheme(next).catch(() => {
      showNotice(tr("gui.theme.saved_preview_desktop_unavailable", "Theme saved for this preview · desktop service unavailable"));
    });
  }

  function setDensity(next: DensityChoice) {
    activeDensityChoice = next;
    syncUrl();
    void ipc.setUiDensity(next).catch(() => {
      showNotice(tr("gui.density.saved_preview_desktop_unavailable", "Density saved for this preview · desktop service unavailable"));
    });
  }

  function toggleQuickActions() {
    activePopover = activePopover === "quickActions" ? null : "quickActions";
  }

  function closeQuickActions(restoreFocus = true) {
    activePopover = null;
    if (restoreFocus) queueMicrotask(() => quickActionButton?.focus());
  }

  function chooseQuickAction(next: Screen) {
    setScreen(next);
    closeQuickActions();
  }

  function modeIs(next: Mode): boolean {
    return uiModeChoice() === next || (uiModeChoice() === null && mode === next);
  }

  function isThemeChoice(value: string | null): value is ThemeChoice {
    return value === "system" || value === "light" || value === "dark";
  }

  function isDensityChoice(value: string | null): value is DensityChoice {
    return value === "compact" || value === "standard" || value === "comfort";
  }

  function isPaletteId(value: string | null): value is PaletteId {
    return paletteIds.includes(value as PaletteId);
  }

  function isCreateProfileId(value: string | null): value is CreateProfileId {
    return createProfileIds.includes(value as CreateProfileId);
  }

  function isCreateFormatId(value: string | null): value is CreateFormatId {
    return createFormatIds.includes(value as CreateFormatId);
  }

  function archiveFormatDto(
    id: string,
    extensions: string[],
    caps: {
      canCreate?: boolean;
      canExtract?: boolean;
      canEncryptData?: boolean;
      canEncryptNames?: boolean;
      canSplit?: boolean;
      canUpdate?: boolean;
    },
  ): FormatDto {
    const canCreate = caps.canCreate === true;
    const canExtract = caps.canExtract === true;
    return {
      id,
      extensions,
      kind: "archive",
      can_create: canCreate,
      can_extract: canExtract,
      can_encrypt_data: caps.canEncryptData === true,
      can_encrypt_names: caps.canEncryptNames === true,
      can_split: caps.canSplit === true,
      can_update: caps.canUpdate === true,
      can_test: canExtract,
    };
  }

  function compressorFormat(id: string, extensions: string[]): FormatDto {
    return {
      id,
      extensions,
      kind: "compressor",
      can_create: true,
      can_extract: true,
      can_encrypt_data: false,
      can_encrypt_names: false,
      can_split: true,
      can_update: false,
      can_test: true,
    };
  }

  function registryFormats(): FormatDto[] {
    const loaded = allFormats();
    return loaded.length > 0 ? loaded : fallbackFormats;
  }

  function registryFormatExtensions(): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const format of registryFormats()) {
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
    return registryFormats().filter((format) => format.kind === "archive");
  }

  function formatRegistrySourceLabel(): string {
    return allFormats().length > 0
      ? tr("gui.settings.integration.format_registry", "Format registry")
      : tr("gui.settings.integration.preview_registry", "Preview registry");
  }

  function formatDisplayName(id: string): string {
    if (id === "sqz") return "SQZ";
    if (id === "tar.zst") return "TAR.ZST";
    if (id === "tar.gz") return "TAR.GZ";
    if (id === "tar.xz") return "TAR.XZ";
    if (id === "tar.bz2") return "TAR.BZ2";
    if (id === "tgz") return "TAR.GZ";
    if (id === "txz") return "TAR.XZ";
    if (id === "tbz2") return "TAR.BZ2";
    if (id === "tzst") return "TAR.ZST";
    if (id === "bzip2") return "BZIP2";
    if (id === "bz2") return "BZIP2";
    if (id === "gzip") return "GZIP";
    if (id === "gz") return "GZIP";
    if (id === "zstd") return "ZSTD";
    if (id === "zst") return "ZSTD";
    if (id === "br") return "BROTLI";
    return id.toUpperCase();
  }

  function formatIsExternal(format: FormatDto): boolean {
    return format.id === "wim" || format.id === "rar" || longTailBridgeFormatIds.has(format.id);
  }

  function formatStateLabel(format: FormatDto): string {
    if (format.id === "zip") return tr("gui.format.state.default", "Default");
    if (format.id === "sqz") return tr("gui.format.state.recovery_container", "Recovery container");
    if (format.id === "wim") return tr("gui.format.state.external_writer", "External writer");
    if (format.id === "rar") return tr("gui.format.state.open_only", "Open only");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.format.state.7zz_bridge", "7zz bridge");
    if (format.id.startsWith("tar.")) return tr("gui.format.state.compound", "Compound");
    if (format.kind === "compressor") return tr("gui.format.state.stream_codec", "Stream codec");
    return tr("gui.format.state.built_in", "Built-in");
  }

  function formatCreateLabel(format: FormatDto): string {
    if (!format.can_create) return tr("common.no", "No");
    if (format.id === "wim") return tr("gui.format.create.if_wimlib", "If wimlib exists");
    return tr("common.yes", "Yes");
  }

  function formatEncryptLabel(format: FormatDto): string {
    if (format.can_encrypt_names) return tr("gui.format.encrypt.names_data", "Names + data");
    if (format.can_encrypt_data) return tr("gui.format.encrypt.data", "Data");
    return tr("common.no", "No");
  }

  function formatNote(format: FormatDto): string {
    if (format.id === "rar") return tr("gui.format.note.rar_read_only", "Read-only; no RAR creation or .rev claim");
    if (format.id === "wim") return tr("gui.format.note.wim_external", "WIM create requires wimlib-imagex; read uses 7zz/7z");
    if (format.id === "sqz") return tr("gui.format.note.sqz_recovery", "Embedded recovery container with export");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.format.note.7zz_bridge", "Unpack-only through the 7zz/7z bridge");
    if (format.id.startsWith("tar.")) return tr("gui.format.note.compound_tar", "Compound TAR stream; no encryption claim");
    return format.extensions.length > 0
      ? tr("gui.format.note.extensions", "Extensions {extensions}").replace("{extensions}", `.${format.extensions.slice(0, 3).join(", .")}`)
      : tr("gui.format.note.registry_capability", "Registry capability");
  }

  function associationFormatLabel(format: FormatDto, extension: string): string {
    const display = formatDisplayName(format.id);
    const ext = extension.toLowerCase();
    if (format.id === "zip" && ext !== "zip") return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", "ZIP");
    if (format.id === "rar" && ext !== "rar") return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", "RAR");
    if (format.id.startsWith("tar.") && ext !== format.id) return tr("gui.settings.integration.format_alias", "{format} alias").replace("{format}", display);
    if (format.kind === "compressor") return tr("gui.settings.integration.format_stream", "{format} stream").replace("{format}", display);
    return display;
  }

  function associationStatusLabel(format: FormatDto): string {
    if (format.kind === "compressor") return openWithLabel();
    if (format.id === "wim") return tr("gui.settings.integration.open_with_writer", "{openWith} + writer").replace("{openWith}", openWithLabel());
    if (!format.can_create && format.can_extract) return tr("gui.settings.integration.open_with_read_only", "{openWith} / read only").replace("{openWith}", openWithLabel());
    return openWithLabel();
  }

  function associationActionLabel(format: FormatDto, extension: string): string {
    const ext = extension.toLowerCase();
    if (format.id === "sqz") return tr("gui.settings.integration.action_browse_extract_test_export", "Browse, extract, test, export");
    if (format.id === "wim") return tr("gui.settings.integration.action_wim", "Browse, extract via 7zz/7z; create via wimlib");
    if (format.id === "rar") return ext === "cbr"
      ? tr("gui.settings.integration.action_comics_7zz", "Browse comics via 7zz/7z")
      : tr("gui.settings.integration.action_7zz_bridge", "Browse, extract via 7zz/7z bridge");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.settings.integration.action_7zz_bridge", "Browse, extract via 7zz/7z bridge");
    if (format.id === "7z") return tr("gui.settings.integration.action_browse_extract_convert", "Browse, extract, convert");
    if (format.kind === "compressor") return tr("gui.settings.integration.action_decompress_stream", "Decompress stream");
    if (ext === "cbz") return tr("gui.settings.integration.action_browse_comics_extract", "Browse comics, extract");
    if (format.id.startsWith("tar.")) return tr("gui.settings.integration.action_extract_convert", "Extract, convert");
    return tr("gui.settings.integration.action_browse_extract_test", "Browse, extract, test");
  }

  function associationRows(): AssociationRow[] {
    const seen = new Set<string>();
    const rows: AssociationRow[] = [];
    const sortedFormats = registryFormats()
      .slice()
      .sort((a, b) => formatSortRank(a).localeCompare(formatSortRank(b)));
    for (const format of sortedFormats) {
      for (const rawExtension of format.extensions) {
        const normalized = rawExtension.toLowerCase().replace(/^\.+/, "").trim();
        if (!normalized || seen.has(normalized)) continue;
        seen.add(normalized);
        rows.push({
          ext: `.${normalized}`,
          format: associationFormatLabel(format, normalized),
          status: associationStatusLabel(format),
          action: associationActionLabel(format, normalized),
        });
      }
    }
    rows.push(
      {
        ext: ".par2",
        format: tr("gui.settings.integration.par2_sidecar", "PAR2 sidecar"),
        status: tr("gui.settings.integration.sidecar", "Sidecar"),
        action: tr("gui.settings.integration.verify_repair", "Verify, repair"),
      },
      {
        ext: ".001",
        format: tr("gui.settings.integration.split_volume", "Split volume"),
        status: tr("gui.settings.integration.not_claimed", "Not claimed"),
        action: tr("gui.settings.integration.open_first_volume", "Open first known volume"),
      },
      {
        ext: ".z01",
        format: tr("gui.settings.integration.zip_split", "ZIP split"),
        status: tr("gui.settings.integration.not_claimed", "Not claimed"),
        action: tr("gui.settings.integration.use_zip_head", "Use .zip head file"),
      },
    );
    return rows;
  }

  function associationSummary(): string[] {
    const archiveFormats = archiveRegistryFormats();
    const writableArchives = archiveFormats.filter((format) => format.can_create).length;
    const unpackOnlyArchives = archiveFormats.filter((format) => !format.can_create && format.can_extract).length;
    return [
      tr("gui.settings.integration.summary_registry_extensions", "{count} registry extensions").replace("{count}", String(registryFormatExtensions().length)),
      tr("gui.settings.integration.summary_archive_families", "{count} archive families").replace("{count}", String(archiveFormats.length)),
      tr("gui.settings.integration.summary_writable", "{count} writable").replace("{count}", String(writableArchives)),
      tr("gui.settings.integration.summary_unpack_only", "{count} unpack-only").replace("{count}", String(unpackOnlyArchives)),
      tr("gui.settings.integration.summary_sidecar_rules", "3 sidecar/split rules"),
    ];
  }

  function formatSortRank(format: FormatDto): string {
    const featured = featuredFormatIds.indexOf(format.id);
    if (featured >= 0) return `0-${featured.toString().padStart(2, "0")}`;
    if (format.can_create && format.can_extract) return `1-${format.id}`;
    if (format.can_extract) return `2-${format.id}`;
    return `3-${format.id}`;
  }

  function formatCapabilityCards(): FormatCapabilityCard[] {
    return archiveRegistryFormats()
      .slice()
      .sort((a, b) => formatSortRank(a).localeCompare(formatSortRank(b)))
      .map((format) => ({
        id: format.id,
        name: formatDisplayName(format.id),
        state: formatStateLabel(format),
        create: formatCreateLabel(format),
        split: format.can_split ? "Yes" : "No",
        encrypt: formatEncryptLabel(format),
        note: formatNote(format),
      }));
  }

  function featuredFormatCards(): FormatCapabilityCard[] {
    const cards = formatCapabilityCards();
    const byId = new Map(cards.map((card) => [card.id, card]));
    const featured = featuredFormatIds
      .map((id) => byId.get(id))
      .filter((card): card is FormatCapabilityCard => Boolean(card));
    const rest = cards.filter((card) => !featuredFormatIds.includes(card.id));
    return [...featured, ...rest].slice(0, 8);
  }

  function formatExamples(formats: FormatDto[], max = 7): string {
    if (formats.length === 0) return tr("common.none", "None");
    const sorted = formats
      .slice()
      .sort((a, b) => formatSortRank(a).localeCompare(formatSortRank(b)));
    const shown = sorted.slice(0, max).map((format) => formatDisplayName(format.id));
    const hidden = sorted.length - shown.length;
    return hidden > 0 ? `${shown.join(", ")} +${hidden}` : shown.join(", ");
  }

  function formatCoverageRows(): FormatCoverageRow[] {
    const formats = registryFormats();
    const archives = formats.filter((format) => format.kind === "archive");
    const packUnpack = archives.filter((format) => format.can_create && format.can_extract);
    const unpackOnly = archives.filter((format) => !format.can_create && format.can_extract);
    const codecs = formats.filter((format) => format.kind === "compressor");
    const external = archives.filter((format) => formatIsExternal(format));
    return [
      { label: tr("gui.format.coverage.pack_unpack", "Pack / unpack"), value: String(packUnpack.length), detail: formatExamples(packUnpack) },
      { label: tr("gui.format.coverage.unpack_only", "Unpack only"), value: String(unpackOnly.length), detail: formatExamples(unpackOnly) },
      { label: tr("gui.format.coverage.stream_codecs", "Stream codecs"), value: String(codecs.length), detail: formatExamples(codecs, 6) },
      { label: tr("gui.format.coverage.external_bridge", "External bridge"), value: String(external.length), detail: tr("gui.format.coverage.external_bridge_detail", "7zz/7z and wimlib boundaries stay visible") },
      { label: tr("gui.format.coverage.recovery", "Recovery"), value: "3", detail: tr("gui.format.coverage.recovery_detail", "PAR2 sidecars, .sqz embedded, .sqz.rev sidecars") },
    ];
  }

  function loadCreateProfile(): CreateProfileId {
    try {
      const value = window.localStorage.getItem("squallz.createProfile");
      return isCreateProfileId(value) ? value : "balanced";
    } catch {
      return "balanced";
    }
  }

  function loadCreateFormat(): CreateFormatId {
    try {
      const value = window.localStorage.getItem("squallz.createFormat");
      return isCreateFormatId(value) ? value : "7z";
    } catch {
      return "7z";
    }
  }

  function clampCreateLevel(value: number): number {
    if (!Number.isFinite(value)) return 6;
    return Math.min(9, Math.max(1, Math.round(value)));
  }

  function customCreateLevelInvalidMessage(): string {
    return tr("gui.create.custom_level_invalid", "Use a compression level from 1 to 9");
  }

  function parseCustomCreateLevelInput(input: HTMLInputElement): number | null {
    const raw = input.value.trim();
    const next = Number(raw);
    if (!raw || !Number.isFinite(next) || !Number.isInteger(next) || next < 1 || next > 9) {
      return null;
    }
    return next;
  }

  function loadCustomCreateLevel(): number {
    try {
      const raw = window.localStorage.getItem("squallz.customCreateLevel");
      return raw === null ? 6 : clampCreateLevel(Number(raw));
    } catch {
      return 6;
    }
  }

  function sanitizeCustomProfileName(value: string): string {
    return value.replace(/\s+/g, " ").trim().slice(0, 24);
  }

  function customCreateProfileNameRequiredMessage(): string {
    return tr("gui.create.custom_profile_name_required", "Enter a custom profile name");
  }

  function requireCustomCreateProfileName(): string | null {
    const name = sanitizeCustomProfileName(customCreateProfileName);
    if (name) {
      customCreateProfileNameError = "";
      return name;
    }
    customCreateProfileNameError = customCreateProfileNameRequiredMessage();
    showNotice(customCreateProfileNameError);
    return null;
  }

  function fallbackCustomProfile(): CustomCreateProfile {
    return {
      id: "custom-default",
      name: "Custom",
      level: loadCustomCreateLevel(),
    };
  }

  function normalizeCustomCreateProfile(input: unknown, index: number): CustomCreateProfile | null {
    if (!input || typeof input !== "object") return null;
    const raw = input as Partial<CustomCreateProfile>;
    const name = sanitizeCustomProfileName(typeof raw.name === "string" ? raw.name : "");
    if (!name) return null;
    return {
      id: typeof raw.id === "string" && raw.id ? raw.id : `custom-${index + 1}`,
      name,
      level: clampCreateLevel(Number(raw.level)),
    };
  }

  function loadCustomCreateProfiles(): CustomCreateProfile[] {
    try {
      const raw = window.localStorage.getItem(customCreateProfilesKey);
      if (!raw) return [fallbackCustomProfile()];
      const parsed = JSON.parse(raw) as unknown;
      if (!Array.isArray(parsed)) return [fallbackCustomProfile()];
      const normalized = parsed
        .map((item, index) => normalizeCustomCreateProfile(item, index))
        .filter((item): item is CustomCreateProfile => item !== null)
        .slice(0, maxCustomCreateProfiles);
      return normalized.length > 0 ? normalized : [fallbackCustomProfile()];
    } catch {
      return [fallbackCustomProfile()];
    }
  }

  function activeCustomProfileSnapshot(
    profiles = customCreateProfiles,
    id = activeCustomCreateProfileId,
  ): CustomCreateProfile {
    return profiles.find((profile) => profile.id === id) ?? profiles[0] ?? fallbackCustomProfile();
  }

  function loadActiveCustomCreateProfileId(profiles: CustomCreateProfile[]): string {
    try {
      const raw = window.localStorage.getItem(activeCustomCreateProfileKey);
      return profiles.some((profile) => profile.id === raw) ? String(raw) : profiles[0].id;
    } catch {
      return profiles[0].id;
    }
  }

  function persistCreateProfile(next: CreateProfileId) {
    try {
      window.localStorage.setItem("squallz.createProfile", next);
    } catch {
      // Profile choice is a convenience setting; browser storage failure is non-fatal.
    }
  }

  function persistCreateFormat(next: CreateFormatId) {
    try {
      window.localStorage.setItem("squallz.createFormat", next);
    } catch {
      // Format preference only affects the next save-dialog default.
    }
  }

  function persistActiveCustomCreateProfile(id: string) {
    try {
      window.localStorage.setItem(activeCustomCreateProfileKey, id);
    } catch {
      // Custom profile choice is non-critical.
    }
  }

  function persistCustomCreateProfiles(next: CustomCreateProfile[]) {
    try {
      window.localStorage.setItem(customCreateProfilesKey, JSON.stringify(next.slice(0, maxCustomCreateProfiles)));
    } catch {
      // Custom profile list is non-critical; private-mode failures should not block jobs.
    }
  }

  function persistCustomCreateLevel(next: number) {
    try {
      window.localStorage.setItem("squallz.customCreateLevel", String(next));
    } catch {
      // Custom profile is non-critical; private-mode failures should not block jobs.
    }
  }

  function createProfileData(profileId: CreateProfileId) {
    if (profileId === "custom") {
      const profile = activeCustomProfileSnapshot();
      return {
        label: profile.name,
        level: customCreateLevel,
        detail: `${profile.name} level ${customCreateLevel} saved for future jobs`,
      };
    }
    return createProfiles[profileId];
  }

  function activateCustomProfile(profile: CustomCreateProfile) {
    customCreateLevelError = "";
    customCreateProfileNameError = "";
    activeCustomCreateProfileId = profile.id;
    customCreateLevel = profile.level;
    customCreateProfileName = profile.name;
    persistActiveCustomCreateProfile(profile.id);
    persistCustomCreateLevel(profile.level);
  }

  function chooseCreateProfile(next: CreateProfileId) {
    activeCreateProfile = next;
    persistCreateProfile(next);
    if (next === "custom") activateCustomProfile(activeCustomProfileSnapshot());
    const profile = createProfileData(next);
    recordOperation({
      status: "info",
      title: tr("gui.create.profile_selected_operation", "Profile selected"),
      detail: `${profile.label} · level ${profile.level}`,
    });
    showNotice(tr("gui.create.profile_selected_notice", "{profile} profile selected").replace("{profile}", profile.label));
  }

  function activeCreateFormatData() {
    return createFormats[activeCreateFormat];
  }

  function chooseCreateFormat(next: CreateFormatId) {
    activeCreateFormat = next;
    persistCreateFormat(next);
    const format = activeCreateFormatData();
    recordOperation({
      status: "info",
      title: tr("gui.create.format_selected_operation", "Create format selected"),
      detail: `${format.label} · .${format.extension}`,
    });
    showNotice(tr("gui.create.format_selected_notice", "{format} format selected").replace("{format}", format.label));
  }

  function chooseCustomCreateProfile(id: string) {
    const profile = customCreateProfiles.find((item) => item.id === id);
    if (!profile) return;
    activeCreateProfile = "custom";
    persistCreateProfile("custom");
    activateCustomProfile(profile);
    recordOperation({
      status: "info",
      title: tr("gui.create.custom_profile_selected_operation", "Custom profile selected"),
      detail: `${profile.name} · level ${profile.level}`,
    });
    showNotice(tr("gui.create.profile_selected_notice", "{profile} profile selected").replace("{profile}", profile.name));
  }

  function setCustomProfiles(next: CustomCreateProfile[]) {
    customCreateProfiles = next.length > 0 ? next.slice(0, maxCustomCreateProfiles) : [fallbackCustomProfile()];
    persistCustomCreateProfiles(customCreateProfiles);
  }

  function updateCustomCreateLevel(value: number, commit = false) {
    const next = clampCreateLevel(value);
    customCreateLevelError = "";
    customCreateLevel = next;
    setCustomProfiles(
      customCreateProfiles.map((profile) =>
        profile.id === activeCustomCreateProfileId ? { ...profile, level: next } : profile,
      ),
    );
    persistCustomCreateLevel(next);
    if (activeCreateProfile !== "custom") {
      activeCreateProfile = "custom";
      persistCreateProfile("custom");
    }
    if (commit) {
      recordOperation({
        status: "info",
        title: tr("gui.create.custom_profile_updated_operation", "Custom profile updated"),
        detail: `${activeCustomProfileSnapshot().name} · level ${next}`,
      });
      showNotice(
        tr("gui.create.profile_level_saved_notice", "{profile} profile level {level} saved")
          .replace("{profile}", activeCustomProfileSnapshot().name)
          .replace("{level}", String(next)),
      );
    }
  }

  function updateCustomCreateLevelFromInput(event: Event, commit = false) {
    const input = event.currentTarget as HTMLInputElement;
    const next = parseCustomCreateLevelInput(input);
    if (next === null) {
      customCreateLevelError = customCreateLevelInvalidMessage();
      showNotice(customCreateLevelError);
      return;
    }
    updateCustomCreateLevel(next, commit);
  }

  function updateCustomCreateProfileNameFromInput(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    customCreateProfileName = input.value;
    if (customCreateProfileNameError && sanitizeCustomProfileName(input.value)) {
      customCreateProfileNameError = "";
    }
  }

  function uniqueCustomProfileName(base: string, excludeActive = true): string {
    const clean = sanitizeCustomProfileName(base) || "Custom";
    const existing = new Set(
      customCreateProfiles
        .filter((profile) => !excludeActive || profile.id !== activeCustomCreateProfileId)
        .map((profile) => profile.name.toLowerCase()),
    );
    if (!existing.has(clean.toLowerCase())) return clean;
    for (let index = 2; index <= 99; index += 1) {
      const candidate = `${clean} ${index}`;
      if (!existing.has(candidate.toLowerCase())) return candidate;
    }
    return `${clean} ${Date.now().toString(36).slice(-3)}`;
  }

  function saveActiveCustomCreateProfile() {
    if (customCreateLevelError) {
      showNotice(customCreateLevelError);
      return;
    }
    const requestedName = requireCustomCreateProfileName();
    if (!requestedName) return;
    const name = uniqueCustomProfileName(requestedName);
    setCustomProfiles(
      customCreateProfiles.map((profile) =>
        profile.id === activeCustomCreateProfileId
          ? { ...profile, name, level: customCreateLevel }
          : profile,
      ),
    );
    customCreateProfileName = name;
    recordOperation({
      status: "info",
      title: tr("gui.create.custom_profile_saved_operation", "Custom profile saved"),
      detail: `${name} · level ${customCreateLevel}`,
    });
    showNotice(tr("gui.create.profile_saved_notice", "{profile} profile saved").replace("{profile}", name));
  }

  function createNewCustomCreateProfile() {
    if (customCreateLevelError) {
      showNotice(customCreateLevelError);
      return;
    }
    const requestedName = requireCustomCreateProfileName();
    if (!requestedName) return;
    if (customCreateProfiles.length >= maxCustomCreateProfiles) {
      showNotice(customProfileLimitMessage());
      return;
    }
    const profile: CustomCreateProfile = {
      id: `custom-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 5)}`,
      name: uniqueCustomProfileName(requestedName, false),
      level: customCreateLevel,
    };
    setCustomProfiles([profile, ...customCreateProfiles]);
    activeCreateProfile = "custom";
    persistCreateProfile("custom");
    activateCustomProfile(profile);
    recordOperation({
      status: "info",
      title: tr("gui.create.custom_profile_saved_operation", "Custom profile saved"),
      detail: `${profile.name} · level ${profile.level}`,
    });
    showNotice(tr("gui.create.profile_saved_notice", "{profile} profile saved").replace("{profile}", profile.name));
  }

  function deleteActiveCustomCreateProfile() {
    if (customCreateProfiles.length <= 1) return;
    const deleted = activeCustomProfileSnapshot();
    const next = customCreateProfiles.filter((profile) => profile.id !== activeCustomCreateProfileId);
    setCustomProfiles(next);
    activateCustomProfile(next[0]);
    recordOperation({
      status: "info",
      title: tr("gui.create.custom_profile_deleted_operation", "Custom profile deleted"),
      detail: deleted.name,
    });
    showNotice(tr("gui.create.profile_deleted_notice", "{profile} profile deleted").replace("{profile}", deleted.name));
  }

  function customProfileDeleteTitle(): string {
    return customCreateProfiles.length <= 1
      ? tr("gui.create.keep_one_custom_profile", "Keep at least one custom profile")
      : "";
  }

  function customProfileLimitMessage(): string {
    return tr("gui.create.custom_profile_limit", "Maximum 8 custom profiles; delete one to save another");
  }

  function customProfileSaveAsNewTitle(): string {
    return customCreateProfiles.length >= maxCustomCreateProfiles ? customProfileLimitMessage() : "";
  }

  function activeCreateProfileData() {
    return createProfileData(activeCreateProfile);
  }

  function createCompressionLevel(): number {
    return activeCreateProfileData().level;
  }

  function createArchivePreviewName(base = "archive"): string {
    return `${base}.${activeCreateFormatData().extension}`;
  }

  function createArchivePreviewPath(base = "archive"): string {
    return `${tr("gui.path.preview_archive_root", "~/Downloads/Archives")}/${createArchivePreviewName(base)}`;
  }

  function createSaveDefaultPath(input: string, base: string): string {
    return `${pathDir(input)}/${createArchivePreviewName(base)}`;
  }

  function createSaveFilters() {
    const activeId = activeCreateFormat;
    const rest = createFormatIds.filter((id) => id !== activeId);
    return [activeId, ...rest].map((formatId) => ({
      name: createFormatFilterName(formatId),
      extensions: createFormats[formatId].extensions,
    }));
  }

  function createFormatFilterName(formatId: CreateFormatId): string {
    return tr(`gui.create.format.${formatId}.filter`, createFormats[formatId].filterName);
  }

  function archiveOutputFilterName(format: "zip" | "7z" | "tar.zst" | "tar" | "sqz"): string {
    return tr(`gui.dialog.filter.${format.replace(".", "_")}`, {
      zip: "ZIP archive",
      "7z": "7Z archive",
      "tar.zst": "TAR.ZST archive",
      tar: "TAR archive",
      sqz: "SQZ container",
    }[format]);
  }

  function createMethodLabel(): string {
    if (activeCreateFormat === "wim") return createFormatMethod();
    if (activeCreateFormat === "sqz") {
      return tr("gui.create.method_profile", "{method} · profile {profile}")
        .replace("{method}", createFormatMethod())
        .replace("{profile}", createProfileLabel(activeCreateProfile));
    }
    return tr("gui.create.method_level_profile", "{method} · Level {level} · {profile}")
      .replace("{method}", createFormatMethod())
      .replace("{level}", String(createCompressionLevel()))
      .replace("{profile}", createProfileLabel(activeCreateProfile));
  }

  function createPasswordCapability(): string {
    return createFormatPassword();
  }

  function createSplitCapability(): string {
    return createFormatSplit();
  }

  function createRecoveryCapability(): string {
    return createFormatRecovery();
  }

  function createFormatNote(): string {
    return createFormatNoteFor();
  }

  function historySummaryCount(): string {
    return historyRows.length > 0
      ? tr("gui.history.recent_activity", "Recent operations")
      : tr("gui.history.no_activity", "No operation history yet");
  }

  function historyLastLabel(): string {
    return historyRows.length > 0
      ? tr("gui.history.local_only", "Local history stored")
      : tr("gui.history.no_activity", "No operation history yet");
  }

  async function exportOperationAuditFromUi() {
    try {
      const { save } = await getDialogModule();
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      const dest = await saveNativeDialog("history.export-operation-audit", save, {
        title: tr("gui.history.export_operation_audit", "Export task audit"),
        defaultPath: `squallz-operation-audit-${stamp}.json`,
        filters: [{ name: tr("gui.dialog.filter.json", "JSON"), extensions: ["json"] }],
      });
      if (!dest) return;
      await ipc.exportOperationAudit(dest);
      recordOperation({
        status: "done",
        title: tr("gui.history.operation_audit_exported", "Task audit exported"),
        detail: tr("gui.history.sanitized_operation_audit", "Sanitized task audit"),
      });
      showNotice(tr("gui.history.operation_audit_exported", "Task audit exported"));
    } catch {
      showNotice(tr("gui.history.operation_audit_requires_desktop_service", "Task audit export requires the desktop service"));
    }
  }

  function themeStatusLabel(): string {
    if (activeThemeChoice === "system") {
      return tr("gui.theme.system_resolved", "System ({theme})").replace(
        "{theme}",
        activeTheme === "dark" ? tr("gui.theme.dark", "Dark") : tr("gui.theme.light", "Light"),
      );
    }
    return activeThemeChoice === "dark" ? tr("gui.theme.dark", "Dark") : tr("gui.theme.light", "Light");
  }

  function densityLabel(value: DensityChoice = activeDensityChoice): string {
    if (value === "compact") return tr("gui.density.compact", "Compact");
    if (value === "comfort") return tr("gui.density.comfort", "Comfort");
    return tr("gui.density.standard", "Standard");
  }

  function applySettingsSnapshot(settings: SettingsDto) {
    const savedCustomAccent = normalizeHexColor(settings.custom_accent);
    if (savedCustomAccent) {
      customAccent = savedCustomAccent;
      customAccentInput = savedCustomAccent;
      customAccentSaveError = false;
    }
    accentContrastGuard = settings.accent_contrast_guard !== false;
    if (!hasPaletteOverride && isPaletteId(settings.accent_palette)) {
      activePalette = settings.accent_palette;
    }
    if (!hasDensityOverride && isDensityChoice(settings.ui_density)) {
      activeDensityChoice = settings.ui_density;
    }
    generalLanguageChoice = settings.language ?? "";
    generalDefaultExtractDir = settings.default_extract_dir ?? "";
    generalRevealAfterExtract = settings.reveal_after_extract === true;
    setRevealAfterExtractPreference(generalRevealAfterExtract);

    safetyMaxOutputGiB =
      settings.safety_max_output_bytes && settings.safety_max_output_bytes > 0
        ? Math.max(1, Math.round(settings.safety_max_output_bytes / bytesPerGiB))
        : defaultSafety.maxOutputGiB;
    safetyMaxEntries =
      settings.safety_max_entries && settings.safety_max_entries > 0
        ? settings.safety_max_entries
        : defaultSafety.maxEntries;
    safetyMaxCompressionRatio =
      settings.safety_max_compression_ratio && settings.safety_max_compression_ratio > 0
        ? settings.safety_max_compression_ratio
        : defaultSafety.maxCompressionRatio;
    performanceThreads =
      settings.performance_threads && settings.performance_threads > 0
        ? Math.min(settings.performance_threads, 64)
        : null;
    performanceMemoryMiB =
      settings.performance_memory_limit_bytes && settings.performance_memory_limit_bytes > 0
        ? wholeSetting(
            Math.round(settings.performance_memory_limit_bytes / bytesPerMiB),
            512,
            1,
            262_144,
          )
        : null;

    const customSafety = Boolean(
      settings.safety_max_output_bytes ||
        settings.safety_max_entries ||
        settings.safety_max_compression_ratio,
    );
    const workerLabel =
      performanceThreads === null
        ? tr("gui.settings.snapshot.workers_auto", "workers auto")
        : tr("gui.settings.snapshot.workers_count", "{count} workers")
            .replace("{count}", String(performanceThreads));
    const memoryLabel =
      performanceMemoryMiB === null
        ? tr("gui.settings.snapshot.buffer_auto", "buffer auto")
        : tr("gui.settings.snapshot.buffer_mib", "{count} MiB buffer")
            .replace("{count}", formattedNumber(performanceMemoryMiB, 512));
    settingsSnapshotLabel = tr("gui.settings.snapshot.summary", "{safety} · {workers} · {buffer}")
      .replace(
        "{safety}",
        customSafety
          ? tr("gui.settings.snapshot.custom_safety", "Custom safety")
          : tr("gui.settings.snapshot.default_safety", "Default safety"),
      )
      .replace("{workers}", workerLabel)
      .replace("{buffer}", memoryLabel);
  }

  function wholeSetting(value: NumericSetting, fallback: number, min: number, max: number): number {
    const numberValue = typeof value === "number" && Number.isFinite(value) ? value : fallback;
    return Math.min(max, Math.max(min, Math.round(numberValue)));
  }

  function showNumericRangeNotice(label: string, min: number, max: number) {
    showNotice(
      tr("gui.settings.number.invalid_range", "{label} must be a whole number from {min} to {max}")
        .replace("{label}", label)
        .replace("{min}", numberFormatter.format(min))
        .replace("{max}", numberFormatter.format(max)),
    );
  }

  function validateRequiredWholeSetting(
    value: NumericSetting,
    min: number,
    max: number,
    label: string,
  ): number | null {
    if (
      typeof value !== "number" ||
      !Number.isFinite(value) ||
      !Number.isInteger(value) ||
      value < min ||
      value > max
    ) {
      showNumericRangeNotice(label, min, max);
      return null;
    }
    return value;
  }

  function validateOptionalWholeSetting(
    value: NumericSetting,
    min: number,
    max: number,
    label: string,
  ): number | null | undefined {
    if (value === null) return null;
    return validateRequiredWholeSetting(value, min, max, label) ?? undefined;
  }

  function formattedNumber(value: NumericSetting, fallback: number): string {
    return numberFormatter.format(wholeSetting(value, fallback, 1, Number.MAX_SAFE_INTEGER));
  }

  async function saveSafetySettings() {
    const maxOutputGiB = validateRequiredWholeSetting(
      safetyMaxOutputGiB,
      1,
      8192,
      tr("gui.settings.security.max_output_gib", "Max output GiB"),
    );
    if (maxOutputGiB === null) return;
    const maxEntries = validateRequiredWholeSetting(
      safetyMaxEntries,
      1,
      10_000_000,
      tr("gui.settings.security.max_entries", "Max entries"),
    );
    if (maxEntries === null) return;
    const maxCompressionRatio = validateRequiredWholeSetting(
      safetyMaxCompressionRatio,
      1,
      100_000,
      tr("gui.settings.security.ratio_guard", "Ratio guard"),
    );
    if (maxCompressionRatio === null) return;
    safetyMaxOutputGiB = maxOutputGiB;
    safetyMaxEntries = maxEntries;
    safetyMaxCompressionRatio = maxCompressionRatio;

    try {
      const settings = await ipc.setSafetyLimits(
        maxOutputGiB * bytesPerGiB,
        maxEntries,
        maxCompressionRatio,
      );
      applySettingsSnapshot(settings);
      showNotice(tr("gui.settings.security.saved", "Security settings saved"));
    } catch {
      settingsSnapshotLabel = tr("gui.settings.snapshot.preview_desktop_unavailable", "Preview values · desktop service unavailable");
      showNotice(tr("gui.settings.security.saved_preview", "Security settings saved for this preview · desktop service unavailable"));
    }
  }

  async function resetSafetySettings() {
    safetyMaxOutputGiB = defaultSafety.maxOutputGiB;
    safetyMaxEntries = defaultSafety.maxEntries;
    safetyMaxCompressionRatio = defaultSafety.maxCompressionRatio;

    try {
      const settings = await ipc.setSafetyLimits(null, null, null);
      applySettingsSnapshot(settings);
      showNotice(tr("gui.settings.security.reset_defaults", "Security settings reset to defaults"));
    } catch {
      settingsSnapshotLabel = tr("gui.settings.snapshot.default_preview_desktop_unavailable", "Default preview · desktop service unavailable");
      showNotice(tr("gui.settings.security.reset_preview", "Security settings reset for this preview · desktop service unavailable"));
    }
  }

  function choosePerformanceThreads(next: NumericSetting) {
    performanceThreads = next;
  }

  function choosePerformanceMemory(next: NumericSetting) {
    performanceMemoryMiB = next;
  }

  async function savePerformanceSettings() {
    const threads = validateOptionalWholeSetting(
      performanceThreads,
      1,
      64,
      tr("gui.settings.performance.custom_threads", "Custom threads"),
    );
    if (threads === undefined) return;
    const memoryMiB = validateOptionalWholeSetting(
      performanceMemoryMiB,
      1,
      262_144,
      tr("gui.settings.performance.custom_buffer_mib", "Custom buffer MiB"),
    );
    if (memoryMiB === undefined) return;
    performanceThreads = threads;
    performanceMemoryMiB = memoryMiB;

    try {
      const settings = await ipc.setPerformanceOptions(
        threads,
        memoryMiB === null ? null : memoryMiB * bytesPerMiB,
      );
      applySettingsSnapshot(settings);
      showNotice(tr("gui.settings.performance.saved", "Performance settings saved"));
    } catch {
      settingsSnapshotLabel = tr("gui.settings.snapshot.preview_desktop_unavailable", "Preview values · desktop service unavailable");
      showNotice(tr("gui.settings.performance.saved_preview", "Performance settings saved for this preview · desktop service unavailable"));
    }
  }

  async function resetPerformanceSettings() {
    performanceThreads = null;
    performanceMemoryMiB = null;

    try {
      const settings = await ipc.setPerformanceOptions(null, null);
      applySettingsSnapshot(settings);
      showNotice(tr("gui.settings.performance.reset_auto_resources", "Performance settings reset to automatic resources"));
    } catch {
      settingsSnapshotLabel = tr("gui.settings.snapshot.auto_resources_preview_desktop_unavailable", "Auto resources preview · desktop service unavailable");
      showNotice(tr("gui.settings.performance.reset_preview", "Performance settings reset for this preview · desktop service unavailable"));
    }
  }

  async function savePaletteSettings() {
    const payload = palettePayloadForSave();
    if (!payload) {
      showNotice(tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color"));
      return;
    }
    try {
      const settings = await ipc.setAccentPalette(payload.palette, payload.customAccent, payload.contrastGuard);
      applySettingsSnapshot(settings);
      syncUrl();
      showNotice(tr("gui.colors.saved", "Theme colors saved"));
    } catch {
      showNotice(tr("gui.colors.saved_preview", "Theme colors saved for this preview · desktop service unavailable"));
    }
  }

  async function saveAppearanceSettings() {
    let saved = true;
    const nextMode = uiModeChoice() ?? mode;

    try {
      await persistUiMode(nextMode);
    } catch {
      saved = false;
    }

    try {
      await ipc.setTheme(activeThemeChoice);
    } catch {
      saved = false;
    }

    try {
      await ipc.setUiDensity(activeDensityChoice);
    } catch {
      saved = false;
    }

    try {
      const payload = palettePayloadForSave();
      if (!payload) {
        saved = false;
      } else {
        const settings = await ipc.setAccentPalette(payload.palette, payload.customAccent, payload.contrastGuard);
        applySettingsSnapshot(settings);
      }
    } catch {
      saved = false;
    }

    syncUrl();
    showNotice(
      saved
        ? tr("gui.appearance.saved", "Appearance settings saved")
        : tr("gui.appearance.saved_preview", "Appearance settings saved for this preview · desktop service unavailable"),
    );
  }

  function languageLabel(tag: string | null): string {
    if (!tag) return tr("gui.settings.language.follow_system", "Follow system");
    const language = availableLanguages.find((item) => item.tag === tag);
    return language ? `${language.name} · ${language.tag}` : tag;
  }

  function tr(key: string, fallback: string): string {
    const value = t(key);
    return value === key ? fallback : value;
  }

  function getDialogModule(): Promise<DialogModule> {
    if (!openDialogModulePromise) {
      openDialogModulePromise = import("@tauri-apps/plugin-dialog").catch((error) => {
        openDialogModulePromise = null;
        throw error;
      });
    }
    return openDialogModulePromise;
  }

  function buildTargetPlatform(): PlatformKind {
    return __SQUALLZ_TARGET_PLATFORM__;
  }

  function platformKind(): PlatformKind {
    return activePlatform;
  }

  function applyWindowChromePlatform(platform: PlatformKind) {
    if (typeof document === "undefined") return;
    document.documentElement.dataset.platform = platform;
    document.documentElement.style.setProperty(
      "--traffic-light-reserve",
      platform === "macos" ? "78px" : "0px",
    );
  }

  function platformNameLabel(): string {
    const platform = platformKind();
    if (platform === "macos") return tr("gui.platform.macos", "macOS");
    if (platform === "windows") return tr("gui.platform.windows", "Windows");
    return tr("gui.platform.linux", "Linux");
  }

  function fileManagerLabel(): string {
    const platform = platformKind();
    if (platform === "macos") return tr("gui.platform.file_manager.macos", "Finder");
    if (platform === "windows") return tr("gui.platform.file_manager.windows", "File Explorer");
    return tr("gui.platform.file_manager.linux", "File manager");
  }

  function secretStoreLabel(): string {
    const platform = platformKind();
    if (platform === "macos") return tr("gui.platform.secret_store.macos", "Keychain");
    if (platform === "windows") return tr("gui.platform.secret_store.windows", "Credential Manager");
    return tr("gui.platform.secret_store.linux", "Secret Service");
  }

  function openWithLabel(): string {
    return tr("gui.settings.integration.open_with", "Open With");
  }

  function labelKey(label: string): string {
    return label.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");
  }

  function navLabel(label: string): string {
    const key = `gui.nav.${labelKey(label)}`;
    return tr(key, label);
  }

  function toolbarLabel(label: string): string {
    const key = `gui.toolbar.${labelKey(label)}`;
    return tr(key, label);
  }

  function actionLabel(label: string): string {
    return tr(`gui.action.${labelKey(label)}`, label);
  }

  function classicCommandLabel(label: string): string {
    return tr(`gui.classic.command.${labelKey(label)}`, label);
  }

  function settingsSectionLabel(label: string): string {
    return tr(`gui.settings.section.${labelKey(label)}`, label);
  }

  function settingsSectionDetail(label: string, detail: string): string {
    return tr(`gui.settings.section.${labelKey(label)}.detail`, detail);
  }

  function quickActionLabel(label: string): string {
    return tr(`gui.quick.${labelKey(label)}`, label);
  }

  function quickActionDetail(label: string, detail: string): string {
    return tr(`gui.quick.${labelKey(label)}.detail`, detail);
  }

  function createProfileLabel(profileId: CreateProfileId): string {
    return tr(`gui.create.profile.${profileId}`, createProfiles[profileId].label);
  }

  function createProfileDetail(profileId: CreateProfileId): string {
    return tr(`gui.create.profile.${profileId}.detail`, createProfiles[profileId].detail);
  }

  function activeCreateProfileDetail(): string {
    return createProfileDetail(activeCreateProfile);
  }

  function createFormatMethod(formatId: CreateFormatId = activeCreateFormat): string {
    return tr(`gui.create.format.${formatId}.method`, createFormats[formatId].method);
  }

  function createFormatPassword(formatId: CreateFormatId = activeCreateFormat): string {
    return tr(`gui.create.format.${formatId}.password`, createFormats[formatId].password);
  }

  function createPasswordDataAvailable(formatId: CreateFormatId = activeCreateFormat): boolean {
    return createFormats[formatId].can_encrypt_data;
  }

  function createNameEncryptionAvailable(formatId: CreateFormatId = activeCreateFormat): boolean {
    return createFormats[formatId].can_encrypt_names;
  }

  function createNameEncryptionCapability(formatId: CreateFormatId = activeCreateFormat): string {
    if (createNameEncryptionAvailable(formatId)) {
      return tr("gui.create.name_encryption_available", "7Z can hide file names");
    }
    if (formatId === "zip") {
      return tr("gui.create.name_encryption_zip_visible", "ZIP names stay visible; use 7Z");
    }
    return tr("gui.create.name_encryption_unavailable", "File name encryption unavailable");
  }

  function createFormatSplit(formatId: CreateFormatId = activeCreateFormat): string {
    return tr(`gui.create.format.${formatId}.split`, createFormats[formatId].split);
  }

  function createFormatRecovery(formatId: CreateFormatId = activeCreateFormat): string {
    return tr(`gui.create.format.${formatId}.recovery`, createFormats[formatId].recovery);
  }

  function createFormatNoteFor(formatId: CreateFormatId = activeCreateFormat): string {
    return tr(`gui.create.format.${formatId}.note`, createFormats[formatId].note);
  }

  function batchArchiveStateLabel(state: string): string {
    return tr(`gui.batch.state.${labelKey(state)}`, state);
  }

  function recoveryModeName(index: number, name: string): string {
    return tr(`gui.recovery.mode.${index}.name`, name);
  }

  function recoveryModeDetail(index: number, detail: string): string {
    return tr(`gui.recovery.mode.${index}.detail`, detail);
  }

  function recoveryModeSize(index: number, size: string): string {
    return tr(`gui.recovery.mode.${index}.size`, size);
  }

  function recoveryBlockStatusLabel(status: string): string {
    return tr(`gui.recovery.block_status.${labelKey(status)}`, status);
  }

  function conflictDecisionLabel(decision: string): string {
    if (decision === "Keep both") return tr("gui.conflict.rename", "Keep both");
    if (decision === "Ask") return tr("gui.extract.overwrite.ask", "Ask");
    if (decision === "Replace") return tr("gui.conflict.overwrite", "Replace");
    if (decision === "Choose") return tr("gui.conflict.choose", "Choose");
    return decision;
  }

  function noArchiveLabel(): string {
    return tr("gui.empty.no_archive_short", "No archive open");
  }

  function openArchiveFirstLabel(): string {
    return tr("gui.empty.open_archive_first", "Open an archive first");
  }

  function noEntriesLabel(): string {
    return tr("gui.empty.no_entries", "No entries");
  }

  function normalizedDefaultExtractDir(): string | null {
    const normalized = generalDefaultExtractDir.trim().replaceAll("\\", "/");
    if (normalized === "/") return "/";
    const value = normalized.replace(/\/+$/g, "");
    if (!value) return null;
    return value;
  }

  function defaultExtractFolderLabel(): string {
    return normalizedDefaultExtractDir() ?? tr("gui.settings.folder.next_to_archive", "Next to archive");
  }

  async function chooseDefaultExtractFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("settings.default-extract-folder", open, {
        title: tr("gui.settings.folder.choose_title", "Choose default extract folder"),
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        generalDefaultExtractDir = selected;
      }
    } catch {
      showNotice(tr("gui.settings.folder.picker_requires_desktop_service", "Folder picker requires the desktop service"));
    }
  }

  function clearDefaultExtractFolder() {
    generalDefaultExtractDir = "";
  }

  async function saveGeneralSettings() {
    const nextLanguage = generalLanguageChoice.trim() || null;
    const defaultExtractDir = normalizedDefaultExtractDir();
    try {
      const settings = await ipc.setGeneralOptions(
        nextLanguage,
        defaultExtractDir,
        generalRevealAfterExtract,
      );
      storePreviewLanguage(settings.language);
      applySettingsSnapshot(settings);
      await loadLocale(settings.language).catch(() => undefined);
      recordOperation({
        status: "done",
        title: tr("gui.settings.general.saved", "General settings saved"),
        detail: tr(
          "gui.settings.general.saved_detail",
          "Language: {language} · Default folder: {folder} · Reveal after extract {reveal}",
        )
          .replace("{language}", languageLabel(settings.language))
          .replace("{folder}", defaultExtractFolderLabel())
          .replace("{reveal}", settings.reveal_after_extract ? tr("common.on", "on") : tr("common.off", "off")),
      });
      showNotice(tr("gui.settings.general.saved", "General settings saved"));
    } catch {
      storePreviewLanguage(nextLanguage);
      setRevealAfterExtractPreference(generalRevealAfterExtract);
      await loadLocale(nextLanguage).catch(() => undefined);
      settingsSnapshotLabel = tr("gui.settings.general.preview_locale", "General preview · locale applied locally");
      showNotice(tr("gui.settings.general.saved_preview", "General settings saved for this preview · locale applied locally"));
    }
  }

  function openRecoveryConfiguration() {
    setScreen("recovery");
    showNotice(
      currentArchive
        ? tr("gui.recovery.route_from_create_current_archive", "Recovery opened for the current archive")
        : tr("gui.recovery.route_from_create_open_archive_first", "Recovery opens separate Protect, Verify, Repair, and Export jobs. Open an archive first."),
    );
  }

  async function openArchiveFromDialog() {
    if (archiveOpenStatus === "opening") return;
    archiveOpenStatus = "opening";
    showNotice(tr("gui.archive.opening_picker", "Opening file picker..."));
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("archive.open", open, {
        title: tr("gui.archive.open_dialog_title", "Open archive"),
        multiple: false,
        directory: false,
        filters: [
          {
            name: tr("gui.archive.filter_archives", "Archives"),
            extensions: registryFormatExtensions(),
          },
        ],
      });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (typeof path === "string") {
        await openArchivePath(path, "dialog");
      }
    } catch {
      showNotice(tr("gui.archive.open_requires_desktop_dialog", "Open archive requires the desktop file dialog"));
    } finally {
      archiveOpenStatus = "idle";
    }
  }

  async function openFirstArchivePath(paths: string[], source: "dialog" | "open-file") {
    const validPaths = paths.filter((item) => typeof item === "string" && item.length > 0);
    const path = validPaths[0];
    if (!path) return;
    batchArchivePaths = validPaths;
    await openArchivePath(path, source);
    if (validPaths.length > 1) {
      showNotice(tr("gui.archive.opened_first_more_batch", "Opened first archive · {count} more ready for batch extract").replace("{count}", String(validPaths.length - 1)));
    }
  }

  async function handleOpenFilesPayload(payload: OpenFilesPayload) {
    const action = externalOpenAction(payload.action);
    if (action) {
      await submitExternalTaskWindow(action, payload.paths, payload.output ?? null);
      return;
    }
    await openFirstArchivePath(payload.paths, "open-file");
  }

  async function submitExternalTaskWindow(action: ExternalOpenAction, paths: string[], output: string | null) {
    function applyTaskWindowSubmitTransition(transition: TaskWindowSubmitTransition) {
      taskWindowLaunchState = transition.state;
      if (transition.notice) showNotice(transition.notice);
    }

    const plan = taskWindowSubmitPlan(
      action,
      buildExternalTaskJobSpec(action, {
        paths,
        output,
        checksumAlgorithm,
        checksumExcludes: checksumExcludeRules(),
        archiveStemName,
      }),
      tr,
    );
    applyTaskWindowSubmitTransition(plan.starting);
    if (!plan.jobSpec) {
      applyTaskWindowSubmitTransition(plan.noSelection);
      return;
    }
    try {
      await submitJob(plan.jobSpec);
      applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "started", tr));
    } catch (error) {
      applyTaskWindowSubmitTransition(
        taskWindowSubmitTransition(action, taskWindowSubmitFailureStatus(isJobSubmitBlocked(error)), tr),
      );
    }
  }

  async function openArchivePath(path: string, source: "dialog" | "open-file") {
    archiveOpenStatus = "opening";
    const ok = await openArchiveStore(path);
    archiveOpenStatus = "idle";
    if (ok) {
      rememberRecent(path);
      recordOperation({
        status: "info",
        title: tr("gui.archive.opened_operation", "Opened archive"),
        detail: pathBaseName(path),
      });
      nestedPreview = null;
      entryPreview = null;
      entryPreviewFailure = null;
      screen = "browse";
      syncUrl();
      showNotice(source === "open-file" ? tr("gui.archive.open_file_loaded", "Open-file archive loaded") : tr("gui.archive.open_loaded", "Open archive loaded"));
      recordValidationRenderReady(`archive-open:${source}`);
    } else {
      showNotice(tr("gui.archive.open_failed_or_password", "Archive open failed or needs a password"));
      if (currentArchive === null) setScreen("password");
    }
  }

  function formatModified(value: number | null): string {
    if (value == null) return "-";
    const date = new Date(value * 1000);
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, "0");
    const day = String(date.getDate()).padStart(2, "0");
    const hour = String(date.getHours()).padStart(2, "0");
    const minute = String(date.getMinutes()).padStart(2, "0");
    return `${year}-${month}-${day} ${hour}:${minute}`;
  }

  function entryType(row: EntryDto): string {
    if (row.entry_type === "dir") return "folder";
    if (row.encrypted) return "locked";
    if (row.encoding !== "utf-8" || row.display.includes("\uFFFD")) return "warning";
    return "file";
  }

  function entryAttributeLabel(row: EntryDto): string {
    const parts: string[] = [];
    if (row.entry_type === "dir") parts.push(tr("gui.attr.folder", "Folder"));
    else if (row.entry_type === "symlink") parts.push(tr("gui.attr.symlink", "Symbolic link"));
    else if (row.entry_type === "hardlink") parts.push(tr("gui.attr.hardlink", "Hard link"));
    else if (row.entry_type === "other") parts.push(tr("gui.attr.other", "Other"));
    else parts.push(tr("gui.attr.file", "File"));
    if (row.encrypted) parts.push(tr("gui.attr.encrypted", "Encrypted"));
    if (row.encoding !== "utf-8" || row.display.includes("\uFFFD")) parts.push(tr("gui.attr.encoding_review", "Encoding review"));
    return parts.join(" · ");
  }

  function toDisplayEntry(row: EntryDto): DisplayEntry {
    const ratio =
      row.compressed && row.size > 0 ? `${Math.round((row.compressed / row.size) * 100)}%` : "-";
    return {
      name: row.display,
      type: entryType(row),
      size: row.entry_type === "dir" ? "-" : formatBytes(row.size),
      packed: row.compressed == null ? "-" : formatBytes(row.compressed),
      ratio,
      modified: formatModified(row.modified),
      crc: row.crc == null ? "" : row.crc.toString(16).toUpperCase().padStart(8, "0"),
      method: row.encrypted ? "AES" : row.encoding === "utf-8" ? "" : row.encoding.toUpperCase(),
      attr: entryAttributeLabel(row),
      source: row,
    };
  }

  function browseEntries(rowHeight = MODERN_ROW_HEIGHT): DisplayEntry[] {
    if (!currentArchive) return [];
    const window = browseVirtualWindow(rowHeight);
    const rows: DisplayEntry[] = [];
    prefetchAround(window.start);
    prefetchAround(Math.max(window.end - 1, 0));
    for (let index = window.start; index < window.end; index += 1) {
      const row = rowAt(index);
      if (row) rows.push({ ...toDisplayEntry(row), virtualIndex: index });
    }
    return rows;
  }

  function browseVirtualWindow(rowHeight = MODERN_ROW_HEIGHT) {
    const total = currentArchive ? totalRows() : 0;
    if (!currentArchive) return { start: 0, end: total, top: 0, bottom: 0 };
    const viewport = Math.max(browseViewportHeight || 360, rowHeight * 6);
    const visibleRows = Math.ceil(viewport / rowHeight);
    const rawStart = Math.floor(browseScrollTop / rowHeight);
    const start = Math.max(0, rawStart - VIRTUAL_OVERSCAN_ROWS);
    const end = Math.min(total, start + visibleRows + VIRTUAL_OVERSCAN_ROWS * 2);
    return {
      start,
      end,
      top: start * rowHeight,
      bottom: Math.max(0, (total - end) * rowHeight),
    };
  }

  function browsePaddingTop(rowHeight = MODERN_ROW_HEIGHT): number {
    return browseVirtualWindow(rowHeight).top;
  }

  function browsePaddingBottom(rowHeight = MODERN_ROW_HEIGHT): number {
    return browseVirtualWindow(rowHeight).bottom;
  }

  function onBrowseVirtualScroll(event: Event) {
    const target = event.currentTarget as HTMLElement;
    browseScrollTop = target.scrollTop;
    browseViewportHeight = target.clientHeight;
  }

  function archiveTitle(): string {
    return currentArchive?.name ?? noArchiveLabel();
  }

  function archiveFormat(): string {
    return currentArchive?.format.toUpperCase() ?? tr("gui.archive.generic", "Archive");
  }

  function archiveSummary(): string {
    if (!currentArchive) {
      return tr(
        "gui.empty.no_archive_summary",
        "Open an archive to browse entries, inspect metadata, and run archive actions.",
      );
    }
    const diagnostics = currentArchive.garbled_count
      ? `${currentArchive.garbled_count} names need review`
      : currentArchive.legacy_encoding_count
        ? `${currentArchive.legacy_encoding_count} legacy encoded names`
        : "names decoded cleanly";
    return `${currentArchive.entry_count.toLocaleString()} entries · ${archiveFormat()} · ${diagnostics}`;
  }

  function showArchiveReturnBar(value: Screen = screen): boolean {
    return currentArchive !== null && archiveReturnScreens.includes(value);
  }

  function archiveReturnDetail(): string {
    return tr(
      "gui.archive.return_current_detail",
      "{archive} remains open; return to its file list without losing this tool setup.",
    ).replace("{archive}", archiveTitle());
  }

  function returnToCurrentArchive() {
    const title = archiveTitle();
    setScreen("browse");
    showNotice(
      tr("gui.archive.returned_to_current", "Returned to {archive}").replace("{archive}", title),
    );
  }

  function archiveWarningText(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    const count = currentArchive.garbled_count || currentArchive.legacy_encoding_count;
    return `${count} filename${count === 1 ? "" : "s"} need encoding review.`;
  }

  function hasEncodingWarning(): boolean {
    return Boolean(currentArchive && (currentArchive.garbled_count > 0 || currentArchive.legacy_encoding_count > 0));
  }

  function isEntrySelected(entry: DisplayEntry): boolean {
    if (!currentArchive) return false;
    if (entry.source) return selectedPaths().has(entry.source.path);
    return entry.name === "Launch plan.pdf" || entry.name === "screenshots" || entry.name === "财务报表.xlsx";
  }

  function entrySelectionLabel(entry: DisplayEntry): string {
    const name = entry.name || entry.source?.path || tr("gui.selection.entry", "entry");
    const key = isEntrySelected(entry) ? "gui.selection.deselect_entry" : "gui.selection.select_entry";
    const fallback = isEntrySelected(entry) ? "Deselect {name}" : "Select {name}";
    return tr(key, fallback).replace("{name}", name);
  }

  function clearEntryPreviewState() {
    nestedPreview = null;
    entryPreview = null;
    entryPreviewFailure = null;
    previewPhase = "idle";
    previewTargetName = "";
  }

  function selectOnlyEntry(entry: DisplayEntry) {
    if (!entry.source) return;
    clearSelection();
    toggleSelect(entry.source);
    clearEntryPreviewState();
  }

  function toggleEntrySelection(entry: DisplayEntry) {
    if (!entry.source) return;
    toggleSelect(entry.source);
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.selection_toggle", {
      path: entry.source.path,
      selected: selectedPaths().has(entry.source.path),
      selected_count: selectedPaths().size,
    });
  }

  function selectEntry(entry: DisplayEntry, event?: MouseEvent | KeyboardEvent) {
    if (!entry.source) return;
    if (event?.metaKey || event?.ctrlKey) {
      toggleSelect(entry.source);
    } else {
      clearSelection();
      toggleSelect(entry.source);
    }
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.select", {
      path: entry.source.path,
      selected_count: selectedPaths().size,
      multi: Boolean(event?.metaKey || event?.ctrlKey),
    });
  }

  async function activateEntry(entry: DisplayEntry) {
    if (!entry.source) return;
    selectOnlyEntry(entry);
    recordValidationEvent("frontend.entry.activate", {
      path: entry.source.path,
      entry_type: entry.source.entry_type,
      archive_like: archiveLikePath(entry.source.path),
    });
    await submitPreviewEntry(entry.source.path, entry.source.entry_type);
  }

  function canGoUpArchive(): boolean {
    return Boolean(currentArchive && archiveDirs.length > 0);
  }

  async function goArchiveUp() {
    if (!canGoUpArchive()) return;
    await goUp();
    browseScrollTop = 0;
    clearSelection();
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.go_up", {
      path: archiveDirs.join("/"),
    });
    showNotice(tr("gui.nav.opened_parent_folder", "Opened parent folder"));
  }

  async function openArchiveBreadcrumb(level: number) {
    if (!currentArchive) return;
    await gotoBreadcrumb(level);
    browseScrollTop = 0;
    clearSelection();
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.breadcrumb", {
      level,
      path: archiveDirs.slice(0, level + 1).join("/"),
    });
  }

  function showEntryContextAt(x: number, y: number, entry: DisplayEntry) {
    if (entry.source && !selectedPaths().has(entry.source.path)) {
      toggleSelect(entry.source);
    }
    closeQuickActions(false);
    const viewportPadding = 12;
    const menuWidth = 236;
    const menuHeight = 264;
    entryContext = {
      x: Math.max(viewportPadding, Math.min(x, window.innerWidth - menuWidth - viewportPadding)),
      y: Math.max(viewportPadding, Math.min(y, window.innerHeight - menuHeight - viewportPadding)),
      name: entry.name,
      path: entry.source?.path ?? null,
      canRename: Boolean(entry.source && entry.source.entry_type !== "dir"),
      isDir: entry.source?.entry_type === "dir",
    };
  }

  function openEntryContext(event: MouseEvent, entry: DisplayEntry) {
    event.preventDefault();
    showEntryContextAt(event.clientX, event.clientY, entry);
  }

  function closeEntryContext() {
    entryContext = null;
  }

  async function runEntryContextAction(action: "extract" | "delete" | "rename" | "move" | "preview" | "test") {
    const contextPath = entryContext?.path ?? null;
    const contextIsDir = entryContext?.isDir === true;
    closeEntryContext();
    if (action === "extract") {
      await submitExtractJob();
    } else if (action === "delete") {
      await submitDeleteSelectedJob();
    } else if (action === "rename") {
      await submitRenameSelectedJob();
    } else if (action === "move") {
      await submitMoveSelectedJob();
    } else if (action === "preview") {
      await submitPreviewEntry(contextPath, contextIsDir ? "dir" : undefined);
    } else {
      await submitTestJob();
    }
  }

  function onEntryKeydown(event: KeyboardEvent, entry: DisplayEntry) {
    if (event.key === "Enter") {
      event.preventDefault();
      void activateEntry(entry);
    } else if (event.key === " ") {
      event.preventDefault();
      selectEntry(entry, event);
    } else if (event.key === "Backspace" && selectedPaths().size === 0) {
      event.preventDefault();
      void goArchiveUp();
    } else if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      void submitDeleteSelectedJob();
    } else if (event.metaKey && event.key === "ArrowUp") {
      event.preventDefault();
      void goArchiveUp();
    } else if (event.key === "e" || event.key === "E") {
      event.preventDefault();
      void submitExtractJob();
    } else if (event.key === "m" || event.key === "M") {
      event.preventDefault();
      void submitMoveSelectedJob();
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const target = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
      const rect = target?.getBoundingClientRect();
      showEntryContextAt(rect ? rect.left + 24 : window.innerWidth / 2, rect ? rect.bottom - 2 : window.innerHeight / 2, entry);
    }
  }

  function selectedSummary(): string {
    if (!currentArchive) return tr("gui.selection.none", "0 selected");
    const count = selectedPaths().size;
    return tr("gui.selection.selected_size", "{count} selected · {size}")
      .replace("{count}", count.toLocaleString())
      .replace("{size}", formatBytes(selectedSize()));
  }

  function archiveEntryCountLabel(count: number): string {
    return tr("gui.archive.entry_count", "{count} entries")
      .replace("{count}", count.toLocaleString());
  }

  function archiveVolumeCountLabel(count: number): string {
    return tr("gui.archive.volume_count", "{count} volumes")
      .replace("{count}", count.toLocaleString());
  }

  function archivePathWithoutSplitSuffix(name: string): string {
    return name.replace(/\.\d{3,}$/i, "");
  }

  function archiveExtensionMatch(name: string): string | null {
    const lower = archivePathWithoutSplitSuffix(name).toLowerCase().trimEnd();
    return registryFormatExtensions().find((extension) => lower.endsWith(`.${extension}`)) ?? null;
  }

  function archiveStemName(name: string = currentArchive?.name ?? "archive"): string {
    const unsplit = archivePathWithoutSplitSuffix(name);
    const extension = archiveExtensionMatch(unsplit);
    if (extension) return unsplit.slice(0, -(extension.length + 1));
    const dot = unsplit.lastIndexOf(".");
    return dot > 0 ? unsplit.slice(0, dot) : unsplit;
  }

  function archiveFormatFromPath(path: string): string {
    const name = pathBaseName(path).toLowerCase();
    const extension = archiveExtensionMatch(name);
    if (extension) return formatDisplayName(extension);
    const dot = name.lastIndexOf(".");
    return dot > 0 ? name.slice(dot + 1).toUpperCase() : "ARCHIVE";
  }

  function batchReviewArchives(): BatchArchiveRow[] {
    if (batchArchivePaths.length === 0) {
      if (currentArchive) {
        return [{
          name: currentArchive.name,
          format: archiveFormat(),
          entries: currentArchive.entry_count.toLocaleString(),
          target: effectiveExtractDest(),
          state: "Ready",
        }];
      }
      return [];
    }
    return batchArchivePaths.map((path) => {
      const isCurrent = currentArchive?.path === path;
      return {
        name: pathBaseName(path),
        format: isCurrent ? archiveFormat() : archiveFormatFromPath(path),
        entries: isCurrent ? currentArchive.entry_count.toLocaleString() : "Pending",
        target: extractDestForPath(path),
        state: isCurrent ? "Ready" : "Ready to start",
      };
    });
  }

  function batchReviewWarningCount(): number {
    return batchReviewArchives().filter((item) => item.state.toLowerCase().includes("password")).length;
  }

  function batchReadyCount(): number {
    return batchReviewArchives().length - batchReviewWarningCount();
  }

  function batchReadyPercent(): number {
    const total = batchReviewArchives().length;
    return total === 0 ? 0 : Math.round((batchReadyCount() / total) * 100);
  }

  function batchWarningLabel(): string {
    if (batchReviewArchives().length === 0) return openArchiveFirstLabel();
    const count = batchReviewWarningCount();
    return tr("gui.batch.passwords_required_count", "{count} passwords required").replace("{count}", count.toLocaleString());
  }

  function droppedSourceLabel(): string {
    if (createDropInputs.length === 0) return tr("gui.create.no_dropped_sources", "No dropped sources");
    const first = pathBaseName(createDropInputs[0]);
    return createDropInputs.length === 1
      ? first
      : tr("gui.create.dropped_more", "{first} + {count} more")
        .replace("{first}", first)
        .replace("{count}", String(createDropInputs.length - 1));
  }

  function dropStatusLabel(): string {
    if (dragActive) return tr("gui.drop.active", "Drop archives to open, or files and folders to create an archive");
    if (lastDropKind === "archives") {
      return tr("gui.drop.archives_ready", "{count} dropped archives ready").replace("{count}", String(batchArchivePaths.length));
    }
    if (lastDropKind === "create") {
      return tr("gui.drop.create_ready", "{count} dropped items ready to archive").replace("{count}", String(createDropInputs.length));
    }
    return "";
  }

  function uniqueNonEmptyPaths(paths: string[]): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const path of paths.map((item) => item.trim()).filter(Boolean)) {
      if (seen.has(path)) continue;
      seen.add(path);
      out.push(path);
    }
    return out;
  }

  function pathsFromDomDrop(event: DragEvent): string[] {
    const transfer = event.dataTransfer;
    if (!transfer) return [];
    const uriList = transfer.getData("text/uri-list");
    const textList = transfer.getData("text/plain");
    const fromText = (uriList || textList)
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line && !line.startsWith("#"))
      .map((line) => {
        if (!line.startsWith("file://")) return line;
        try {
          return decodeURIComponent(new URL(line).pathname);
        } catch {
          return line.replace(/^file:\/\//, "");
        }
      });
    const fromFiles = Array.from(transfer.files)
      .map((file) => {
        const maybePath = (file as File & { path?: string }).path;
        return maybePath || file.name;
      })
      .filter(Boolean);
    return uniqueNonEmptyPaths([...fromText, ...fromFiles]);
  }

  function recordValidationEvent(event: string, payload: Record<string, unknown>) {
    void ipc.recordValidationEvent(event, payload).catch(() => {
      // Dev preview and normal sessions may not have the validation command.
    });
  }

  function recordNativeDialogRequest(kind: string, options: NativeDialogOptions) {
    const snapshot = options as {
      title?: string;
      multiple?: boolean;
      directory?: boolean;
      defaultPath?: string;
      filters?: Array<{ name: string; extensions?: string[] }>;
    };
    recordValidationEvent("frontend.dialog.request", {
      kind,
      lang: currentLang(),
      platform: platformKind(),
      title: snapshot.title ?? null,
      multiple: snapshot.multiple === true,
      directory: snapshot.directory === true,
      has_default_path: typeof snapshot.defaultPath === "string" && snapshot.defaultPath.length > 0,
      default_name: typeof snapshot.defaultPath === "string" ? pathBaseName(snapshot.defaultPath) : null,
      filters: (snapshot.filters ?? []).map((filter) => ({
        name: filter.name,
        extensions: filter.extensions ?? [],
      })),
    });
  }

  async function openNativeDialog(kind: string, open: DialogModule["open"], options: OpenDialogOptions) {
    recordNativeDialogRequest(kind, options);
    return open(options);
  }

  async function saveNativeDialog(kind: string, save: DialogModule["save"], options: SaveDialogOptions) {
    recordNativeDialogRequest(kind, options);
    return save(options);
  }

  function recordValidationRenderReady(reason: string) {
    let emitted = false;
    const emit = () => {
      if (emitted) return;
      emitted = true;
      const text = document.body?.innerText.replace(/\s+/g, " ").trim().slice(0, 320) ?? "";
      recordValidationEvent("frontend.render.ready", {
        reason,
        screen,
        ui_mode: activeUiMode(),
        archive: currentArchive?.name ?? null,
        entry_count: currentArchive?.entry_count ?? null,
        viewport_width: document.documentElement.clientWidth,
        viewport_height: document.documentElement.clientHeight,
        text_sample: text,
      });
    };
    void tick().then(() => setTimeout(emit, 0));
    requestAnimationFrame(() => requestAnimationFrame(emit));
  }

  async function handleDroppedPaths(paths: string[], source: "native" | "dom" | "preview" | "validation") {
    const dropped = uniqueNonEmptyPaths(paths);
    if (dropped.length === 0) return;
    const archivePaths = dropped.filter(archiveLikePath);
    const createInputs = dropped.filter((path) => !archiveLikePath(path));
    if (archivePaths.length > 0 && createInputs.length === 0) {
      recordValidationEvent("frontend.drop", {
        source,
        route: archivePaths.length === 1 ? "open-archive" : "batch",
        paths: archivePaths,
        archive_count: archivePaths.length,
        create_count: 0,
      });
      lastDropKind = "archives";
      batchArchivePaths = archivePaths;
      if (archivePaths.length === 1) {
        await openArchivePath(archivePaths[0], "open-file");
      } else {
        setScreen("batch");
      }
      return;
    }

    lastDropKind = "create";
    createDropInputs = dropped;
    recordValidationEvent("frontend.drop", {
      source,
      route: "create",
      paths: dropped,
      archive_count: archivePaths.length,
      create_count: createInputs.length,
    });
    setScreen("create");
    if (source === "native") {
      await submitCreateInputs(dropped, "drop");
    }
  }

  function extractDestInDefaultFolder(fallbackParent: string, archiveName: string): string {
    const parent = normalizedDefaultExtractDir() ?? fallbackParent;
    const name = archiveStemName(archiveName);
    if (parent === "/") return `/${name}`;
    return `${parent}/${name}`;
  }

  function defaultExtractDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return extractDestInDefaultFolder(pathDir(currentArchive.path), currentArchive.name);
  }

  function sameFolderExtractDest(): string {
    return currentArchive ? pathDir(currentArchive.path) : openArchiveFirstLabel();
  }

  function chosenExtractDest(): string {
    return extractCustomDest.trim() || tr("gui.extract.pick_another_folder", "Pick another folder");
  }

  function effectiveExtractDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (extractDestinationMode === "same") return sameFolderExtractDest();
    if (extractDestinationMode === "choose") return extractCustomDest.trim() || defaultExtractDest();
    return defaultExtractDest();
  }

  function extractDestinationTitle(mode: ExtractDestinationMode): string {
    if (mode === "same") return tr("gui.extract.same_folder", "Same folder");
    if (mode === "choose") return tr("gui.extract.choose", "Choose");
    return tr("gui.extract.smart_folder", "Smart folder");
  }

  function extractDestinationDetail(mode: ExtractDestinationMode): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (mode === "same") return sameFolderExtractDest();
    if (mode === "choose") return chosenExtractDest();
    return defaultExtractDest();
  }

  async function selectExtractDestination(mode: ExtractDestinationMode) {
    if (!currentArchive) {
      showNotice(openArchiveFirstLabel());
      return;
    }
    if (mode === "choose" && !extractCustomDest.trim()) {
      await chooseExtractDestination();
      return;
    }
    extractDestinationMode = mode;
  }

  async function chooseExtractDestination() {
    if (!currentArchive) {
      showNotice(openArchiveFirstLabel());
      return;
    }
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("extract.destination", open, {
        title: tr("gui.extract.choose_destination_title", "Choose extract destination"),
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        extractCustomDest = selected;
        extractDestinationMode = "choose";
        showNotice(tr("gui.extract.destination_selected", "Extract destination selected"));
      }
    } catch {
      showNotice(tr("gui.extract.destination_picker_requires_desktop_service", "Destination picker requires the desktop service"));
    }
  }

  function extractOverwriteLabel(mode: ExtractOverwriteMode = extractOverwriteMode): string {
    if (mode === "skip") return tr("gui.extract.overwrite.skip", "Skip");
    if (mode === "overwrite") return tr("gui.extract.overwrite.overwrite", "Overwrite");
    if (mode === "rename") return tr("gui.extract.overwrite.rename", "Keep both (auto-rename)");
    return tr("gui.extract.overwrite.ask", "Ask");
  }

  function selectExtractOverwrite(mode: ExtractOverwriteMode) {
    if (!currentArchive) {
      showNotice(openArchiveFirstLabel());
      return;
    }
    extractOverwriteMode = mode;
  }

  function extractDestForPath(path: string): string {
    return extractDestInDefaultFolder(pathDir(path), pathBaseName(path));
  }

  function nestedExtractDest(preview: NestedArchivePreviewDto): string {
    return extractDestInDefaultFolder(pathDir(preview.outer_path), pathBaseName(preview.entry_path));
  }

  function defaultConvertDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    const ext = currentArchive.format.toLowerCase() === "zip" ? ".7z" : ".zip";
    return `${pathDir(currentArchive.path)}/${archiveStemName(currentArchive.name)}${ext}`;
  }

  function defaultConvertTargetFormat(): string {
    if (!currentArchive) return "-";
    return currentArchive.format.toLowerCase() === "zip" ? "7Z" : "ZIP";
  }

  function defaultSqzExportDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return `${pathDir(currentArchive.path)}/${archiveStemName(currentArchive.name)}.zip`;
  }

  function defaultSqzRepairDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return `${pathDir(currentArchive.path)}/${archiveStemName(currentArchive.name)}.repaired.sqz`;
  }

  function defaultZipRepairDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return `${pathDir(currentArchive.path)}/${archiveStemName(currentArchive.name)}.rebuilt.zip`;
  }

  function defaultRecoveryPath(): string {
    return currentArchive ? `${currentArchive.path}.par2` : openArchiveFirstLabel();
  }

  function isCurrentArchiveZipFamily(): boolean {
    const format = currentArchive?.format.toLowerCase();
    return !!format && ["zip", "jar", "apk", "cbz", "ipa"].includes(format);
  }

  function isCurrentArchiveSqz(): boolean {
    return currentArchive?.format.toLowerCase() === "sqz";
  }

  function labelWithDisabledReason(label: string, reason: string): string {
    return reason ? `${label} · ${reason}` : label;
  }

  function recoveryZipDisabledReason(): string {
    if (!currentArchive) {
      return tr("gui.recovery.open_zip_before_rebuild", "Open a ZIP archive before rebuilding its index");
    }
    return isCurrentArchiveZipFamily()
      ? ""
      : tr("gui.recovery.zip_rebuild_zip_family_only", "ZIP index rebuild is available for ZIP-family archives");
  }

  function recoverySqzExportDisabledReason(): string {
    return isCurrentArchiveSqz()
      ? ""
      : tr("gui.recovery.open_sqz_before_export", "Open an SQZ archive before exporting");
  }

  function recoverySqzRepairDisabledReason(): string {
    return isCurrentArchiveSqz()
      ? ""
      : tr("gui.recovery.open_sqz_before_repair", "Open an SQZ archive before repairing");
  }

  function recoveryProtectDisabledReason(): string {
    return currentArchive
      ? ""
      : tr("gui.recovery.open_before_par2_protect", "Open an archive before creating PAR2 recovery data");
  }

  function recoveryVerifyDisabledReason(): string {
    return currentArchive
      ? ""
      : tr("gui.recovery.open_before_verify", "Open an archive before verifying recovery data");
  }

  function recoveryRepairPar2DisabledReason(): string {
    return currentArchive
      ? ""
      : tr("gui.recovery.open_before_par2_repair", "Open an archive before repairing with PAR2 recovery data");
  }

  function recoveryFailureDisabledReason(): string {
    return recoveryFailureAvailable() ? "" : tr("gui.recovery.run_verify_first", "Run Verify first");
  }

  function archiveEncodingForJob(): string | null {
    return currentArchive?.encoding_override ?? null;
  }

  function selectedJobPaths(): string[] | null {
    const selected = [...selectedPaths()];
    return selected.length > 0 ? selected : null;
  }

  function hasArchiveOpen(): boolean {
    return currentArchive !== null;
  }

  function hasArchiveSelection(): boolean {
    return hasArchiveOpen() && selectedPaths().size > 0;
  }

  function canRenameSelection(): boolean {
    return hasArchiveOpen() && selectedRenameSource() !== null;
  }

  function entryExtension(entryPath: string): string {
    const name = pathBaseName(entryPath);
    const index = name.lastIndexOf(".");
    return index > 0 ? name.slice(index + 1).toLowerCase() : "";
  }

  function previewInlineImageMime(entryPath: string): string | null {
    const ext = entryExtension(entryPath);
    if (["jpg", "jpeg", "png", "gif", "webp", "bmp"].includes(ext)) return `image/${ext === "jpg" ? "jpeg" : ext}`;
    return null;
  }

  function previewEntryForPath(entryPath: string): EntryDto | null {
    return loadedRows().find((row) => row.path === entryPath) ?? null;
  }

  function previewEntrySize(entryPath: string): number | null {
    return previewEntryForPath(entryPath)?.size ?? null;
  }

  function previewSystemCode(entryPath: string, size: number | null): PreviewPolicyCode {
    const mime = previewInlineImageMime(entryPath);
    if (mime && size !== null && size > INLINE_IMAGE_PREVIEW_MAX_BYTES) {
      return "system_large_image";
    }
    const ext = entryExtension(entryPath);
    return ext ? "system_type" : "system_unknown";
  }

  function previewPolicyFor(entryPath: string | null, entryType: EntryDto["entry_type"] | null = null): PreviewPolicy {
    if (!currentArchive) {
      return {
        kind: "none",
        label: actionLabel("Preview selected"),
        code: "no_archive",
        disabledReason: tr("gui.preview.open_archive_first", "Open an archive before previewing entries"),
      };
    }
    if (!entryPath) {
      return {
        kind: "none",
        label: actionLabel("Preview selected"),
        code: "select_one",
        disabledReason: tr("gui.preview.select_one", "Select one entry to preview or open"),
      };
    }

    const resolvedType = entryType ?? entryTypeForPath(entryPath);
    if (resolvedType === "dir" || entryPath.endsWith("/")) {
      return {
        kind: "folder",
        label: tr("gui.preview.open_folder", "Open"),
        code: "folder",
        disabledReason: "",
      };
    }
    if (archiveLikePath(entryPath)) {
      return {
        kind: "nested",
        label: actionLabel("Preview"),
        code: "nested",
        disabledReason: "",
      };
    }

    const size = previewEntrySize(entryPath);
    if (previewInlineImageMime(entryPath) && (size === null || size <= INLINE_IMAGE_PREVIEW_MAX_BYTES)) {
      return {
        kind: "inline-image",
        label: tr("gui.preview.action_inline_image", "Preview"),
        code: "inline_image",
        disabledReason: "",
      };
    }

    return {
      kind: "system-file",
      label: tr("gui.preview.action_system_file", "Preview"),
      code: previewSystemCode(entryPath, size),
      disabledReason: "",
    };
  }

  function selectedPreviewPolicy(): PreviewPolicy {
    return previewPolicyFor(selectedPreviewPath());
  }

  function previewPolicyForFailure(): PreviewPolicy | null {
    return entryPreviewFailure
      ? previewPolicyFor(entryPreviewFailure.entryPath, entryPreviewFailure.entryType)
      : null;
  }

  function canPreviewEntrySelection(): boolean {
    return selectedPreviewPolicy().kind !== "none" && !previewBusy();
  }

  function renameSelectedDisabledReason(): string {
    return canRenameSelection()
      ? ""
      : tr("gui.precondition.select_one_before_rename", "Select exactly one file entry before renaming");
  }

  function deleteSelectedDisabledReason(): string {
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_delete", "Select entries before deleting");
  }

  function moveSelectedDisabledReason(): string {
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_move", "Select entries before moving");
  }

  function copyOutSelectedDisabledReason(): string {
    if (!currentArchive) return tr("gui.precondition.open_before_copy_out", "Open an archive before copying entries out");
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_copy_out", "Select entries before copying out");
  }

  function previewSelectedDisabledReason(): string {
    if (previewBusy()) return tr("gui.preview.loading", "Loading preview");
    return selectedPreviewPolicy().disabledReason;
  }

  function previewActionLabel(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
  ): string {
    if (previewBusy()) return tr("gui.preview.loading", "Loading preview");
    return previewPolicyFor(entryPath, entryType).label;
  }

  function archiveActionTitle(enabled: boolean): string {
    return enabled ? "" : openArchiveFirstLabel();
  }

  function createExcludeRules(): string[] {
    return parseDelimitedRules(createExcludeText);
  }

  function createExcludeSummary(): string {
    const rules = createExcludeRules();
    return rules.length > 0 ? rules.join("; ") : tr("gui.create.no_exclude_rules", "No exclude rules");
  }

  function createExcludeCountLabel(): string {
    const count = createExcludeRules().length;
    return tr("gui.create.rule_count", "{count} rules").replace("{count}", count.toLocaleString());
  }

  function createPreflightBusy(): boolean {
    return ["selecting", "measuring", "checkingTemp", "choosingDest", "checkingDest", "submitting"].includes(createPreflightPhase);
  }

  function createPreflightPhaseLabel(): string {
    switch (createPreflightPhase) {
      case "selecting":
        return tr("gui.create.waiting_source_picker", "Waiting for source picker");
      case "measuring":
        return tr("gui.create.measuring_source_bytes", "Measuring source bytes and exclude rules");
      case "checkingTemp":
        return tr("gui.create.checking_temp_workspace", "Checking temporary workspace");
      case "choosingDest":
        return tr("gui.create.waiting_destination", "Waiting for destination");
      case "checkingDest":
        return tr("gui.create.checking_destination_disk_short", "Checking destination disk");
      case "submitting":
        return tr("gui.create.submitting_archive_job", "Submitting archive job");
      case "ready":
        return tr("gui.create.preflight_ready", "Preflight ready");
      case "blocked":
        return tr("gui.create.preflight_blocked", "Preflight blocked");
      case "idle":
        return tr("gui.create.preflight_pending", "Preflight pending");
    }
  }

  function createPreflightPercent(): number {
    switch (createPreflightPhase) {
      case "selecting":
        return 10;
      case "measuring":
        return 28;
      case "checkingTemp":
        return 50;
      case "choosingDest":
        return 64;
      case "checkingDest":
        return 80;
      case "submitting":
        return 92;
      case "ready":
      case "blocked":
        return 100;
      case "idle":
        return 0;
    }
  }

  function createEstimateTitle(): string {
    if (createPreflightPhase === "measuring" && createPreflightScanned > 0) {
      return tr("gui.create.scanned_count", "{count} scanned").replace("{count}", createPreflightScanned.toLocaleString());
    }
    return lastCreateEstimate ? formatBytes(lastCreateEstimate.total_bytes) : tr("gui.create.choose_sources", "Choose sources");
  }

  function createEstimateSubtitle(): string {
    if (createPreflightPhase === "measuring" && createPreflightScanned > 0) {
      return createPreflightCurrent ? pathBaseName(createPreflightCurrent) : tr("gui.create.walking_inputs", "walking inputs");
    }
    if (!lastCreateEstimate) return tr("gui.create.input_size_pending", "input size pending");
    const files = lastCreateEstimate.files.toLocaleString();
    const dirs = lastCreateEstimate.directories.toLocaleString();
    return tr("gui.create.files_folders_count", "{files} files, {folders} folders")
      .replace("{files}", files)
      .replace("{folders}", dirs);
  }

  function createEstimateBody(): string {
    if (createPreflightPhase === "measuring" && createPreflightScanned > 0) {
      return tr("gui.create.scanning_after_excludes", "Scanning local inputs after exclude rules · {count} entries found so far.")
        .replace("{count}", createPreflightScanned.toLocaleString());
    }
    if (!lastCreateEstimate) {
      return tr("gui.create.measure_real_bytes_body", "Squallz will measure real input bytes after the source picker; it will not guess compressed output size.");
    }
    const entries = lastCreateEstimate.entries.toLocaleString();
    const inputs = lastCreateEstimate.input_count.toLocaleString();
    const dest = lastCreateDest
      ? tr("gui.create.destination_sentence_suffix", " Destination: {name}.").replace("{name}", pathBaseName(lastCreateDest))
      : "";
    return tr("gui.create.entries_from_sources_after_excludes", "{entries} entries from {inputs} sources after excludes.")
      .replace("{entries}", entries)
      .replace("{inputs}", inputs) + dest;
  }

  function createEstimateMeterWidth(): number {
    if (!lastCreateEstimate) return 0;
    const gib = lastCreateEstimate.total_bytes / bytesPerGiB;
    return Math.max(8, Math.min(100, Math.round(gib * 24)));
  }

  function createVolumePreview(): string {
    if (!lastCreateEstimate) return tr("gui.create.final_volume_count_after_write", "Final volume count appears after the archive is written.");
    if (activeCreateFormat === "tar.zst" || activeCreateFormat === "wim") return createSplitCapability();
    const inputSizedVolumes = Math.max(1, Math.ceil(lastCreateEstimate.total_bytes / (2 * bytesPerGiB)));
    return tr("gui.create.volume_output_budget_guide", "{split} · output-budget guide {count} volumes; final count depends on compression.")
      .replace("{split}", createSplitCapability())
      .replace("{count}", String(inputSizedVolumes));
  }

  function createEstimateStatusbar(): string {
    if (createPreflightPhase === "selecting") return tr("gui.create.waiting_source_picker", "Waiting for source picker");
    if (createPreflightPhase === "measuring") {
      return createPreflightScanned > 0
        ? tr("gui.create.scanning_inputs_count", "Scanning inputs · {count} entries").replace("{count}", createPreflightScanned.toLocaleString())
        : tr("gui.create.measuring_input_bytes", "Measuring input bytes...");
    }
    if (createPreflightPhase === "blocked" && lastCreateEstimate?.entries === 0) return tr("gui.create.no_entries_after_excludes", "No entries after excludes");
    if (!lastCreateEstimate) return tr("gui.create.input_estimate_pending", "Input estimate pending source selection");
    return tr("gui.create.estimate_status", "{size} input · {entries} entries · {excludes}")
      .replace("{size}", formatBytes(lastCreateEstimate.total_bytes))
      .replace("{entries}", lastCreateEstimate.entries.toLocaleString())
      .replace("{excludes}", createExcludeCountLabel());
  }

  function requiredCreateDiskBytes(estimate: CreateEstimateDto): number {
    return estimate.output_budget_bytes;
  }

  function diskPreflightTitle(): string {
    if (!lastDiskSpace) return tr("gui.create.destination_pending", "Destination pending");
    return lastDiskSpace.ok ? tr("gui.create.space_available", "Space available") : tr("gui.create.not_enough_space", "Not enough space");
  }

  function diskPreflightBody(): string {
    if (!lastDiskSpace) {
      return tr("gui.create.disk_preflight_body_pending", "After choosing a destination, Squallz checks the target volume before the task starts.");
    }
    return tr("gui.create.disk_preflight_body", "{available} available in {path}; {required} reserved as a conservative output budget.")
      .replace("{available}", formatBytes(lastDiskSpace.available_bytes))
      .replace("{path}", lastDiskSpace.path)
      .replace("{required}", formatBytes(lastDiskSpace.required_bytes));
  }

  function diskPreflightStatusbar(): string {
    if (createPreflightPhase === "choosingDest") return tr("gui.create.waiting_destination_picker", "Waiting for destination picker");
    if (createPreflightPhase === "checkingDest") return tr("gui.create.checking_destination_disk", "Checking destination disk...");
    if (!lastDiskSpace) return tr("gui.create.destination_disk_pending", "Destination disk preflight pending");
    return tr("gui.create.disk_status_available", "{status} · {available} available")
      .replace("{status}", lastDiskSpace.ok ? tr("gui.create.disk_ok", "Disk OK") : tr("gui.create.disk_blocked", "Disk blocked"))
      .replace("{available}", formatBytes(lastDiskSpace.available_bytes));
  }

  function tempPreflightTitle(): string {
    if (!lastTempDiskSpace) return tr("gui.create.temp_pending", "Temp pending");
    return lastTempDiskSpace.ok ? tr("gui.create.temp_space_available", "Temp space available") : tr("gui.create.temp_space_blocked", "Temp space blocked");
  }

  function tempPreflightBody(): string {
    if (!lastTempDiskSpace) {
      return tr("gui.create.temp_preflight_body_pending", "Squallz also checks the system temporary folder before queuing create jobs.");
    }
    return tr("gui.create.temp_preflight_body", "{available} available in {path}; {required} reserved for temporary rewrite headroom.")
      .replace("{available}", formatBytes(lastTempDiskSpace.available_bytes))
      .replace("{path}", lastTempDiskSpace.path)
      .replace("{required}", formatBytes(lastTempDiskSpace.required_bytes));
  }

  function tempPreflightStatusbar(): string {
    if (createPreflightPhase === "checkingTemp") return tr("gui.create.checking_temporary_space", "Checking temporary space...");
    if (!lastTempDiskSpace) return tr("gui.create.temp_preflight_pending", "Temp preflight pending");
    return tr("gui.create.temp_status_available", "{status} · {available} available")
      .replace("{status}", lastTempDiskSpace.ok ? tr("gui.create.temp_ok", "Temp OK") : tr("gui.create.temp_blocked", "Temp blocked"))
      .replace("{available}", formatBytes(lastTempDiskSpace.available_bytes));
  }

  function selectedDeletePatterns(): string[] {
    return [...selectedPaths()].map((path) => path.endsWith("/") ? path.slice(0, -1) : path);
  }

  function renameTargetForPath(path: string): string {
    const clean = path.endsWith("/") ? path.slice(0, -1) : path;
    const slash = clean.lastIndexOf("/");
    const dir = slash >= 0 ? `${clean.slice(0, slash + 1)}` : "";
    const base = slash >= 0 ? clean.slice(slash + 1) : clean;
    const dot = base.lastIndexOf(".");
    if (dot > 0) return `${dir}${base.slice(0, dot)}-renamed${base.slice(dot)}`;
    return `${dir}${base}-renamed`;
  }

  function selectedRenameSource(): string | null {
    const selected = [...selectedPaths()].filter((path) => !path.endsWith("/"));
    return selected.length === 1 ? selected[0] : null;
  }

  function normalizeArchiveFilePath(value: string, fallback: string): string {
    const parts = value
      .replaceAll("\\", "/")
      .split("/")
      .map((part) => part.trim())
      .filter((part) => part.length > 0 && part !== "." && part !== "..");
    return parts.length === 0 ? fallback : parts.join("/");
  }

  function archiveEntryExtension(path: string): string {
    const base = pathBaseName(path.endsWith("/") ? path.slice(0, -1) : path);
    const dot = base.lastIndexOf(".");
    if (dot <= 0 || dot === base.length - 1) return "";
    return base.slice(dot);
  }

  function windowsUnsafeArchiveSegment(path: string): string | null {
    const segments = path
      .replaceAll("\\", "/")
      .split("/")
      .map((part) => part.trim())
      .filter(Boolean);
    for (const segment of segments) {
      if (/[<>:"|?*\u0000-\u001F]/u.test(segment)) {
        return `"${segment}" contains Windows-invalid characters`;
      }
      if (/[. ]$/u.test(segment)) {
        return `"${segment}" ends with a space or dot`;
      }
      const windowsName = segment.replace(/[. ]+$/u, "");
      const stem = windowsName.split(".")[0]?.toUpperCase() ?? "";
      if (windowsReservedBaseNames.has(stem)) {
        return `"${segment}" is reserved on Windows`;
      }
    }
    return null;
  }

  function renameTargetIssue(source: string, target: string): RenameTargetIssue {
    const unsafeSegment = windowsUnsafeArchiveSegment(target);
    if (unsafeSegment) {
      return { blocking: unsafeSegment, warning: null };
    }
    const sourceExt = archiveEntryExtension(source);
    const targetExt = archiveEntryExtension(target);
    if (sourceExt.toLowerCase() !== targetExt.toLowerCase()) {
      const from = sourceExt || "no extension";
      const to = targetExt || "no extension";
      return { blocking: null, warning: `Extension changes ${from} -> ${to}` };
    }
    return { blocking: null, warning: null };
  }

  function normalizeRenameTargetName(value = renameTargetName, source = selectedRenameSource()): string {
    const fallback = source ? renameTargetForPath(source) : "renamed.txt";
    const trimmed = value.trim();
    if (!source || trimmed.includes("/") || trimmed.includes("\\")) {
      return normalizeArchiveFilePath(trimmed, fallback);
    }
    const cleanSource = source.endsWith("/") ? source.slice(0, -1) : source;
    const slash = cleanSource.lastIndexOf("/");
    const dir = slash >= 0 ? `${cleanSource.slice(0, slash + 1)}` : "";
    return `${dir}${normalizeArchiveFilePath(trimmed, pathBaseName(fallback))}`;
  }

  function commitRenameTargetName(value = renameTargetName) {
    renameTargetName = normalizeRenameTargetName(value);
  }

  function renameTargetStatus(): string {
    const selected = [...selectedPaths()].filter((path) => !path.endsWith("/"));
    const target = normalizeRenameTargetName();
    if (!currentArchive) return openArchiveFirstLabel();
    if (selected.length !== 1) return tr("gui.rename.select_one_file", "Select exactly one file to rename");
    const from = selected[0];
    if (target === from) return tr("gui.rename.target_must_differ", "Rename target must differ from source");
    if (archivePathSet().has(target)) return tr("gui.new_folder.already_exists", "Already exists: {folder}").replace("{folder}", target);
    const issue = renameTargetIssue(from, target);
    if (issue.blocking) return tr("gui.rename.blocked_reason", "Blocked: {reason}").replace("{reason}", issue.blocking);
    if (issue.warning) return `${issue.warning} · ${from} -> ${target}`;
    return `${from} -> ${target}${archiveConflictCoverageNote()}`;
  }

  function normalizeMoveTargetDir(value = moveTargetDir): string {
    const parts = value
      .replaceAll("\\", "/")
      .split("/")
      .map((part) => part.trim())
      .filter((part) => part.length > 0 && part !== "." && part !== "..");
    if (parts.length === 0) return "moved/";
    return `${parts.join("/")}/`;
  }

  function commitMoveTargetDir(value = moveTargetDir) {
    moveTargetDir = normalizeMoveTargetDir(value);
    moveConflictReview = null;
  }

  function moveTargetForPath(path: string, targetDir = normalizeMoveTargetDir()): string {
    const isDir = path.endsWith("/");
    const clean = isDir ? path.slice(0, -1) : path;
    const base = pathBaseName(clean);
    return `${targetDir}${base}${isDir ? "/" : ""}`;
  }

  function uniqueArchiveTarget(path: string, reserved: Set<string>): string {
    const isDir = path.endsWith("/");
    const clean = isDir ? path.slice(0, -1) : path;
    const slash = clean.lastIndexOf("/");
    const dir = slash >= 0 ? `${clean.slice(0, slash + 1)}` : "";
    const base = slash >= 0 ? clean.slice(slash + 1) : clean;
    const dot = !isDir ? base.lastIndexOf(".") : -1;
    const stem = dot > 0 ? base.slice(0, dot) : base;
    const ext = dot > 0 ? base.slice(dot) : "";
    for (let copy = 1; copy < 1000; copy += 1) {
      const suffix = copy === 1 ? " copy" : ` copy ${copy}`;
      const candidate = `${dir}${stem}${suffix}${ext}${isDir ? "/" : ""}`;
      if (!reserved.has(candidate)) {
        reserved.add(candidate);
        return candidate;
      }
    }
    return `${dir}${stem} copy ${Date.now()}${ext}${isDir ? "/" : ""}`;
  }

  function buildMovePlan(targetDir = normalizeMoveTargetDir()): MovePlanItem[] {
    const selected = [...selectedPaths()];
    const existing = archivePathSet();
    const targetCounts = new Map<string, number>();
    for (const from of selected) {
      const to = moveTargetForPath(from, targetDir);
      targetCounts.set(to, (targetCounts.get(to) ?? 0) + 1);
    }
    const reserved = new Set(existing);
    for (const from of selected) {
      reserved.add(moveTargetForPath(from, targetDir));
    }
    return selected.map((from) => {
      const to = moveTargetForPath(from, targetDir);
      const duplicate = (targetCounts.get(to) ?? 0) > 1;
      const exists = existing.has(to);
      const conflict = exists || duplicate;
      const reason = exists
        ? tr("gui.move.target_already_exists", "Target already exists")
        : duplicate
          ? tr("gui.move.duplicate_target_name", "Multiple selected entries share this target name")
          : null;
      return {
        from,
        to,
        conflict,
        reason,
        keepBothTo: conflict ? uniqueArchiveTarget(to, reserved) : null,
      };
    });
  }

  function moveConflictCount(): number {
    return moveConflictReview?.items.filter((item) => item.conflict).length ?? 0;
  }

  function moveReadyCount(): number {
    return moveConflictReview?.items.filter((item) => !item.conflict).length ?? 0;
  }

  function visibleMoveConflictItems(): MovePlanItem[] {
    return moveConflictReview?.items.filter((item) => item.conflict).slice(0, 5) ?? [];
  }

  function moveTargetConflictCount(): number {
    if (!currentArchive || selectedPaths().size === 0) return 0;
    return buildMovePlan().filter((item) => item.conflict).length;
  }

  function archiveConflictCoverageNote(): string {
    return allRowsLoaded() ? "" : tr("gui.archive.full_validated_when_queued_suffix", " · full archive validated when task starts");
  }

  function moveTargetStatus(): string {
    const targetDir = normalizeMoveTargetDir();
    if (!currentArchive) return openArchiveFirstLabel();
    const selected = selectedPaths().size;
    if (selected === 0) {
      return tr("gui.move.select_entries_to_move_into", "Select entries to move into {target}").replace("{target}", targetDir);
    }
    const conflicts = moveTargetConflictCount();
    if (conflicts > 0) {
      return tr("gui.move.target_conflicts", "{count} target conflicts in {target}")
        .replace("{count}", conflicts.toLocaleString())
        .replace("{target}", targetDir);
    }
    return tr("gui.move.selected_to_target", "{count} selected -> {target}")
      .replace("{count}", selected.toLocaleString())
      .replace("{target}", targetDir) + archiveConflictCoverageNote();
  }

  function normalizeNewFolderPath(value = newFolderName): string {
    const parts = value
      .replaceAll("\\", "/")
      .split("/")
      .map((part) => part.trim())
      .filter((part) => part.length > 0 && part !== "." && part !== "..");
    const name = parts.length === 0 ? "New Folder" : parts.join("/");
    return `${name}/`;
  }

  function commitNewFolderName(value = newFolderName) {
    newFolderName = normalizeNewFolderPath(value);
  }

  function newFolderStatus(): string {
    const folder = normalizeNewFolderPath();
    if (!currentArchive) return openArchiveFirstLabel();
    const existing = archivePathSet();
    if (existing.has(folder) || existing.has(folder.slice(0, -1))) {
      return tr("gui.new_folder.already_exists", "Already exists: {folder}").replace("{folder}", folder);
    }
    return allRowsLoaded()
      ? tr("gui.new_folder.ready_to_create", "Ready to create {folder}").replace("{folder}", folder)
      : tr("gui.new_folder.loaded_rows_clear", "Loaded rows are clear for {folder} · full archive validated when task starts").replace("{folder}", folder);
  }

  function archivePathSet(): Set<string> {
    const paths = new Set<string>();
    for (const entry of loadedRows()) {
      const path = entry.path;
      paths.add(path);
      paths.add(path.endsWith("/") ? path.slice(0, -1) : `${path}/`);
    }
    return paths;
  }

  function archiveLikePath(path: string): boolean {
    return archiveExtensionMatch(path) !== null;
  }

  function selectedPreviewPath(): string | null {
    const selected = [...selectedPaths()];
    return selected.length === 1 ? selected[0] : null;
  }

  function entryTypeForPath(entryPath: string): EntryDto["entry_type"] | null {
    return loadedRows().find((row) => row.path === entryPath)?.entry_type ?? null;
  }

  function nestedPreviewTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase === "nested") return tr("gui.preview.loading", "Loading preview");
    if (!nestedPreview) return tr("gui.preview.no_nested", "Preview");
    return `${pathBaseName(nestedPreview.entry_path)} · ${nestedPreview.format.toUpperCase()}`;
  }

  function nestedPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase === "nested") {
      return tr("gui.preview.loading_target", "Preparing {name}...")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry", "selected entry"));
    }
    if (!nestedPreview) return tr("gui.preview.select_file", "Select one file and choose Preview.");
    return tr("gui.preview.nested_entries", "{count} entries{suffix}")
      .replace("{count}", nestedPreview.entry_count.toLocaleString())
      .replace("{suffix}", nestedPreview.truncated ? tr("gui.preview.first_200_shown_suffix", " · first 200 shown") : "");
  }

  function nestedPreviewRows(): EntryDto[] {
    return nestedPreview?.items.slice(0, 5) ?? [];
  }

  function entryPreviewTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") return tr("gui.preview.loading", "Loading preview");
    if (entryPreviewFailure) return tr("gui.preview.failed_title", "Preview did not open");
    if (!entryPreview) return tr("gui.preview.no_file", "Preview");
    return entryPreview.display_name;
  }

  function entryPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") {
      return tr("gui.preview.loading_target", "Preparing {name}...")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry", "selected entry"));
    }
    if (entryPreviewFailure) {
      return tr("gui.preview.failed_subtitle", "Could not prepare {name}")
        .replace("{name}", entryPreviewFailure.displayName);
    }
    if (!entryPreview) {
      return tr("gui.preview.select_file", "Select one file and choose Preview.");
    }
    return formatBytes(entryPreview.size);
  }

  function entryPreviewImageSrc(): string | null {
    return entryPreview?.preview_data_url ?? null;
  }

  function selectedPreviewPolicyCode(): PreviewPolicyCode {
    return selectedPreviewPolicy().code;
  }

  function activePreviewPolicyKind(): PreviewPolicyKind | "failed" {
    if (entryPreviewFailure) return "failed";
    if (nestedPreview) return "nested";
    if (entryPreview) return entryPreview.preview_data_url ? "inline-image" : "system-file";
    return selectedPreviewPolicy().kind;
  }

  function entryPreviewPolicyCode(): PreviewPolicyCode {
    if (!entryPreview) return selectedPreviewPolicyCode();
    return entryPreview.preview_data_url ? "inline_ready" : "system_ready";
  }

  function activeEntryPreviewPolicyCode(): PreviewPolicyCode {
    if (entryPreviewFailure) return "failed";
    if (entryPreview) return entryPreviewPolicyCode();
    return selectedPreviewPolicyCode();
  }

  function nestedPreviewPolicyCode(): PreviewPolicyCode {
    return "nested_ready";
  }

  function activePreviewPolicyCode(): PreviewPolicyCode {
    if (nestedPreview) return nestedPreviewPolicyCode();
    return activeEntryPreviewPolicyCode();
  }

  function retryEntryPreview() {
    if (!entryPreviewFailure) return;
    void submitPreviewEntry(entryPreviewFailure.entryPath, entryPreviewFailure.entryType);
  }

  function previewBusy(): boolean {
    return previewPhase !== "idle";
  }

  async function waitForPreviewFeedbackFrame() {
    await tick();
    if (import.meta.env.DEV && previewDelayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, previewDelayMs));
    }
  }

  function extractSelectionLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    const count = selectedPaths().size;
    return count > 0
      ? tr("gui.selection.count_selected", "{count} selected").replace("{count}", count.toLocaleString())
      : tr("gui.selection.all_entries", "All entries");
  }

  function extractPasswordLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (passwordBookStatus.saved) return tr("gui.password.book_can_unlock", "Password Book can unlock if needed");
    return tr("gui.password.ask_only_if_required", "Ask only if the archive requires it");
  }

  function extractEncodingLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (currentArchive.encoding_override) return currentArchive.encoding_override.toUpperCase();
    if (currentArchive.suggested_encoding) {
      return tr("gui.archive.encoding_suggested", "{encoding} suggested").replace("{encoding}", currentArchive.suggested_encoding.toUpperCase());
    }
    return tr("gui.archive.encoding_utf8_clean", "UTF-8 clean");
  }

  function archiveLine(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return tr("gui.archive.line", "{name} · {count} entries · {format}")
      .replace("{name}", currentArchive.name)
      .replace("{count}", currentArchive.entry_count.toLocaleString())
      .replace("{format}", archiveFormat());
  }

  function extractDestinationHint(): string {
    return tr("gui.extract.destination_hint", "Will extract to {destination}")
      .replace("{destination}", effectiveExtractDest());
  }

  function extractArchiveRequiredReason(): string {
    return currentArchive ? "" : tr("gui.precondition.open_before_extract", "Open an archive before extracting");
  }

  function convertArchiveRequiredReason(): string {
    return currentArchive ? "" : tr("gui.precondition.open_before_convert", "Open an archive before converting");
  }

  function archiveInfoRows(): Array<[string, string]> {
    if (!currentArchive) {
      return [[tr("gui.inspector.archive", "Archive"), openArchiveFirstLabel()]];
    }
    return [
      [tr("common.name", "Name"), currentArchive.name],
      [tr("common.path", "Path"), currentArchive.path],
      [tr("gui.archive.format", "Format"), archiveFormat()],
      [tr("gui.table.entries", "Entries"), currentArchive.entry_count.toLocaleString()],
      [tr("gui.archive.encoding", "Encoding"), extractEncodingLabel()],
      [
        tr("gui.archive.volumes", "Volumes"),
        currentArchive.volumes?.length
          ? archiveVolumeCountLabel(currentArchive.volumes.length)
          : tr("gui.archive.single", "Single"),
      ],
      [tr("common.selection", "Selection"), selectedSummary()],
      [tr("gui.extract.final_destination", "Final destination"), effectiveExtractDest()],
    ];
  }

  async function submitCurrentArchiveJob(spec: JobSpec, success: string, missing: string): Promise<boolean> {
    if (!currentArchive) {
      showNotice(missing);
      return false;
    }
    if (focusBlockingTaskIfAny()) return false;
    try {
      await submitJob(spec);
      showNotice(success);
      return true;
    } catch (error) {
      if (isJobSubmitBlocked(error)) return false;
      showNotice(tr("gui.job.requires_desktop_service_after_success_label", "{label} requires the desktop service").replace("{label}", success));
      return false;
    }
  }

  async function submitExtractJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_extract", "Open an archive before extracting"));
      return;
    }
    const selection = selectedJobPaths();
    const destination = effectiveExtractDest();
    const action = selection ? tr("gui.extract.selected_queued", "Extract selected started") : tr("gui.extract.all_queued", "Extract all started");
    const success = tr("gui.extract.started_to_destination", "{action} · destination: {destination}")
      .replace("{action}", action)
      .replace("{destination}", destination);
    const queued = await submitCurrentArchiveJob(
      {
        kind: "extract",
        path: currentArchive.path,
        dest: destination,
        selection,
        overwrite: extractOverwriteMode,
        symlinks: "preserve",
        smart: extractDestinationMode === "smart",
        encoding: archiveEncodingForJob(),
        password: null,
        best_effort: false,
      },
      success,
      tr("gui.precondition.open_before_extract", "Open an archive before extracting"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: action,
        detail: `${archiveTitle()} -> ${destination}`,
      });
    }
  }

  async function submitCopyOutSelectedJob() {
    const selection = selectedJobPaths();
    if (!currentArchive || !selection) {
      showNotice(copyOutSelectedDisabledReason());
      return;
    }
    const destination = effectiveExtractDest();
    const action = tr("gui.copy_out.selected_queued", "Copy out selected started");
    const success = tr("gui.copy_out.started_to_destination", "{action} · destination: {destination}")
      .replace("{action}", action)
      .replace("{destination}", destination);
    const queued = await submitCurrentArchiveJob(
      {
        kind: "extract",
        path: currentArchive.path,
        dest: destination,
        selection,
        overwrite: extractOverwriteMode,
        symlinks: "preserve",
        smart: extractDestinationMode === "smart",
        encoding: archiveEncodingForJob(),
        password: null,
        best_effort: false,
      },
      success,
      tr("gui.precondition.open_before_copy_out", "Open an archive before copying entries out"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: action,
        detail: `${archiveTitle()} -> ${destination}`,
      });
    }
  }

  async function startBatchExtract() {
    const paths = batchArchivePaths.length > 0
      ? batchArchivePaths
      : currentArchive
        ? [currentArchive.path]
        : [];
    if (paths.length === 0) {
      showNotice(tr("gui.batch.open_archives_before_start", "Open archives before starting batch extract"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "batch_extract",
        items: paths.map((path) => ({
          path,
          dest: extractDestForPath(path),
          encoding: currentArchive?.path === path ? archiveEncodingForJob() : null,
          password: null,
          best_effort: false,
        })),
        overwrite: "ask",
        symlinks: "preserve",
        smart: true,
      });
      showNotice(tr("gui.batch.extract_job_queued", "{count} archives started as one task").replace("{count}", paths.length.toLocaleString()));
      recordOperation({
        status: "queued",
        title: tr("gui.batch.extract_queued", "Batch extract started"),
        detail: tr("gui.batch.archive_count", "{count} archives").replace("{count}", paths.length.toLocaleString()),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.batch.requires_desktop_service", "Batch extract requires the desktop service"));
    }
  }

  function checksumTarget(): string {
    if (checksumPath.trim()) return checksumPath.trim();
    return currentArchive ? currentArchive.path : "";
  }

  function checksumTargetLabel(): string {
    const target = checksumTarget();
    return target ? target : tr("gui.checksum.choose_file_or_folder", "Choose a file or folder");
  }

  function checksumTargetName(): string {
    const target = checksumTarget();
    return target ? pathBaseName(target) || target : tr("gui.checksum.no_target_selected", "No target selected");
  }

  function checksumManifestLabel(): string {
    return checksumManifestPath.trim() || tr("gui.checksum.choose_manifest_prompt", "Choose a checksum manifest");
  }

  function latestChecksumTask(kind: "checksum" | "checksum_check"): Task | null {
    for (let index = jobRows.length - 1; index >= 0; index -= 1) {
      const task = jobRows[index];
      if (task.spec.kind === kind) return task;
    }
    return null;
  }

  function checksumResultNumber(kind: "checksum" | "checksum_check", key: string): number {
    const value = latestChecksumTask(kind)?.result?.[key];
    return typeof value === "number" && Number.isFinite(value) ? value : 0;
  }

  function checksumItems(kind: "checksum" | "checksum_check"): Record<string, unknown>[] {
    const items = latestChecksumTask(kind)?.result?.items;
    if (!Array.isArray(items)) return [];
    return items.filter((item): item is Record<string, unknown> => item !== null && typeof item === "object" && !Array.isArray(item));
  }

  function checksumItemNumber(item: Record<string, unknown>, key: string): number {
    const value = item[key];
    return typeof value === "number" && Number.isFinite(value) ? value : 0;
  }

  function checksumResultText(kind: "checksum" | "checksum_check"): string {
    return checksumItems(kind)
      .map((item) => checksumResultLine(kind, item))
      .filter((line) => line.trim().length > 0)
      .join("\n");
  }

  async function writeClipboardText(text: string): Promise<boolean> {
    if (!text.trim()) return false;
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      const textArea = document.createElement("textarea");
      textArea.value = text;
      textArea.setAttribute("readonly", "true");
      textArea.className = "clipboard-proxy";
      document.body.appendChild(textArea);
      textArea.select();
      try {
        return document.execCommand("copy");
      } catch {
        return false;
      } finally {
        textArea.remove();
      }
    }
  }

  async function copyChecksumText(
    text: string,
    kind: "checksum" | "checksum_check" | "task",
    taskId: number | null = null,
  ) {
    if (!text.trim()) {
      const message = tr("gui.checksum.no_copyable_results", "No checksum results to copy");
      showNotice(message);
      showChecksumCopyFeedback(kind, taskId, message, "danger");
      return;
    }
    const ok = await writeClipboardText(text);
    const message = ok
      ? tr("gui.checksum.results_copied", "Checksum results copied")
      : tr("gui.checksum.copy_failed", "Could not copy checksum results");
    showNotice(message);
    showChecksumCopyFeedback(kind, taskId, message, ok ? "success" : "danger");
  }

  async function copyChecksumResults(kind: "checksum" | "checksum_check") {
    await copyChecksumText(checksumResultText(kind), kind);
  }

  async function copyTaskChecksumResults(task: TaskDialogModel) {
    await copyChecksumText(taskChecksumResultText(task), "task", task.id);
  }

  function checksumExcludeRules(): string[] {
    return parseDelimitedRules(checksumExcludeText);
  }

  function checksumAlgorithmLabel(algorithm: ChecksumAlgorithmId): string {
    if (algorithm === "sha256") return "SHA-256";
    if (algorithm === "sha224") return "SHA-224";
    if (algorithm === "sha384") return "SHA-384";
    if (algorithm === "sha512") return "SHA-512";
    if (algorithm === "sha1") return "SHA-1";
    if (algorithm === "md5") return "MD5";
    if (algorithm === "blake3") return "BLAKE3";
    return "CRC32";
  }

  function checksumAlgorithmHint(algorithm: ChecksumAlgorithmId): string {
    if (algorithm === "sha256") return tr("gui.checksum.algorithm_sha256_hint", "Default");
    if (algorithm === "sha224") return tr("gui.checksum.algorithm_sha224_hint", "SHA-2");
    if (algorithm === "sha384") return tr("gui.checksum.algorithm_sha384_hint", "Signed releases");
    if (algorithm === "sha512") return tr("gui.checksum.algorithm_sha512_hint", "Wide SHA-2");
    if (algorithm === "sha1") return tr("gui.checksum.algorithm_sha1_hint", "Legacy");
    if (algorithm === "md5") return tr("gui.checksum.algorithm_md5_hint", "Legacy");
    if (algorithm === "blake3") return tr("gui.checksum.algorithm_blake3_hint", "Fast");
    return tr("gui.checksum.algorithm_crc32_hint", "ZIP CRC");
  }

  function selectChecksumAlgorithm(value: ChecksumAlgorithmId) {
    checksumAlgorithm = value;
  }

  async function chooseChecksumFile() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("checksum.file", open, {
        title: tr("gui.checksum.choose_file_to_checksum", "Choose file to checksum"),
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) {
        showNotice(tr("gui.checksum.target_unchanged", "Checksum target unchanged"));
        return;
      }
      checksumPath = selected;
      showNotice(tr("gui.checksum.target_selected", "Checksum target selected"));
    } catch {
      showNotice(tr("gui.checksum.choose_file_requires_desktop_dialog", "Choosing a checksum file requires the desktop file dialog"));
    }
  }

  async function chooseChecksumFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("checksum.folder", open, {
        title: tr("gui.checksum.choose_folder_to_checksum", "Choose folder to checksum"),
        directory: true,
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) {
        showNotice(tr("gui.checksum.target_unchanged", "Checksum target unchanged"));
        return;
      }
      checksumPath = selected;
      showNotice(tr("gui.checksum.folder_selected", "Checksum folder selected"));
    } catch {
      showNotice(tr("gui.checksum.choose_folder_requires_desktop_dialog", "Choosing a checksum folder requires the desktop file dialog"));
    }
  }

  async function chooseChecksumManifest() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("checksum.manifest", open, {
        title: tr("gui.checksum.choose_manifest", "Choose checksum manifest"),
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) {
        showNotice(tr("gui.checksum.manifest_unchanged", "Checksum manifest unchanged"));
        return;
      }
      checksumManifestPath = selected;
      showNotice(tr("gui.checksum.manifest_selected", "Checksum manifest selected"));
    } catch {
      showNotice(tr("gui.checksum.choose_manifest_requires_desktop_dialog", "Choosing a checksum manifest requires the desktop file dialog"));
    }
  }

  function useCurrentArchiveForChecksum() {
    if (!currentArchive) {
      showNotice(tr("gui.checksum.open_archive_or_choose_file", "Open an archive or choose a file first"));
      return;
    }
    checksumPath = currentArchive.path;
    showNotice(tr("gui.checksum.target_current_archive", "Checksum target set to current archive"));
  }

  async function submitChecksumJob() {
    const target = checksumTarget();
    if (!target) {
      showNotice(tr("gui.checksum.choose_before_checksumming", "Choose a file or folder before checksumming"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "checksum",
        inputs: [target],
        excludes: checksumExcludeRules(),
        algorithm: checksumAlgorithm,
      });
      showNotice(tr("gui.checksum.queued", "Checksum started"));
      recordOperation({
        status: "queued",
        title: tr("gui.checksum.queued", "Checksum started"),
        detail: `${pathBaseName(target) || target} · ${checksumAlgorithm}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.checksum.requires_desktop_service", "Checksum requires the desktop service"));
    }
  }

  async function submitChecksumCheckJob() {
    const manifest = checksumManifestPath.trim();
    if (!manifest) {
      showNotice(tr("gui.checksum.choose_manifest_before_verifying", "Choose a checksum manifest before verifying"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "checksum_check",
        manifest,
        algorithm: checksumAlgorithm,
      });
      showNotice(tr("gui.checksum.verification_queued", "Checksum verification started"));
      recordOperation({
        status: "queued",
        title: tr("gui.checksum.verification_queued", "Checksum verification started"),
        detail: `${pathBaseName(manifest) || manifest} · ${checksumAlgorithm}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.checksum.verification_requires_desktop_service", "Checksum verification requires the desktop service"));
    }
  }

  function duplicateScanTarget(): string {
    if (duplicateScanPath.trim()) return duplicateScanPath.trim();
    return currentArchive ? pathDir(currentArchive.path) : "";
  }

  function duplicateScanTargetLabel(): string {
    const target = duplicateScanTarget();
    return target ? target : tr("gui.duplicates.choose_folder_or_open_archive", "Choose a folder or open an archive");
  }

  function duplicateScanTargetName(): string {
    const target = duplicateScanTarget();
    return target ? pathBaseName(target) || target : tr("gui.duplicates.no_folder_selected", "No folder selected");
  }

  function latestDuplicateScanTask(): Task | null {
    for (let index = jobRows.length - 1; index >= 0; index -= 1) {
      const task = jobRows[index];
      if (task.spec.kind === "duplicate_scan") return task;
    }
    return null;
  }

  function duplicateResultNumber(key: string): number {
    const value = latestDuplicateScanTask()?.result?.[key];
    return typeof value === "number" && Number.isFinite(value) ? value : 0;
  }

  function duplicateExcludeRules(): string[] {
    return parseDelimitedRules(duplicateExcludeText);
  }

  function duplicateMinSizeInvalidMessage(): string {
    return tr("gui.duplicates.min_size_invalid", "Use a whole number of bytes, 0 or more");
  }

  function parseDuplicateMinSizeInput(input: HTMLInputElement): number | null {
    const raw = input.value.trim();
    const value = Number(raw);
    if (!raw || !Number.isFinite(value) || !Number.isInteger(value) || value < 0) {
      return null;
    }
    return value;
  }

  function updateDuplicateMinSizeFromInput(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const value = parseDuplicateMinSizeInput(input);
    if (value === null) {
      duplicateMinSizeError = duplicateMinSizeInvalidMessage();
      showNotice(duplicateMinSizeError);
      return;
    }
    duplicateMinSizeError = "";
    duplicateMinSize = value;
  }

  async function chooseDuplicateScanFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("duplicates.folder", open, {
        title: tr("gui.duplicates.choose_folder_to_scan", "Choose folder to scan"),
        directory: true,
        multiple: false,
      });
      if (!selected || Array.isArray(selected)) {
        showNotice(tr("gui.duplicates.folder_unchanged", "Duplicate scan folder unchanged"));
        return;
      }
      duplicateScanPath = selected;
      showNotice(tr("gui.duplicates.folder_selected", "Duplicate scan folder selected"));
    } catch {
      showNotice(tr("gui.duplicates.choose_folder_requires_desktop_dialog", "Choosing a scan folder requires the desktop file dialog"));
    }
  }

  function useCurrentArchiveFolderForDuplicates() {
    if (!currentArchive) {
      showNotice(tr("gui.duplicates.open_archive_or_choose_folder", "Open an archive or choose a folder first"));
      return;
    }
    duplicateScanPath = pathDir(currentArchive.path);
    showNotice(tr("gui.duplicates.target_archive_folder", "Duplicate scan target set to archive folder"));
  }

  async function submitDuplicateScanJob() {
    if (duplicateMinSizeError) {
      showNotice(duplicateMinSizeError);
      return;
    }
    const target = duplicateScanTarget();
    if (!target) {
      showNotice(tr("gui.duplicates.choose_folder_before_scan", "Choose a folder before scanning for duplicates"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "duplicate_scan",
        inputs: [target],
        excludes: duplicateExcludeRules(),
        min_size: Math.max(0, Math.floor(duplicateMinSize)),
      });
      showNotice(tr("gui.duplicates.queued", "Duplicate scan started"));
      recordOperation({
        status: "queued",
        title: tr("gui.duplicates.queued", "Duplicate scan started"),
        detail: tr("gui.duplicates.operation_detail", "{target} · min {min}")
          .replace("{target}", pathBaseName(target) || target)
          .replace("{min}", formatBytes(duplicateMinSize)),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.duplicates.requires_desktop_service", "Duplicate scan requires the desktop service"));
    }
  }

  async function submitBestEffortExtractJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_before_best_effort", "Open an archive before best-effort extract"));
      return;
    }
    const selection = selectedJobPaths();
    const dest = `${defaultExtractDest()}-readable`;
    const queued = await submitCurrentArchiveJob(
      {
        kind: "extract",
        path: currentArchive.path,
        dest,
        selection,
        overwrite: "rename",
        symlinks: "preserve",
        smart: true,
        encoding: archiveEncodingForJob(),
        password: null,
        best_effort: true,
      },
      selection ? tr("gui.recovery.best_effort_selected_queued", "Best-effort extract selected started") : tr("gui.recovery.best_effort_queued", "Best-effort extract started"),
      tr("gui.recovery.open_before_best_effort", "Open an archive before best-effort extract"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: selection ? tr("gui.recovery.best_effort_selected_queued", "Best-effort extract selected started") : tr("gui.recovery.best_effort_queued", "Best-effort extract started"),
        detail: `${archiveTitle()} -> ${pathBaseName(dest)}`,
      });
    }
  }

  async function submitTestJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_test", "Open an archive before testing"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "test",
        path: currentArchive.path,
        encoding: archiveEncodingForJob(),
        password: null,
      },
      tr("gui.test.queued", "Archive test started"),
      tr("gui.precondition.open_before_test", "Open an archive before testing"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.test.queued", "Archive test started"),
        detail: archiveTitle(),
      });
    }
  }

  async function submitConvertJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_convert", "Open an archive before converting"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "convert",
        src: currentArchive.path,
        dest: defaultConvertDest(),
        level: createCompressionLevel(),
        src_encoding: archiveEncodingForJob(),
        src_password: null,
        dest_password: null,
        encrypt_names: false,
      },
      tr("gui.convert.queued", "Convert started"),
      tr("gui.precondition.open_before_convert", "Open an archive before converting"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.convert.queued", "Convert started"),
        detail: `${archiveTitle()} -> ${pathBaseName(defaultConvertDest())} · ${createProfileLabel(activeCreateProfile)}`,
      });
    }
  }

  async function submitExportSqzJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_sqz_before_export", "Open an SQZ archive before exporting"));
      return;
    }
    if (currentArchive.format.toLowerCase() !== "sqz") {
      showNotice(tr("gui.recovery.open_sqz_before_export", "Open an SQZ archive before exporting"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const { save } = await getDialogModule();
      const dest = await saveNativeDialog("recovery.export-sqz", save, {
        title: tr("gui.recovery.export_sqz_as", "Export SQZ as"),
        defaultPath: defaultSqzExportDest(),
        filters: [
          { name: archiveOutputFilterName("zip"), extensions: ["zip"] },
          { name: archiveOutputFilterName("7z"), extensions: ["7z"] },
          { name: archiveOutputFilterName("tar.zst"), extensions: ["tar.zst", "tzst"] },
          { name: archiveOutputFilterName("tar"), extensions: ["tar"] },
        ],
      });
      if (!dest) {
        showNotice(tr("gui.recovery.export_sqz_cancelled", "Export SQZ cancelled"));
        return;
      }
      await submitJob({
        kind: "export_sqz",
        src: currentArchive.path,
        dest,
        level: createCompressionLevel(),
        dest_password: null,
      });
      showNotice(tr("gui.recovery.sqz_export_queued", "SQZ export started"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.sqz_export_queued", "SQZ export started"),
        detail: `${archiveTitle()} -> ${pathBaseName(dest)} · ${createProfileLabel(activeCreateProfile)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.export_sqz_requires_desktop_service", "Export SQZ requires the desktop service"));
    }
  }

  async function submitRepairSqzJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_sqz_before_repair", "Open an SQZ archive before repairing"));
      return;
    }
    if (currentArchive.format.toLowerCase() !== "sqz") {
      showNotice(tr("gui.recovery.open_sqz_before_repair", "Open an SQZ archive before repairing"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const { save } = await getDialogModule();
      const dest = await saveNativeDialog("recovery.repair-sqz", save, {
        title: tr("gui.recovery.repair_sqz_as", "Repair SQZ as"),
        defaultPath: defaultSqzRepairDest(),
        filters: [{ name: archiveOutputFilterName("sqz"), extensions: ["sqz"] }],
      });
      if (!dest) {
        showNotice(tr("gui.recovery.repair_sqz_cancelled", "Repair SQZ cancelled"));
        return;
      }
      await submitJob({
        kind: "repair_sqz",
        src: currentArchive.path,
        dest,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.recovery.sqz_repair_queued", "SQZ repair started"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.sqz_repair_queued", "SQZ repair started"),
        detail: `${archiveTitle()} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.repair_sqz_requires_desktop_service", "Repair SQZ requires the desktop service"));
    }
  }

  async function submitRepairZipJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_zip_before_rebuild", "Open a ZIP archive before rebuilding its index"));
      return;
    }
    if (!isCurrentArchiveZipFamily()) {
      showNotice(tr("gui.recovery.zip_rebuild_zip_family_only", "ZIP index rebuild is available for ZIP-family archives"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const { save } = await getDialogModule();
      const dest = await saveNativeDialog("recovery.rebuild-zip-index", save, {
        title: tr("gui.recovery.rebuild_zip_index_as", "Rebuild ZIP index as"),
        defaultPath: defaultZipRepairDest(),
        filters: [{ name: archiveOutputFilterName("zip"), extensions: ["zip"] }],
      });
      if (!dest) {
        showNotice(tr("gui.recovery.zip_rebuild_cancelled", "ZIP index rebuild cancelled"));
        return;
      }
      await submitJob({
        kind: "repair_zip",
        src: currentArchive.path,
        dest,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.recovery.zip_rebuild_queued", "ZIP index rebuild started"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.zip_rebuild_queued", "ZIP index rebuild started"),
        detail: `${archiveTitle()} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.zip_rebuild_requires_desktop_service", "ZIP index rebuild requires the desktop service"));
    }
  }

  async function submitProtectJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_before_par2_protect", "Open an archive before creating PAR2 recovery data"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "protect",
        path: currentArchive.path,
        redundancy: 10,
        recovery: null,
      },
      tr("gui.recovery.par2_protection_queued", "PAR2 protection started"),
      tr("gui.recovery.open_before_par2_protect", "Open an archive before creating PAR2 recovery data"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.par2_protection_queued", "PAR2 protection started"),
        detail: archiveTitle(),
      });
    }
  }

  async function submitVerifyRecoveryJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_before_verify", "Open an archive before verifying recovery data"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "verify_recovery",
        path: currentArchive.path,
        recovery: null,
      },
      tr("gui.recovery.par2_verify_queued", "PAR2 verify started"),
      tr("gui.recovery.open_before_verify", "Open an archive before verifying recovery data"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.par2_verify_queued", "PAR2 verify started"),
        detail: archiveTitle(),
      });
    }
  }

  async function submitRepairRecoveryJob() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.open_before_par2_repair", "Open an archive before repairing with PAR2 recovery data"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "repair_recovery",
        path: currentArchive.path,
        output: null,
        recovery: null,
      },
      tr("gui.recovery.par2_repair_queued", "PAR2 repair started"),
      tr("gui.recovery.open_before_par2_repair", "Open an archive before repairing with PAR2 recovery data"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.par2_repair_queued", "PAR2 repair started"),
        detail: archiveTitle(),
      });
    }
  }

  async function submitAddToArchiveJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_add", "Open an archive before adding files"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("archive.add-files", open, {
        title: tr("gui.add.choose_files_to_add", "Choose files to add"),
        multiple: true,
        directory: false,
      });
      const add = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (add.length === 0) {
        showNotice(tr("gui.add.cancelled", "Add files cancelled"));
        return;
      }
      await submitJob({
        kind: "update",
        path: currentArchive.path,
        add,
        delete: [],
        rename: [],
        mkdir: [],
        excludes: createExcludeRules(),
        password: null,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.add.operations_queued", "{count} add operations started").replace("{count}", add.length.toLocaleString()));
      recordOperation({
        status: "queued",
        title: tr("gui.add.queued", "Add files started"),
        detail: tr("gui.add.items_profile", "{count} items · {profile}")
          .replace("{count}", add.length.toLocaleString())
          .replace("{profile}", createProfileLabel(activeCreateProfile)),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.add.requires_desktop_dialog", "Add files requires the desktop file dialog"));
    }
  }

  async function submitDeleteSelectedJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_delete", "Open an archive before deleting entries"));
      return;
    }
    const patterns = selectedDeletePatterns();
    if (patterns.length === 0) {
      showNotice(tr("gui.precondition.select_entries_before_delete", "Select entries before deleting"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.path,
        add: [],
        delete: patterns,
        rename: [],
        mkdir: [],
        excludes: [],
        password: null,
        level: 6,
      },
      (patterns.length === 1
        ? tr("gui.delete.operation_queued", "1 delete operation started")
        : tr("gui.delete.operations_queued", "{count} delete operations started").replace("{count}", patterns.length.toLocaleString())),
      tr("gui.precondition.open_before_delete", "Open an archive before deleting entries"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.delete.queued", "Delete entries started"),
        detail: tr("gui.delete.entries_from_archive", "{count} entries from {archive}")
          .replace("{count}", patterns.length.toLocaleString())
          .replace("{archive}", archiveTitle()),
      });
    }
  }

  async function submitRenameSelectedJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_rename", "Open an archive before renaming entries"));
      return;
    }
    const selected = [...selectedPaths()].filter((path) => !path.endsWith("/"));
    if (selected.length !== 1) {
      showNotice(tr("gui.precondition.select_one_before_rename", "Select exactly one file entry before renaming"));
      return;
    }
    const from = selected[0];
    const to = normalizeRenameTargetName(renameTargetName, from);
    renameTargetName = to;
    if (to === from) {
      showNotice(tr("gui.rename.target_must_differ", "Rename target must differ from source"));
      return;
    }
    if (archivePathSet().has(to)) {
      showNotice(tr("gui.rename.target_already_exists", "Rename target already exists: {target}").replace("{target}", to));
      return;
    }
    const issue = renameTargetIssue(from, to);
    if (issue.blocking) {
      showNotice(tr("gui.rename.target_blocked", "Rename target blocked: {reason}").replace("{reason}", issue.blocking));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.path,
        add: [],
        delete: [],
        rename: [{ from, to }],
        mkdir: [],
        excludes: [],
        password: null,
        level: 6,
      },
      tr("gui.rename.queued_notice", "Rename started: {from} -> {to}").replace("{from}", from).replace("{to}", to),
      tr("gui.precondition.open_before_rename", "Open an archive before renaming entries"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.rename.queued", "Rename entry started"),
        detail: `${from} -> ${to}`,
      });
    }
  }

  async function submitMoveSelectedJob() {
    const targetDir = normalizeMoveTargetDir();
    moveTargetDir = targetDir;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_move", "Open an archive before moving entries"));
      return;
    }
    const selected = [...selectedPaths()];
    if (selected.length === 0) {
      showNotice(tr("gui.precondition.select_entries_before_move", "Select entries before moving"));
      return;
    }
    const plan = buildMovePlan(targetDir);
    const conflicts = plan.filter((item) => item.conflict);
    if (conflicts.length > 0) {
      moveConflictReview = { targetDir, items: plan };
      showNotice(tr("gui.move.review_conflicts", "Review {count} move target conflicts").replace("{count}", conflicts.length.toLocaleString()));
      return;
    }
    await submitMovePlan(plan.map(({ from, to }) => ({ from, to })), targetDir);
  }

  async function submitMovePlan(rename: Array<{ from: string; to: string }>, targetDir: string) {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_move", "Open an archive before moving entries"));
      return;
    }
    if (rename.length === 0) {
      showNotice(tr("gui.move.no_non_conflicting_targets", "No non-conflicting move targets to submit"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.path,
        add: [],
        delete: [],
        rename,
        mkdir: [targetDir],
        excludes: [],
        password: null,
        level: 6,
      },
      (rename.length === 1
        ? tr("gui.move.operation_queued", "1 move operation started")
        : tr("gui.move.operations_queued", "{count} move operations started").replace("{count}", rename.length.toLocaleString())),
      tr("gui.precondition.open_before_move", "Open an archive before moving entries"),
    );
    if (queued) {
      moveConflictReview = null;
      recordOperation({
        status: "queued",
        title: tr("gui.move.queued", "Move entries started"),
        detail: tr("gui.move.entries_to_target", "{count} entries to {target}")
          .replace("{count}", rename.length.toLocaleString())
          .replace("{target}", targetDir),
      });
    }
  }

  async function submitMoveReadyOnly() {
    const review = moveConflictReview;
    if (!review) return;
    const ready = review.items
      .filter((item) => !item.conflict)
      .map(({ from, to }) => ({ from, to }));
    await submitMovePlan(ready, review.targetDir);
  }

  async function submitMoveKeepBoth() {
    const review = moveConflictReview;
    if (!review) return;
    const rename = review.items.map((item) => ({
      from: item.from,
      to: item.conflict && item.keepBothTo ? item.keepBothTo : item.to,
    }));
    await submitMovePlan(rename, review.targetDir);
  }

  async function submitNewFolderJob() {
    const folder = normalizeNewFolderPath();
    newFolderName = folder;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder"));
      return;
    }
    const existing = archivePathSet();
    if (existing.has(folder) || existing.has(folder.slice(0, -1))) {
      showNotice(tr("gui.new_folder.already_exists", "Already exists: {folder}").replace("{folder}", folder));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.path,
        add: [],
        delete: [],
        rename: [],
        mkdir: [folder],
        excludes: [],
        password: null,
        level: 6,
      },
      tr("gui.new_folder.queued_notice", "New folder started: {folder}").replace("{folder}", folder),
      tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.new_folder.queued", "New folder started"),
        detail: folder,
      });
    }
  }

  async function openArchiveDirectoryEntry(entryPath: string) {
    const name = pathBaseName(entryPath.replace(/\/+$/g, ""));
    if (!name) {
      showNotice(tr("gui.preview.open_folder_failed", "Folder preview failed"));
      return;
    }
    await enterDir(name);
    browseScrollTop = 0;
    clearSelection();
    nestedPreview = null;
    entryPreview = null;
    entryPreviewFailure = null;
    recordValidationEvent("frontend.entry.open_dir", {
      entry_path: entryPath,
      name,
      path: archiveDirs.join("/"),
    });
    showNotice(tr("gui.preview.folder_opened", "Opened folder: {name}").replace("{name}", name));
  }

  async function submitPreviewEntry(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
  ) {
    if (!currentArchive) {
      showNotice(tr("gui.preview.open_archive_first", "Open an archive before previewing entries"));
      return;
    }
    if (!entryPath) {
      showNotice(tr("gui.preview.select_one", "Select one file entry to preview"));
      return;
    }
    if (entryType === "dir" || entryPath.endsWith("/") || entryTypeForPath(entryPath) === "dir") {
      await openArchiveDirectoryEntry(entryPath);
      return;
    }
    if (archiveLikePath(entryPath)) {
      await submitPreviewNestedArchive(entryPath);
      return;
    }
    previewPhase = "entry";
    previewTargetName = pathBaseName(entryPath);
    nestedPreview = null;
    entryPreview = null;
    entryPreviewFailure = null;
    recordValidationEvent("frontend.entry.preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      entryPreview =
        previewSampleForEntry(currentArchive.path, entryPath) ??
        (await ipc.previewArchiveEntry(
          currentArchive.path,
          entryPath,
          null,
          archiveEncodingForJob(),
        ));
      nestedPreview = null;
      entryPreviewFailure = null;
      recordValidationEvent("frontend.entry.preview_loaded", {
        entry_path: entryPath,
        display_name: entryPreview.display_name,
        temp_path: entryPreview.temp_path,
        inline_preview: Boolean(entryPreview.preview_data_url),
      });
      showNotice(
        (entryPreview.preview_data_url
          ? tr("gui.preview.loaded_inline", "Preview opened: {name}")
          : tr("gui.preview.opening_system", "Opening: {name}"))
          .replace("{name}", entryPreview.display_name),
      );
      if (!entryPreview.preview_data_url) {
        previewPhase = "idle";
        previewTargetName = "";
        await openEntryPreview(entryPreview);
      }
      recordOperation({
        status: "info",
        title: tr("gui.preview.operation_title", "Archive entry previewed"),
        detail: `${pathBaseName(entryPath)} -> ${pathBaseName(entryPreview.temp_path)}`,
      });
    } catch {
      const failurePolicy = previewPolicyFor(entryPath, entryType ?? entryTypeForPath(entryPath));
      entryPreviewFailure = {
        entryPath,
        entryType: entryType ?? entryTypeForPath(entryPath),
        displayName: pathBaseName(entryPath),
        policyKind: failurePolicy.kind,
      };
      recordValidationEvent("frontend.entry.preview_failed", {
        entry_path: entryPath,
        policy_kind: failurePolicy.kind,
      });
      showNotice(tr("gui.preview.failed", "Preview failed"));
    } finally {
      previewPhase = "idle";
      previewTargetName = "";
    }
  }

  async function submitPreviewNestedArchive(entryPath: string | null = selectedPreviewPath()) {
    if (!currentArchive) {
      showNotice(tr("gui.preview.open_archive_first", "Open an archive before previewing entries"));
      return;
    }
    if (!entryPath) {
      showNotice(tr("gui.preview.select_one", "Select one file entry to preview"));
      return;
    }
    if (!archiveLikePath(entryPath)) {
      showNotice(tr("gui.preview.select_archive", "Select an archive-like entry, such as .zip, .7z, .dmg or .7z.001"));
      return;
    }
    previewPhase = "nested";
    previewTargetName = pathBaseName(entryPath);
    nestedPreview = null;
    entryPreview = null;
    entryPreviewFailure = null;
    recordValidationEvent("frontend.entry.nested_preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      nestedPreview = await ipc.previewNestedArchive(
        currentArchive.path,
        entryPath,
        null,
        archiveEncodingForJob(),
      );
      entryPreview = null;
      entryPreviewFailure = null;
      recordValidationEvent("frontend.entry.nested_preview_loaded", {
        entry_path: entryPath,
        entry_count: nestedPreview.entry_count,
        format: nestedPreview.format,
      });
      showNotice(
        tr("gui.preview.nested_loaded", "Nested preview loaded · {count} entries").replace(
          "{count}",
          nestedPreview.entry_count.toLocaleString(),
        ),
      );
      recordOperation({
        status: "info",
        title: tr("gui.preview.nested_operation_title", "Nested archive previewed"),
        detail: `${pathBaseName(entryPath)} · ${nestedPreview.format.toUpperCase()}`,
      });
    } catch {
      showNotice(tr("gui.preview.nested_failed", "Nested archive preview failed or unsupported"));
    } finally {
      previewPhase = "idle";
      previewTargetName = "";
    }
  }

  async function revealEntryPreview() {
    if (!entryPreview) {
      showNotice(tr("gui.preview.preview_first", "Preview a file entry first"));
      return;
    }
    try {
      await ipc.revealPreviewPath(entryPreview.temp_path);
    } catch {
      showNotice(tr("gui.preview.reveal_failed", "Cannot reveal the preview file in the file manager"));
    }
  }

  async function openEntryPreview(preview: EntryPreviewDto | null = entryPreview) {
    if (!preview) {
      showNotice(tr("gui.preview.preview_first", "Preview a file entry first"));
      return;
    }
    try {
      await ipc.openPreviewPath(preview.temp_path);
      showNotice(tr("gui.preview.opened_system", "Opened: {name}").replace("{name}", preview.display_name));
    } catch {
      showNotice(tr("gui.preview.open_failed_ready", "Could not open: {name}").replace("{name}", preview.display_name));
    }
  }

  async function openNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_open", "Preview a nested archive before opening it"));
      return;
    }
    try {
      const info = await ipc.openNestedArchive(
        preview.outer_path,
        preview.entry_path,
        null,
        currentArchive?.path === preview.outer_path ? archiveEncodingForJob() : null,
      );
      await adoptOpenedArchive(info);
      nestedPreview = null;
      entryPreviewFailure = null;
      showNotice(tr("gui.preview.opened_nested_archive", "Opened nested archive · {name}").replace("{name}", info.name));
      recordOperation({
        status: "done",
        title: tr("gui.preview.nested_opened_operation_title", "Nested archive opened"),
        detail: `${pathBaseName(preview.entry_path)} -> ${info.name}`,
      });
    } catch {
      showNotice(tr("gui.preview.open_nested_requires_desktop_service", "Open nested archive requires the desktop service or a supported nested archive"));
    }
  }

  async function extractNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_extract", "Preview a nested archive before extracting it"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const dest = nestedExtractDest(preview);
      await submitJob({
        kind: "extract_nested",
        outer_path: preview.outer_path,
        entry_path: preview.entry_path,
        dest,
        overwrite: "ask",
        symlinks: "preserve",
        smart: true,
        encoding: currentArchive?.path === preview.outer_path ? archiveEncodingForJob() : null,
        password: null,
        best_effort: false,
      });
      showNotice(tr("gui.preview.extract_nested_queued", "Extract nested started · {name}").replace("{name}", pathBaseName(preview.entry_path)));
      recordOperation({
        status: "queued",
        title: tr("gui.preview.nested_extract_queued_operation_title", "Nested archive extract started"),
        detail: `${pathBaseName(preview.entry_path)} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.preview.extract_nested_requires_desktop_service", "Extract nested archive requires the desktop service or a supported nested archive"));
    }
  }

  async function repairFilenameEncoding(encoding = "gbk") {
    if (!currentArchive) {
      showNotice(tr("gui.encoding.open_before_repair", "Open an archive before repairing filename encoding"));
      return;
    }
    const ok = await reopenWithEncoding(encoding);
    showNotice(
      ok
        ? tr("gui.encoding.reopened_with", "Filename encoding reopened with {encoding}").replace("{encoding}", encoding.toUpperCase())
        : tr("gui.encoding.reopen_failed", "Could not reopen archive with that encoding"),
    );
  }

  async function submitCreateJob(sourceKind: "files" | "folder") {
    if (focusBlockingTaskIfAny()) return;
    if (createPreflightBusy()) {
      showNotice(tr("gui.create.preflight_already_running", "Create preflight already running"));
      return;
    }
    createPreflightPhase = "selecting";
    showNotice(sourceKind === "files" ? tr("gui.create.opening_file_picker", "Opening file picker...") : tr("gui.create.opening_folder_picker", "Opening folder picker..."));
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog(`create.${sourceKind}`, open, {
        title: sourceKind === "files" ? tr("gui.create.choose_files_to_archive", "Choose files to archive") : tr("gui.create.choose_folder_to_archive", "Choose folder to archive"),
        multiple: sourceKind === "files",
        directory: sourceKind === "folder",
      });
      const inputs = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (inputs.length === 0) {
        createPreflightPhase = "idle";
        showNotice(tr("gui.create.cancelled", "Create archive cancelled"));
        return;
      }
      await submitCreateInputs(inputs, "dialog");
    } catch {
      createPreflightPhase = "idle";
      showNotice(tr("gui.create.requires_desktop_dialog", "Create archive requires the desktop file dialog"));
    }
  }

  async function submitCreateInputs(inputs: string[], source: "dialog" | "drop") {
    const normalizedInputs = uniqueNonEmptyPaths(inputs);
    if (normalizedInputs.length === 0) {
      createPreflightPhase = "idle";
      showNotice(tr("gui.create.no_source_items", "No source items selected"));
      return;
    }
    if (focusBlockingTaskIfAny()) {
      createPreflightPhase = "idle";
      return;
    }
    const excludes = createExcludeRules();
    await ensureCreatePreflightListener();
    createPreflightPhase = "measuring";
    createPreflightScanned = 0;
    createPreflightCurrent = "";
    lastCreateEstimate = null;
    lastDiskSpace = null;
    lastTempDiskSpace = null;
    lastCreateDest = null;
    let estimate: CreateEstimateDto;
    try {
      estimate = await ipc.estimateCreateInputs(normalizedInputs, excludes);
    } catch {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.check_excludes_or_permissions", "Check exclude rules or source permissions before creating"));
      return;
    }
    if (estimate.entries === 0) {
      lastCreateEstimate = estimate;
      lastDiskSpace = null;
      lastTempDiskSpace = null;
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.no_entries_after_excludes", "No entries after excludes"));
      return;
    }
    lastCreateEstimate = estimate;
    createPreflightScanned = estimate.entries;
    createPreflightCurrent = "";
    lastDiskSpace = null;
    lastTempDiskSpace = null;

    let tempDisk: DiskSpaceDto;
    try {
      createPreflightPhase = "checkingTemp";
      const tempPath = await ipc.tempDir();
      tempDisk = await ipc.checkDiskSpace(tempPath, requiredCreateDiskBytes(estimate));
    } catch {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.temp_preflight_requires_desktop_service", "Temporary disk preflight requires the desktop service"));
      return;
    }
    lastTempDiskSpace = tempDisk;
    if (!tempDisk.ok) {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.not_enough_temp_space", "Not enough temporary space · {available} available").replace("{available}", formatBytes(tempDisk.available_bytes)));
      return;
    }

    createPreflightPhase = "choosingDest";
    const base = normalizedInputs.length === 1 ? archiveStemName(pathBaseName(normalizedInputs[0])) : "archive";
    let dest: string | null;
    try {
      const { save } = await getDialogModule();
      dest = await saveNativeDialog("create.save-archive", save, {
        title: source === "drop" ? tr("gui.create.save_dropped_items_as_archive", "Save dropped items as archive") : tr("gui.create.save_archive_as", "Save archive as"),
        defaultPath: createSaveDefaultPath(normalizedInputs[0], base),
        filters: createSaveFilters(),
      });
    } catch {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.save_dialog_requires_desktop_dialog", "Save dialog requires the desktop file dialog"));
      return;
    }
    if (!dest) {
      createPreflightPhase = "ready";
      showNotice(tr("gui.create.cancelled", "Create archive cancelled"));
      return;
    }
    lastCreateDest = dest;
    let disk: DiskSpaceDto;
    try {
      createPreflightPhase = "checkingDest";
      disk = await ipc.checkDiskSpace(dest, requiredCreateDiskBytes(estimate));
    } catch {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.destination_preflight_requires_desktop_service", "Destination disk preflight requires the desktop service"));
      return;
    }
    lastDiskSpace = disk;
    if (!disk.ok) {
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.not_enough_destination_space", "Not enough free space in destination · {available} available").replace("{available}", formatBytes(disk.available_bytes)));
      return;
    }

    createPreflightPhase = "submitting";
    try {
      await submitJob({
        kind: "compress",
        inputs: normalizedInputs,
        dest,
        level: createCompressionLevel(),
        password: null,
        encrypt_names: false,
        split_size: null,
        excludes,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) {
        createPreflightPhase = "ready";
        return;
      }
      createPreflightPhase = "blocked";
      showNotice(tr("gui.create.submission_requires_desktop_service", "Create archive submission requires the desktop service"));
      return;
    }
    createPreflightPhase = "ready";
    showNotice(tr("gui.create.queued_notice", "Create archive started · {size} input").replace("{size}", formatBytes(estimate.total_bytes)));
    recordOperation({
      status: "queued",
      title: source === "drop" ? tr("gui.create.dropped_items_queued", "Dropped items started") : tr("gui.create.queued", "Create archive started"),
      detail: tr("gui.create.operation_detail", "{name} · {profile} · {size} input")
        .replace("{name}", pathBaseName(dest))
        .replace("{profile}", createProfileLabel(activeCreateProfile))
        .replace("{size}", formatBytes(estimate.total_bytes)),
    });
  }

  function blockingTask(): Task | null {
    return activeCurrentTask ?? jobRows.find((task) => isTaskActiveState(task.state)) ?? null;
  }

  function submittingTaskModel(): TaskDialogModel | null {
    if (!submittingJobSpec) return null;
    return {
      id: null,
      spec: submittingJobSpec,
      title: titleForJobSpec(submittingJobSpec),
      state: "submitting",
      done: 0,
      total: 0,
      current: "",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
      error: null,
      result: null,
      revealPath: null,
      historyRecorded: false,
      controlIntent: null,
      expanded: true,
    };
  }

  function currentTaskStatusLabel(): string {
    const failed = jobRows.find((task) => task.state === "failed");
    const task = activeCurrentTask ?? failed ?? null;
    if (!task) return tr("gui.state.ready", "Ready");
    return `${titleForJobSpec(task.spec)} · ${taskStateLabel(task.state)}`;
  }

  function taskDialogTask(): TaskDialogModel | null {
    const submitting = submittingTaskModel();
    if (submitting) return submitting;
    if (taskDialogTaskId !== null) {
      const remembered = jobRows.find((task) => task.id === taskDialogTaskId);
      if (remembered) return remembered;
    }
    return blockingTask();
  }

  function taskDialogVisible(): boolean {
    const task = taskDialogTask();
    if (!task) return false;
    if (isTaskActiveState(task.state)) return true;
    return taskDialogDismissedId !== task.id;
  }

  function openTaskDialog(task: Task | null = blockingTask()): void {
    if (!task) return;
    taskDialogTaskId = task.id;
    taskDialogDismissedId = null;
  }

  function focusBlockingTaskIfAny(): boolean {
    if (jobSubmitInFlight) {
      if (import.meta.env.DEV && params.has("validationTrace")) {
        const win = window as ValidationWindow;
        win.__squallzValidationJobSubmitBlockedWhileStarting = (win.__squallzValidationJobSubmitBlockedWhileStarting ?? 0) + 1;
      }
      showNotice(
        tr(
          "gui.task.starting_notice",
          "Task is starting. Wait for the progress window before starting another operation",
        ),
      );
      return true;
    }
    const active = blockingTask();
    if (!active) return false;
    openTaskDialog(active);
    showNotice(
      tr(
        "gui.task.one_at_a_time_notice",
        "Finish or cancel the current task before starting another one",
      ),
    );
    return true;
  }

  async function dismissTaskDialog(task: TaskDialogModel): Promise<void> {
    if (task.id === null) return;
    if (isTaskActiveState(task.state)) return;
    if (taskWindowMode && await closeNativeTaskWindow()) return;
    taskDialogDismissedId = task.id;
    taskDialogTaskId = null;
  }

  function isJobSubmitBlocked(error: unknown): boolean {
    return error instanceof JobSubmitBlockedError;
  }

  async function submitJob(spec: JobSpec): Promise<number> {
    if (focusBlockingTaskIfAny()) {
      throw new JobSubmitBlockedError();
    }
    jobSubmitInFlight = true;
    submittingJobSpec = spec;
    taskDialogTaskId = null;
    taskDialogDismissedId = null;
    try {
      if (import.meta.env.DEV && params.has("validationTrace")) {
        const win = window as ValidationWindow;
        win.__squallzValidationJobSubmitAttempts = (win.__squallzValidationJobSubmitAttempts ?? 0) + 1;
      }
      if (import.meta.env.DEV && runtimePreviews.jobSubmitDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, runtimePreviews.jobSubmitDelayMs));
      }
      const id = await submitArchiveJob(spec);
      taskDialogTaskId = id;
      taskDialogDismissedId = null;
      return id;
    } finally {
      jobSubmitInFlight = false;
      submittingJobSpec = null;
    }
  }

  function cancelCurrentTask(task: TaskDialogModel): void {
    if (task.id === null) return;
    cancelTask(task.id);
    showNotice(tr("gui.task.cancel_requested", "Cancel requested"));
  }

  function pauseCurrentTask(task: TaskDialogModel): void {
    if (task.id === null) return;
    pauseTask(task.id);
    showNotice(tr("gui.task.pause_requested", "Pause requested"));
  }

  function resumeCurrentTask(task: TaskDialogModel): void {
    if (task.id === null) return;
    resumeTask(task.id);
    showNotice(tr("gui.task.resume_requested", "Resume requested"));
  }

  function taskRevealOutputLabel(): string {
    return t("gui.task.show_in_file_manager", { fileManager: fileManagerLabel() });
  }

  function viewTaskResults(task: TaskDialogModel): void {
    const target = taskResultScreen(task);
    if (!target) return;
    if (taskWindowMode) {
      if (task.id !== null) setTaskExpanded(task.id, true);
      return;
    }
    setScreen(target);
    void dismissTaskDialog(task);
    if (target === "checksum") {
      void focusChecksumResultPanel(task.spec.kind === "checksum_check" ? "checksum_check" : "checksum");
    }
  }

  async function openTaskOutput(task: TaskDialogModel): Promise<void> {
    const outputPath = taskOutputPath(task);
    if (!outputPath) return;
    try {
      const { openPath } = await import("@tauri-apps/plugin-opener");
      await openPath(outputPath);
      showNotice(
        taskOutputIsFolder(task)
          ? tr("gui.task.output_folder_opened", "Output folder opened")
          : tr("gui.task.output_opened", "Output opened"),
      );
    } catch {
      showNotice(tr("gui.task.open_output_failed", "Cannot open the task output"));
    }
  }

  async function revealTaskOutput(task: TaskDialogModel): Promise<void> {
    if (!task.revealPath) return;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(task.revealPath);
    } catch {
      showNotice(tr("gui.task.reveal_failed", "Cannot reveal the task output"));
    }
  }

  function passwordPromptName(): string {
    return jobPasswordPrompt?.name ?? currentArchive?.name ?? tr("gui.password.no_prompt", "No password prompt");
  }

  function passwordPromptDetail(): string {
    if (jobPasswordPrompt?.wrong) return tr("gui.password.previous_rejected", "Previous password was rejected. Try again or cancel this job.");
    return jobPasswordPrompt
      ? tr("gui.password.task_paused", "This task is waiting for the archive password.")
      : tr("gui.password.no_prompt_pending", "No password request is active.");
  }

  function submitJobPassword() {
    if (!jobPasswordPrompt) {
      showNotice(tr("gui.password.no_prompt_pending", "No password request is active."));
      return;
    }
    answerJobPassword(jobPasswordValue || null);
    jobPasswordValue = "";
    showNotice(tr("gui.password.sent_to_task", "Password sent to task"));
  }

  function cancelJobPassword() {
    if (jobPasswordPrompt) {
      answerJobPassword(null);
      showNotice(tr("gui.password.prompt_cancelled", "Password prompt cancelled"));
    }
    jobPasswordValue = "";
    setScreen("extract");
  }

  function conflictRowsView() {
    if (!jobConflictPrompt) return [];
    return [
      {
        path: jobConflictPrompt.incoming_path,
        existing: `${formatBytes(jobConflictPrompt.existing_size)} · ${formatModified(jobConflictPrompt.existing_modified)}`,
        incoming: `${formatBytes(jobConflictPrompt.incoming_size)} · ${formatModified(jobConflictPrompt.incoming_modified)}`,
        decision: tr("gui.conflict.choose", "Choose"),
      },
    ];
  }

  function conflictPromptTitle(): string {
    if (jobConflictPrompt) return tr("gui.conflict.one_item_exists", "1 item already exists");
    return tr("gui.conflict.no_prompt", "No conflict prompt");
  }

  function conflictPromptDetail(): string {
    if (jobConflictPrompt) return tr("gui.conflict.task_paused", "This task is waiting for your conflict choice.");
    return tr("gui.conflict.real_job_pauses_on_overwrite", "Extract tasks pause here only when a file conflict needs your choice.");
  }

  function latestRecoveryReportTask(): Task | null {
    for (let index = jobRows.length - 1; index >= 0; index -= 1) {
      const task = jobRows[index];
      if (
        task.result &&
        (task.spec.kind === "protect" ||
          task.spec.kind === "verify_recovery" ||
          task.spec.kind === "repair_recovery")
      ) {
        return task;
      }
    }
    return null;
  }

  function recoveryReport(): Record<string, unknown> | null {
    return latestRecoveryReportTask()?.result ?? null;
  }

  function recoveryMetrics(): Record<string, unknown> | null {
    const metrics = recoveryReport()?.metrics;
    return metrics && typeof metrics === "object" && !Array.isArray(metrics)
      ? metrics as Record<string, unknown>
      : null;
  }

  function recoveryMetricNumber(key: string): number {
    const value = recoveryMetrics()?.[key];
    return typeof value === "number" && Number.isFinite(value) ? value : 0;
  }

  function recoveryMetricBoolean(key: string): boolean | null {
    const value = recoveryMetrics()?.[key];
    return typeof value === "boolean" ? value : null;
  }

  function recoveryResultAvailable(): boolean {
    return recoveryReport() !== null;
  }

  function recoveryFailureAvailable(): boolean {
    const repairPossible = recoveryMetricBoolean("repair_possible");
    const allCorrect = recoveryMetricBoolean("all_correct");
    if (repairPossible === false) return true;
    if (allCorrect === false && repairPossible === null) return true;
    return false;
  }

  function recoveryResultTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (!recoveryResultAvailable()) return tr("gui.recovery.not_verified", "Not verified");
    if (recoveryMetricBoolean("no_damage")) return tr("gui.recovery.no_damage", "No damage");
    return recoveryFailureAvailable()
      ? tr("gui.recovery.not_repairable", "Not repairable")
      : tr("gui.recovery.repairable", "Repairable");
  }

  function recoveryResultDetail(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (!recoveryResultAvailable()) return tr("gui.recovery.run_verify_capacity", "Run Verify with PAR2 before reporting repair capacity.");
    return tr("gui.recovery.capacity_summary", "{needed} blocks needed · {available} recovery blocks available")
      .replace("{needed}", recoveryMetricNumber("blocks_needed").toLocaleString())
      .replace("{available}", recoveryMetricNumber("recovery_blocks_available").toLocaleString());
  }

  function recoveryResultFooter(): string {
    if (!currentArchive) return tr("gui.recovery.open_before_verify", "Open an archive before verifying recovery data");
    if (!recoveryResultAvailable()) return tr("gui.recovery.no_verification_result", "No verification result yet");
    const repaired = recoveryMetricNumber("blocks_repaired");
    return repaired > 0
      ? tr("gui.recovery.blocks_repaired", "{count} blocks repaired").replace("{count}", repaired.toLocaleString())
      : taskStateLabel(latestRecoveryReportTask()?.state);
  }

  function recoveryBlocksView() {
    return recoveryResultAvailable() ? recoveryBlocks : [];
  }

  function answerConflictDecision(decision: "skip" | "overwrite" | "rename", applyAll: boolean) {
    if (!jobConflictPrompt) {
      showNotice(tr("gui.conflict.no_prompt_pending", "No conflict request is active"));
      return;
    }
    answerJobConflict(decision, applyAll);
    showNotice(
      applyAll
        ? tr("gui.conflict.decision_applied_remaining", "Conflict decision applied to remaining files")
        : tr("gui.conflict.decision_sent", "Conflict decision sent to task"),
    );
    setScreen("extract");
  }

  function currentArchiveName(): string {
    return currentArchive?.name ?? noArchiveLabel();
  }

  function passwordBookSecretStoreLabel(): string {
    return passwordBookStatus.available
      ? tr("gui.settings.password_book.available", "Available")
      : tr("gui.settings.password_book.unavailable_status", "Unavailable");
  }

  function passwordBookCurrentLabel(): string {
    if (!currentArchive) return noArchiveLabel();
    return passwordBookStatus.saved
      ? tr("gui.settings.password_book.saved_status", "Saved")
      : tr("gui.settings.password_book.not_saved_status", "Not saved");
  }

  function passwordBookDetailLabel(): string {
    if (!currentArchive) return tr("gui.settings.password_book.open_archive_to_check", "Open an archive to check saved password status");
    return passwordBookStatus.saved
      ? tr("gui.settings.password_book.current_has_saved_entry", "Current archive has a saved secret-store entry")
      : tr("gui.settings.password_book.prompt_or_save_after_unlock", "Prompt or save after unlocking this archive");
  }

  async function refreshPasswordBookPanel() {
    if (!currentArchive) {
      showNotice(tr("gui.settings.password_book.open_archive_before_checking", "Open an archive before checking Password Book status"));
      return;
    }

    try {
      await refreshArchivePasswordBookStatus(currentArchive.path);
      showNotice(tr("gui.settings.password_book.status_refreshed", "Password Book status refreshed"));
    } catch {
      showNotice(tr("gui.settings.password_book.status_unavailable_preview", "Password Book status unavailable in this preview"));
    }
  }

  async function forgetPasswordBookPanel() {
    if (!currentArchive) {
      showNotice(tr("gui.settings.password_book.no_open_password_to_forget", "No open archive password to forget"));
      return;
    }

    const ok = await forgetCurrentArchivePassword();
    showNotice(ok
      ? tr("gui.settings.password_book.current_password_forgotten", "Current archive password forgotten")
      : tr("gui.settings.password_book.could_not_forget_current", "Could not forget current archive password"));
  }

  function showNotice(message: string) {
    appNotice = message;
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => {
      appNotice = null;
      noticeTimer = null;
    }, 2600);
  }

  function showChecksumCopyFeedback(
    kind: "checksum" | "checksum_check" | "task",
    taskId: number | null,
    message: string,
    tone: "success" | "danger",
  ) {
    checksumCopyFeedbackKind = kind;
    checksumCopyFeedbackTaskId = taskId;
    checksumCopyFeedbackMessage = message;
    checksumCopyFeedbackTone = tone;
    if (checksumCopyFeedbackTimer) clearTimeout(checksumCopyFeedbackTimer);
    checksumCopyFeedbackTimer = setTimeout(() => {
      checksumCopyFeedbackKind = null;
      checksumCopyFeedbackTaskId = null;
      checksumCopyFeedbackMessage = null;
      checksumCopyFeedbackTone = null;
      checksumCopyFeedbackTimer = null;
    }, 2600);
  }

  function checksumCopyFeedbackFor(kind: "checksum" | "checksum_check"): string | null {
    return checksumCopyFeedbackKind === kind ? checksumCopyFeedbackMessage : null;
  }

  function checksumCopyFeedbackToneFor(kind: "checksum" | "checksum_check"): "success" | "danger" | null {
    return checksumCopyFeedbackKind === kind ? checksumCopyFeedbackTone : null;
  }

  function taskChecksumCopyFeedback(task: TaskDialogModel): string | null {
    return checksumCopyFeedbackKind === "task" && checksumCopyFeedbackTaskId === task.id
      ? checksumCopyFeedbackMessage
      : null;
  }

  function taskChecksumCopyFeedbackTone(task: TaskDialogModel): "success" | "danger" | null {
    return checksumCopyFeedbackKind === "task" && checksumCopyFeedbackTaskId === task.id
      ? checksumCopyFeedbackTone
      : null;
  }

  function integrationApplyLabel(): string {
    if (integrationStatus === "applying") return tr("gui.settings.integration.installing", "Installing...");
    if (integrationInstalledCount > 0) return tr("gui.settings.integration.reinstall_actions", "Reinstall actions");
    return tr("gui.settings.integration.install_platform_actions", "Install {fileManager} actions").replace("{fileManager}", fileManagerLabel());
  }

  function integrationSummaryLabel(): string {
    if (integrationInstalledCount === 0) {
      return tr("gui.settings.integration.platform_actions_not_installed", "{fileManager} actions not installed in this session")
        .replace("{fileManager}", fileManagerLabel());
    }
    return tr("gui.settings.integration.platform_actions_installed_count", "{count} {fileManager} actions installed")
      .replace("{fileManager}", fileManagerLabel())
      .replace("{count}", String(integrationInstalledCount));
  }

  function integrationDetailLabel(): string {
    if (integrationServicesDir) return integrationServicesDir;
    return tr("gui.settings.integration.install_detail", "Installs Checksum, Extract Here, Extract to Folder, Compress to 7Z, and Test Archive.");
  }

  function applyIntegrationStatusSnapshot(result: IntegrationStatusDto | IntegrationApplyResultDto) {
    integrationInstalledCount = result.installed.length;
    integrationServicesDir = result.services_dir || null;
    integrationScriptDir = result.script_dir || null;
    integrationStatus = result.installed.length > 0 ? "installed" : "idle";
  }

  async function applyIntegrationChanges() {
    integrationStatus = "applying";
    try {
      const result = await ipc.applyIntegrationChanges();
      integrationResult = result;
      applyIntegrationStatusSnapshot(result);
      recordOperation({
        status: result.installed.length > 0 ? "done" : "info",
        title: tr("gui.settings.integration.applied_title", "Desktop integrations applied"),
        detail: result.installed.length > 0
          ? tr("gui.settings.integration.actions_with_folder", "{count} actions · {folder}")
            .replace("{count}", String(result.installed.length))
            .replace("{folder}", pathBaseName(result.services_dir))
          : (result.unsupported[0] ?? tr("gui.settings.integration.none_installed_platform", "No integrations installed on this platform")),
      });
      showNotice(
        result.installed.length > 0
          ? tr("gui.settings.integration.installed_platform_count", "Installed {count} {fileManager} actions")
            .replace("{count}", String(result.installed.length))
            .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.not_installed_yet", "This platform integration is not installed yet"),
      );
    } catch {
      integrationStatus = "blocked";
      showNotice(tr("gui.settings.integration.requires_desktop_service", "{fileManager} integration requires the desktop service").replace("{fileManager}", fileManagerLabel()));
    }
  }

  async function refreshIntegrationStatus() {
    try {
      const result = await ipc.getIntegrationStatus();
      applyIntegrationStatusSnapshot(result);
      showNotice(
        result.installed.length > 0
          ? tr("gui.settings.integration.platform_actions_installed_count", "{count} {fileManager} actions installed")
            .replace("{count}", String(result.installed.length))
            .replace("{fileManager}", fileManagerLabel())
          : tr("gui.settings.integration.platform_actions_not_installed_short", "{fileManager} actions are not installed").replace("{fileManager}", fileManagerLabel()),
      );
    } catch {
      integrationStatus = "blocked";
      showNotice(tr("gui.settings.integration.status_requires_desktop_service", "{fileManager} integration status requires the desktop service").replace("{fileManager}", fileManagerLabel()));
    }
  }

  async function removeIntegrationChanges() {
    integrationStatus = "applying";
    try {
      const result: IntegrationRemoveResultDto = await ipc.removeIntegrationChanges();
      integrationResult = null;
      integrationInstalledCount = 0;
      integrationServicesDir = result.services_dir || null;
      integrationScriptDir = result.script_dir || null;
      integrationStatus = "idle";
      recordOperation({
        status: "done",
        title: tr("gui.settings.integration.removed_title", "Desktop integrations removed"),
        detail: tr("gui.settings.integration.removed_count", "{count} actions removed").replace("{count}", String(result.removed.length)),
      });
      showNotice(
        tr("gui.settings.integration.removed_platform_count", "Removed {count} {fileManager} actions")
          .replace("{count}", String(result.removed.length))
          .replace("{fileManager}", fileManagerLabel()),
      );
    } catch {
      integrationStatus = "blocked";
      showNotice(tr("gui.settings.integration.removal_requires_desktop_service", "{fileManager} integration removal requires the desktop service").replace("{fileManager}", fileManagerLabel()));
    }
  }

  async function showNativeWindow() {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const appWindow = getCurrentWindow();
      await appWindow.show();
      await appWindow.unminimize();
      await appWindow.setFocus();
    } catch {
      // Dev preview has no native Tauri window to show.
    }
  }

  async function closeNativeTaskWindow(): Promise<boolean> {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().close();
      return true;
    } catch {
      // Dev preview has no native Tauri window to close.
      return false;
    }
  }

  function screenForNav(label: string): Screen {
    if (label === "Recent") return "recent";
    if (label === "Create") return "create";
    if (label === "Extract") return "extract";
    if (label === "Convert") return "convert";
    if (label === "Checksum") return "checksum";
    if (label === "Duplicates") return "duplicates";
    if (label === "Recovery") return "recovery";
    if (label === "Settings") return "settingsGeneral";
    if (label === "Appearance") return "appearance";
    return "browse";
  }

  function screenForCommand(label: string): Screen {
    if (label === "Add") return "create";
    if (label === "Extract To") return "extract";
    if (label === "Protect" || label === "Test") return "recovery";
    if (label === "Checksum") return "checksum";
    if (label === "Duplicates") return "duplicates";
    if (label === "Convert") return "convert";
    if (label === "Info") return "archiveInfo";
    return "browse";
  }

  function handleClassicCommand(label: string) {
    if (label === "Add") {
      void submitAddToArchiveJob();
      return;
    }
    if (label === "Extract To") {
      void submitExtractJob();
      return;
    }
    if (label === "Test") {
      void submitTestJob();
      return;
    }
    if (label === "View") {
      void submitPreviewEntry();
      return;
    }
    if (label === "Delete") {
      void submitDeleteSelectedJob();
      return;
    }
    if (label === "Rename") {
      void submitRenameSelectedJob();
      return;
    }
    if (label === "Move") {
      void submitMoveSelectedJob();
      return;
    }
    if (label === "New Folder") {
      void submitNewFolderJob();
      return;
    }
    if (label === "Checksum") {
      setScreen("checksum");
      return;
    }
    if (label === "Duplicates") {
      setScreen("duplicates");
      return;
    }
    if (label === "Convert") {
      void submitConvertJob();
      return;
    }
    if (label === "Info") {
      setScreen("archiveInfo");
      return;
    }
    setScreen(screenForCommand(label));
  }

  function classicCommandDisabled(label: string): boolean {
    if (label === "Checksum" || label === "Duplicates" || label === "Info") return false;
    if (label === "Rename") return !canRenameSelection();
    if (label === "Move" || label === "Delete") return !hasArchiveSelection();
    if (label === "View") return !canPreviewEntrySelection();
    return !hasArchiveOpen();
  }

  function classicCommandDisabledTitle(label: string): string {
    if (!classicCommandDisabled(label)) return "";
    if (!currentArchive) {
      if (label === "Add") return tr("gui.precondition.open_before_add", "Open an archive before adding files");
      if (label === "Extract To") return tr("gui.precondition.open_before_extract", "Open an archive before extracting");
      if (label === "Test") return tr("gui.precondition.open_before_test", "Open an archive before testing");
      if (label === "Protect") return tr("gui.precondition.open_before_protect", "Open an archive before protecting");
      if (label === "View") return tr("gui.preview.open_archive_first", "Open an archive before previewing entries");
      if (label === "Delete") return tr("gui.precondition.open_before_delete", "Open an archive before deleting entries");
      if (label === "Rename") return tr("gui.precondition.open_before_rename", "Open an archive before renaming entries");
      if (label === "Move") return tr("gui.precondition.open_before_move", "Open an archive before moving entries");
      if (label === "New Folder") return tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder");
      if (label === "Convert") return convertArchiveRequiredReason();
      return openArchiveFirstLabel();
    }
    if (label === "Rename") return tr("gui.precondition.select_one_before_rename", "Select exactly one file entry before renaming");
    if (label === "Move") return tr("gui.precondition.select_entries_before_move", "Select entries before moving");
    if (label === "Delete") return tr("gui.precondition.select_entries_before_delete", "Select entries before deleting");
    if (label === "View") return tr("gui.preview.select_one", "Select one file entry to preview");
    return "";
  }

  function classicCommandTitle(label: string): string {
    const disabledTitle = classicCommandDisabledTitle(label);
    if (disabledTitle) return disabledTitle;
    if (label === "Extract To") return extractDestinationHint();
    if (label === "Info") return tr("gui.archive.info_title", "Show archive information");
    return "";
  }

  function classicCommandAriaLabel(label: string): string {
    return labelWithDisabledReason(classicCommandLabel(label), classicCommandDisabledTitle(label));
  }

  function isSettingsScreen(value: Screen = screen): boolean {
    return (
      value === "appearance" ||
      value === "colors" ||
      value === "settingsGeneral" ||
      value === "settingsSecurity" ||
      value === "settingsPerformance" ||
      value === "passwordBook" ||
      value === "integration"
    );
  }

  function titleForScreen() {
    if (screen === "recent") return tr("gui.screen.recent", "Recent Archives");
    if (screen === "create") return tr("gui.screen.create", "Create Archive");
    if (screen === "extract") return tr("gui.screen.extract", "Extract");
    if (screen === "convert") return tr("gui.screen.convert", "Convert Archive");
    if (screen === "batch") return tr("gui.screen.batch", "Batch Extract Review");
    if (screen === "checksum") return tr("gui.screen.checksum", "Checksum");
    if (screen === "duplicates") return tr("gui.screen.duplicates", "Duplicate Finder");
    if (screen === "password") return tr("gui.screen.password", "Password Required");
    if (screen === "conflict") return tr("gui.screen.conflict", "Conflict Handling");
    if (screen === "cannotRepair") return tr("gui.screen.cannot_repair", "Recovery Limit");
    if (screen === "recovery") return tr("gui.screen.recovery", "Recovery");
    if (screen === "archiveInfo") return tr("gui.screen.archive_info", "Archive Info");
    if (screen === "integration") return tr("gui.screen.integration", "File Associations");
    if (screen === "appearance") return tr("gui.screen.appearance", "Appearance");
    if (screen === "colors") return tr("gui.screen.colors", "Appearance · Theme Colors");
    if (screen === "settingsGeneral") return tr("gui.screen.settings_general", "Settings · General");
    if (screen === "settingsSecurity") return tr("gui.screen.settings_security", "Settings · Security");
    if (screen === "settingsPerformance") return tr("gui.screen.settings_performance", "Settings · Performance");
    if (screen === "passwordBook") return tr("gui.screen.password_book", "Settings · Password Book");
    return archiveTitle();
  }
</script>

{#if appNotice}
  <div class={`app-notice themed-root palette-${activePalette} theme-${activeTheme}`} style={customPaletteStyle()} role="status">{appNotice}</div>
{/if}

{#if taskDialogVisible()}
  {@const task = taskDialogTask()}
  {#if task}
    <TaskProgressDialog
      {task}
      rootClass={`task-modal-overlay design-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`}
      rootStyle={customPaletteStyle()}
      copyFeedback={taskChecksumCopyFeedback(task)}
      copyFeedbackTone={taskChecksumCopyFeedbackTone(task)}
      {taskOutputPath}
      {taskRevealOutputLabel}
      {taskWindowMode}
      onPause={pauseCurrentTask}
      onResume={resumeCurrentTask}
      onCancel={cancelCurrentTask}
      onCopyChecksumResults={copyTaskChecksumResults}
      onOpenOutput={openTaskOutput}
      onViewResults={viewTaskResults}
      onRevealOutput={revealTaskOutput}
      onDismiss={dismissTaskDialog}
    />
  {/if}
{/if}

{#if dragActive || lastDropKind !== "none"}
  <div class={`drop-status themed-root palette-${activePalette} theme-${activeTheme}`} style={customPaletteStyle()} class:active={dragActive} role="status">{dropStatusLabel()}</div>
{/if}

{#if entryContext}
  <div
    bind:this={entryContextMenu}
    class={`entry-context-menu themed-root palette-${activePalette} theme-${activeTheme}`}
    style={`${customPaletteStyle()}; left: ${entryContext.x}px; top: ${entryContext.y}px;`}
    role="menu"
    aria-label={tr("gui.context.actions_for", "Actions for {name}").replace("{name}", entryContext.name)}
  >
    <div class="entry-context-head">
      <span>{tr("gui.context.selection_actions", "Selection actions")}</span>
      <strong>{entryContext.name}</strong>
    </div>
    <button role="menuitem" disabled={!currentArchive} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void runEntryContextAction("extract")}><Icon name="archive" size={15} />{actionLabel("Extract selected")}</button>
    <button role="menuitem" disabled={!hasArchiveSelection()} title={hasArchiveSelection() ? "" : tr("gui.precondition.select_entries", "Select entries first")} onclick={() => void runEntryContextAction("delete")}><Icon name="x-circle" size={15} />{actionLabel("Delete selected")}</button>
    <button role="menuitem" disabled={!entryContext.canRename || !canRenameSelection()} title={entryContext.canRename && canRenameSelection() ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")} onclick={() => void runEntryContextAction("rename")}><Icon name="repeat" size={15} />{actionLabel("Rename selected")}</button>
    <button role="menuitem" disabled={!hasArchiveSelection()} title={hasArchiveSelection() ? "" : tr("gui.precondition.select_entries", "Select entries first")} onclick={() => void runEntryContextAction("move")}><Icon name="repeat" size={15} />{actionLabel("Move selected")}</button>
    <button role="menuitem" disabled={!entryContext.path} title={entryContext.path ? previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file") : tr("gui.preview.select_one", "Select one file entry to preview")} onclick={() => void runEntryContextAction("preview")}><Icon name={entryContext.isDir ? "folder-open" : "eye"} size={15} />{previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file")}</button>
    <button role="menuitem" disabled={!currentArchive} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void runEntryContextAction("test")}><Icon name="shield-alert" size={15} />{actionLabel("Test archive")}</button>
  </div>
{/if}

{#if firstRunRequired && !taskWindowMode}
  <section class={`first-run-overlay themed-root palette-${activePalette} theme-${activeTheme}`} style={customPaletteStyle()} aria-label={tr("gui.first_run.aria", "Choose Squallz interface mode")}>
    <div class="first-run-panel">
      <div class="first-run-brand">
        <AppIcon size={46} title="Squallz" />
        <div>
          <span class="eyebrow">Squallz</span>
          <h1>{tr("gui.first_run.title", "Choose your interface")}</h1>
          <p>{tr("gui.first_run.body", "Pick the surface that matches how you work. You can switch later in Settings without losing archives, jobs, or settings.")}</p>
        </div>
      </div>

      <div class="first-run-choices">
        <button class="first-run-card recommended" onclick={() => setMode("modern")}>
          <span class="mode-kicker">{tr("gui.first_run.recommended", "Recommended")}</span>
          <strong>{tr("gui.mode.modern", "Modern")}</strong>
          <small>{tr("gui.first_run.modern_body", "Calm macOS utility, guided archive tasks, inspector-first safety.")}</small>
          <div class="mode-preview modern-preview">
            <i></i><span></span><span></span><b></b>
          </div>
        </button>
        <button class="first-run-card" onclick={() => setMode("classic")}>
          <span class="mode-kicker">{tr("gui.first_run.power_workflow", "Power workflow")}</span>
          <strong>{tr("gui.mode.classic", "Classic")}</strong>
          <small>{tr("gui.first_run.classic_body", "Dense command bar, detailed archive table, keyboard-friendly operations.")}</small>
          <div class="mode-preview classic-preview">
            <i></i><i></i><i></i><span></span><span></span><span></span>
          </div>
        </button>
      </div>

      <footer class="first-run-footer">
        <span>{settingsStatus === "loading" ? tr("gui.first_run.checking_saved_settings", "Checking saved settings") : tr("gui.first_run.mode_changed_settings", "Mode can be changed in Settings")}</span>
        <button onclick={() => setScreen("settingsGeneral")}>{tr("gui.first_run.review_settings", "Review Settings")}</button>
      </footer>
    </div>
  </section>
{/if}

{#if taskWindowMode}
  <main
    class={`design-root task-window-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`}
    style={customPaletteStyle()}
    aria-label={tr("gui.external_task.window_label", "Squallz task window")}
  >
    {#if !taskDialogVisible()}
      <section class="task-window-empty" role="status">
        <AppIcon size={42} title="Squallz" />
        <div>
          <span class="eyebrow">{tr("gui.external_task.eyebrow", "Task window")}</span>
          <h1>{taskWindowShellTitleCopy}</h1>
          <p>{taskWindowShellCopy}</p>
        </div>
      </section>
    {/if}
  </main>
{:else if mode === "modern" || isSettingsScreen()}
  <main class={`design-root modern-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`} style={customPaletteStyle()} class:drop-active={dragActive}>
    <section class="window modern-window" aria-label={isSettingsScreen() ? tr("gui.aria.squallz_settings", "Squallz settings") : tr("gui.aria.modern_archive_browser", "Squallz Modern archive browser")}>
      <header class="modern-titlebar" data-tauri-drag-region>
        <div class="brand-lockup">
          <div class="brand-glyph"><AppIcon size={36} title="Squallz" /></div>
          <div>
            <strong>Squallz</strong>
            <span>{titleForScreen()}</span>
          </div>
        </div>
        <div class="modern-toolbar" aria-label={tr("gui.aria.primary_actions", "Primary actions")}>
          {#if isSettingsScreen()}
            <button onclick={() => setScreen("browse")}><Icon name="archive" size={16} />{tr("gui.settings.back_to_archives", "Archives")}</button>
          {:else}
		          <button aria-busy={archiveOpenStatus === "opening"} onclick={() => void openArchiveFromDialog()}><Icon name="folder-open" size={16} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
		          <button onclick={() => setScreen("create")}><Icon name="sparkles" size={16} />{toolbarLabel("Create")}</button>
		          <button disabled={!currentArchive} title={archiveActionTitle(hasArchiveOpen())} onclick={() => setScreen("recovery")}><Icon name="shield-alert" size={16} />{toolbarLabel("Protect")}</button>
		          <button class="primary" disabled={!currentArchive} title={archiveActionTitle(hasArchiveOpen())} onclick={() => setScreen("extract")}><Icon name="archive" size={16} />{toolbarLabel("Extract")}</button>
          <button
            bind:this={quickActionButton}
            class="icon-only"
            aria-label={tr("gui.quick.title", "Quick actions")}
            aria-haspopup="dialog"
            aria-expanded={activePopover === "quickActions"}
            onclick={toggleQuickActions}
          ><Icon name="search" size={16} /></button>
          {/if}
        </div>
      </header>

      {#if activePopover === "quickActions"}
        <div bind:this={quickActionPopover} class="quick-popover modern-quick-popover" role="dialog" aria-label={tr("gui.quick.title", "Quick actions")}>
          <div class="quick-popover-head">
            <strong>{tr("gui.quick.title", "Quick actions")}</strong>
            <span>{tr("gui.quick.subtitle", "Jump without changing layout")}</span>
          </div>
          {#each quickActions as action}
            <button onclick={() => chooseQuickAction(action.screen)}>
              <Icon name={action.icon} size={15} />
              <span><strong>{quickActionLabel(action.label)}</strong><small>{quickActionDetail(action.label, action.detail)}</small></span>
            </button>
          {/each}
        </div>
      {/if}

      <div
        class="modern-shell"
        class:settings-shell={isSettingsScreen()}
        class:no-archive-shell={screen === "browse" && !currentArchive}
        class:no-inspector-shell={screen === "recent" || screen === "convert"}
      >
        <aside class="modern-sidebar" aria-label={tr("gui.aria.navigation", "Navigation")}>
          <div class="sidebar-section">
            {#each nav as item}
              <button
                class:current={(screen === "recent" && item[1] === "Recent") || (screen === "browse" && item[1] === "Archives") || (screen === "create" && item[1] === "Create") || ((screen === "extract" || screen === "batch" || screen === "password" || screen === "conflict") && item[1] === "Extract") || (screen === "convert" && item[1] === "Convert") || (screen === "checksum" && item[1] === "Checksum") || (screen === "duplicates" && item[1] === "Duplicates") || ((screen === "recovery" || screen === "cannotRepair") && item[1] === "Recovery") || (isSettingsScreen() && item[1] === "Settings")}
                onclick={() => setScreen(screenForNav(item[1]))}
              >
                <Icon name={item[0]} size={16} />
	                <span>{navLabel(item[1])}</span>
              </button>
            {/each}
          </div>
	          {#if !hideOperationHistory}
	            <div class="recent-card history-card">
	              <span>{tr("gui.history.title", "Operation history")}</span>
	              <strong>{historySummaryCount()}</strong>
	              <small>{historyLastLabel()}</small>
	            </div>
	          {/if}
        </aside>

        <section class="modern-content" class:settings-workspace={isSettingsScreen()} class:browse-workspace={screen === "browse"} aria-label={tr("gui.aria.archive_contents", "Archive contents")}>
          {#if isSettingsScreen()}
            <aside class="settings-workspace-rail" aria-label={tr("gui.settings.sections", "Settings sections")}>
              <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.title", "Settings")}</div>
              <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
            </aside>
          {/if}
          {#if showArchiveReturnBar()}
            <ArchiveReturnStrip
              title={archiveTitle()}
              detail={archiveReturnDetail()}
              contextLabel={tr("gui.archive.current_context", "Current archive")}
              actionLabel={tr("gui.archive.back_to_current", "Back to current archive")}
              onReturn={returnToCurrentArchive}
            />
          {/if}
          {#if screen === "recent"}
            <div class="create-sheet modern-recent">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.recent.eyebrow", "Workspace / Recent")}</span>
                  <h1>{tr("gui.recent.title", "Recent archives")}</h1>
                  <p>{tr("gui.recent.subtitle", "Reopen local archives you used recently. Squallz keeps only the file paths, never archive contents.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void openArchiveFromDialog()}><Icon name="folder-open" size={17} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
              </div>

              <div class="create-grid">
                <section class="create-main-panel">
                  <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.recent.list_title", "Recent files")}</div>
                  <div class="limits-table recent-table">
                    <div><b>{tr("common.name", "Name")}</b><b>{tr("common.path", "Path")}</b><b>{tr("common.action", "Action")}</b></div>
                    {#each recentFiles() as path}
                      <div><span>{pathBaseName(path) || path}</span><span>{path}</span><button onclick={() => void openArchivePath(path, "dialog")}>{tr("gui.recent.reopen", "Reopen")}</button></div>
                    {:else}
                      <div><span>{tr("gui.recent.none", "No recent archives")}</span><span>{tr("gui.recent.open_to_start", "Open an archive to start this list.")}</span><button onclick={() => void openArchiveFromDialog()}>{toolbarLabel("Open")}</button></div>
                    {/each}
                  </div>
                </section>
                <aside class="create-side-panel">
                  <section>
                    <div class="panel-title"><Icon name="check-circle" size={16} />{tr("gui.recent.current_title", "Current archive")}</div>
                    <strong>{currentArchive ? currentArchive.name : noArchiveLabel()}</strong>
                    <p>{currentArchive ? currentArchive.path : openArchiveFirstLabel()}</p>
                  </section>
                  <section>
                    <div class="panel-title"><Icon name="shield-alert" size={16} />{tr("gui.recent.privacy_title", "Privacy boundary")}</div>
                    <p>{tr("gui.recent.privacy_body", "Recent files are stored locally as paths for quick reopening. Passwords and archive contents are not stored here.")}</p>
                  </section>
                </aside>
              </div>
            </div>
          {:else if screen === "convert"}
            <div class="create-sheet modern-convert">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.convert.eyebrow", "Archive / Convert")}</span>
                  <h1>{tr("gui.convert.title", "Convert archive")}</h1>
                  <p>{tr("gui.convert.subtitle", "Convert the current archive to the opposite launch format without extracting files into a temporary folder.")}</p>
                </div>
                <button
                  class="primary sheet-action"
                  disabled={Boolean(convertArchiveRequiredReason())}
                  title={convertArchiveRequiredReason()}
                  aria-label={currentArchive ? tr("gui.convert.start", "Convert") : labelWithDisabledReason(tr("gui.convert.start", "Convert"), convertArchiveRequiredReason())}
                  onclick={() => void submitConvertJob()}
                ><Icon name="repeat" size={17} />{tr("gui.convert.start", "Convert")}</button>
              </div>

              <div class="create-grid">
                <section class="create-main-panel">
                  <div class="field-label">{tr("gui.convert.source", "Source")}</div>
                  <div class="path-preview">{currentArchive ? currentArchive.path : openArchiveFirstLabel()}</div>
                  <div class="field-label">{tr("gui.convert.destination", "Destination")}</div>
                  <div class="path-preview">{defaultConvertDest()}</div>
                  <div class="settings-metric-grid">
                    <div><span>{tr("gui.convert.source_format", "Source format")}</span><strong>{currentArchive ? currentArchive.format.toUpperCase() : "-"}</strong><small>{currentArchive ? archiveSummary() : openArchiveFirstLabel()}</small></div>
                    <div><span>{tr("gui.convert.target_format", "Target format")}</span><strong>{defaultConvertTargetFormat()}</strong><small>{tr("gui.convert.auto_target_hint", "ZIP sources convert to 7Z; other sources convert to ZIP.")}</small></div>
                    <div><span>{tr("gui.convert.profile", "Compression profile")}</span><strong>{createProfileLabel(activeCreateProfile)}</strong><small>{activeCreateProfileDetail()}</small></div>
                  </div>
                  <div class="setting-callout">
                    <strong>{tr("gui.convert.contract_title", "Conversion scope")}</strong>
                    <span>{tr("gui.convert.contract_body", "The task runs through the same archive engine path as the CLI convert command; source archives are not modified.")}</span>
                  </div>
                </section>
                <aside class="create-side-panel">
                  <section>
                    <div class="panel-title"><Icon name="check-circle" size={16} />{tr("gui.convert.readiness", "Readiness")}</div>
                    <strong>{currentArchive ? tr("gui.state.ready", "Ready") : openArchiveFirstLabel()}</strong>
                    <p>{currentArchive ? tr("gui.convert.ready_body", "Destination is derived next to the source archive. Review it before starting.") : tr("gui.convert.open_archive_first_body", "Open an archive before converting.")}</p>
                  </section>
                  <section>
                    <div class="panel-title"><Icon name="shield-alert" size={16} />{tr("gui.settings.security.guard", "Guard")}</div>
                    <p>{tr("gui.convert.guard_body", "Passwords, encoding overrides, and archive safety checks stay explicit; conversion never implies repair data.")}</p>
                  </section>
                </aside>
              </div>
            </div>
          {:else if screen === "create"}
            <div class="create-sheet modern-create">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.create.eyebrow", "Create archive")}</span>
                  <h1>{tr("gui.create.real_preflight_title", "Create with real preflight")}</h1>
                  <p>{tr("gui.create.real_preflight_body", "Choose files or a folder after editing rules; Squallz measures real input bytes before queuing the archive.")}</p>
                </div>
                <div class="sheet-action-row">
                  <button class="primary sheet-action" disabled={createPreflightBusy()} onclick={() => void submitCreateJob("files")}><Icon name="archive" size={17} />{createPreflightBusy() ? tr("gui.create.checking", "Checking") : tr("gui.create.choose_files", "Choose files")}</button>
                  <button class="sheet-action" disabled={createPreflightBusy()} onclick={() => void submitCreateJob("folder")}><Icon name="folder-open" size={17} />{createPreflightBusy() ? tr("gui.create.checking", "Checking") : tr("gui.checksum.choose_folder", "Choose folder")}</button>
                </div>
              </div>
              {#if createDropInputs.length > 0}
                <div class="drop-summary">
                  <Icon name="archive" size={16} />
                  <span>{tr("gui.create.dropped_sources", "Dropped sources")}</span>
                  <strong>{droppedSourceLabel()}</strong>
                </div>
              {/if}

              <div class="create-grid">
                <section class="create-main-panel">
                  <div class="field-label">{tr("gui.create.archive_name", "Archive name")}</div>
                  <div class="input-shell">{createArchivePreviewName()}</div>

                  <div class="field-label">{tr("gui.batch.destination", "Destination")}</div>
                  <div class="path-preview">{createArchivePreviewPath()}</div>

                  <div class="preset-row" aria-label={tr("gui.create.compression_presets", "Compression presets")}>
                    {#each createProfileIds as profileId}
                      <button
                        class:selected={activeCreateProfile === profileId}
                        aria-pressed={activeCreateProfile === profileId}
                        onclick={() => chooseCreateProfile(profileId)}
                      >{createProfileLabel(profileId)}</button>
                    {/each}
                  </div>

                  <div class="format-segments" aria-label={tr("gui.create.archive_format", "Archive format")}>
                    {#each createFormatIds as formatId}
                      <button
                        class:selected={activeCreateFormat === formatId}
                        aria-pressed={activeCreateFormat === formatId}
                        title={createFormatNoteFor(formatId)}
                        onclick={() => chooseCreateFormat(formatId)}
                      >{createFormats[formatId].label}</button>
                    {/each}
                    <span class="format-boundary-pill" role="note" title={tr("gui.create.rar_not_launch_claim", "RAR creation is not a launch claim")}>{tr("gui.create.rar_read_only", "RAR read only")}</span>
                  </div>
                  <div class="format-note">{createFormatNote()}</div>

                  <div class="level-control">
                    <div><strong>{tr("gui.create.compression_level", "Compression level {level}").replace("{level}", String(createCompressionLevel()))}</strong><span>{activeCreateProfileDetail()}.</span></div>
                    {#if activeCreateProfile === "custom"}
                      <div class="custom-level-row">
                        <input
                          class="custom-level-range"
                          type="range"
                          min="1"
                          max="9"
                          value={customCreateLevel}
                          aria-label={tr("gui.create.custom_compression_level", "Custom compression level")}
                          oninput={(event) => updateCustomCreateLevelFromInput(event)}
                          onchange={(event) => updateCustomCreateLevelFromInput(event, true)}
                        />
                        <input
                          class="custom-level-number"
                          class:invalid={customCreateLevelError.length > 0}
                          type="number"
                          min="1"
                          max="9"
                          step="1"
                          inputmode="numeric"
                          value={customCreateLevel}
                          aria-label={tr("gui.create.custom_compression_level_number", "Custom compression level number")}
                          aria-invalid={customCreateLevelError ? "true" : "false"}
                          aria-describedby={customCreateLevelError ? "custom-create-level-error-modern" : undefined}
                          oninput={(event) => updateCustomCreateLevelFromInput(event)}
                          onchange={(event) => updateCustomCreateLevelFromInput(event, true)}
                        />
                      </div>
                      {#if customCreateLevelError}
                        <small id="custom-create-level-error-modern" class="custom-level-error" role="status" data-custom-level-error>{customCreateLevelError}</small>
                      {/if}
                      <div class="custom-profile-panel">
                        <label class="custom-profile-name">
                          <span>{tr("common.name", "Name")}</span>
                          <input
                            aria-label={tr("gui.create.custom_profile_name", "Custom profile name")}
                            class:invalid={customCreateProfileNameError.length > 0}
                            value={customCreateProfileName}
                            aria-invalid={customCreateProfileNameError ? "true" : "false"}
                            aria-describedby={customCreateProfileNameError ? "custom-create-profile-name-error-modern" : undefined}
                            oninput={updateCustomCreateProfileNameFromInput}
                          />
                        </label>
                        {#if customCreateProfileNameError}
                          <small id="custom-create-profile-name-error-modern" class="custom-profile-name-error" role="status" data-custom-profile-name-error>{customCreateProfileNameError}</small>
                        {/if}
                        <div class="custom-profile-list" aria-label={tr("gui.create.saved_custom_profiles", "Saved custom profiles")}>
                          {#each customCreateProfiles as profile}
                            <button
                              class:active={profile.id === activeCustomCreateProfileId}
                              aria-pressed={profile.id === activeCustomCreateProfileId}
                              onclick={() => chooseCustomCreateProfile(profile.id)}
                            ><strong>{profile.name}</strong><span>L{profile.level}</span></button>
                          {/each}
                        </div>
                        <div class="custom-profile-actions">
                          <button onclick={saveActiveCustomCreateProfile}>{tr("gui.create.save_profile", "Save profile")}</button>
                          <button
                            onclick={createNewCustomCreateProfile}
                            disabled={customCreateProfiles.length >= maxCustomCreateProfiles}
                            title={customProfileSaveAsNewTitle()}
                            aria-label={`${tr("gui.create.save_as_new", "Save as new")}${customProfileSaveAsNewTitle() ? ` · ${customProfileSaveAsNewTitle()}` : ""}`}
                          >{tr("gui.create.save_as_new", "Save as new")}</button>
                          <button onclick={deleteActiveCustomCreateProfile} disabled={customCreateProfiles.length <= 1} title={customProfileDeleteTitle()}>{tr("common.delete", "Delete")}</button>
                        </div>
                        {#if customCreateProfiles.length >= maxCustomCreateProfiles}
                          <small class="custom-profile-limit" role="status">{customProfileLimitMessage()}</small>
                        {/if}
                      </div>
                    {/if}
                  </div>

                  <ExcludeRulesEditor
                    title={tr("gui.excludes.title", "Excludes")}
                    hint={tr("gui.excludes.create_hint", "One glob, folder, or extension per line.")}
                    countLabel={tr("gui.excludes.count", "{count} rules").replace("{count}", String(createExcludeRules().length))}
                    value={createExcludeText}
                    placeholder={tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
                    ariaLabel={tr("gui.create.exclude_glob_rules", "Exclude glob rules")}
                    rules={createExcludeRules()}
                    emptyLabel={tr("gui.create.no_rules", "No rules")}
                    onInput={(value) => (createExcludeText = value)}
                  />

                  <div class="create-preflight-strip" aria-label={tr("gui.create.preflight_status", "Create preflight status")}>
                    <div><span>{tr("gui.create.input_preflight", "Input preflight")}</span><strong>{createEstimateStatusbar()}</strong></div>
                    <div><span>{tr("gui.create.temp_preflight", "Temp preflight")}</span><strong>{tempPreflightStatusbar()}</strong></div>
                    <div><span>{tr("gui.create.disk_preflight", "Disk preflight")}</span><strong>{diskPreflightStatusbar()}</strong></div>
                  </div>
                  <div class={`create-preflight-progress phase-${createPreflightPhase}`}>
                    <span>{createPreflightPhaseLabel()}</span>
                    <progress
                      class="create-preflight-meter"
                      value={createPreflightPercent()}
                      max="100"
                      aria-label={tr("gui.create.preflight_status", "Create preflight status")}
                    ></progress>
                  </div>

                  <div class="recovery-callout">
                    <div>
                      <span class="block-label">{tr("gui.recovery.title", "Recovery")}</span>
                      <strong>{createRecoveryCapability()}</strong>
                      <p>{tr("gui.create.recovery_separate_jobs", "Creating the archive and generating recovery data are separate jobs; use Recovery when you want PAR2 or SQZ repair evidence.")}</p>
                    </div>
                    <button onclick={openRecoveryConfiguration}>{tr("common.change", "Change")}</button>
                  </div>
                </section>

                <aside class="create-side-panel">
                  <section>
                    <h2><Icon name="lock" size={16} />{tr("gui.create.password", "Password")}</h2>
                    <div class="input-shell" class:password={createPasswordDataAvailable()}>{createPasswordDataAvailable() ? "••••••••••••" : tr("gui.create.format_capability", "Format capability")}</div>
                    <div class="check-row" data-capability="password-data-encryption"><span class="fake-check" class:on={createPasswordDataAvailable()}></span>{createPasswordCapability()}</div>
                    <div
                      class="check-row"
                      class:disabled={!createNameEncryptionAvailable()}
                      data-capability="name-encryption"
                      title={createNameEncryptionCapability()}
                      aria-label={`${tr("gui.create.name_encryption", "Name encryption")}: ${createNameEncryptionCapability()}`}
                    >
                      <span class="fake-check" class:on={createNameEncryptionAvailable()}></span>{tr("gui.create.name_encryption", "Name encryption")} · {createNameEncryptionCapability()}
                    </div>
                    <small>{tr("gui.create.password_support_body", "Password support follows the selected archive format capability.")}</small>
                  </section>

                  <section>
                    <h2><Icon name="panel-top" size={16} />{tr("gui.create.split_volumes", "Split volumes")}</h2>
                    <div class="input-shell">{createSplitCapability()}</div>
                    <div class="volume-preview">{createVolumePreview()}</div>
                  </section>

                  <section>
                    <h2><Icon name="shield-alert" size={16} />{tr("gui.recovery.title", "Recovery")}</h2>
                    <div class="input-shell">{createRecoveryCapability()}</div>
                    <div class="volume-preview recovery-preview">{tr("gui.create.recovery_explicit_body", "Recovery jobs remain explicit so the app never implies uncreated repair data.")}</div>
                  </section>

                  <section>
                    <h2><Icon name="list" size={16} />{tr("gui.format.coverage.title", "Format coverage")}</h2>
                    <div class="format-coverage-list" aria-label={tr("gui.format.coverage.summary_aria", "Format coverage summary")}>
                      {#each formatCoverageRows() as row}
                        <div>
                          <span>{row.label}</span>
                          <strong>{row.value}</strong>
                          <small>{row.detail}</small>
                        </div>
                      {/each}
                    </div>
                    <small>{tr("gui.format.coverage.create_limited", "{source} · create controls stay limited to release-claimed writable formats.").replace("{source}", formatRegistrySourceLabel())}</small>
                  </section>

                </aside>
              </div>
            </div>
          {:else if screen === "extract"}
            <div class="extract-view modern-extract">
              <div class="sheet-head">
                <div>
	                  <span class="eyebrow">{tr("gui.extract.eyebrow", "Extract")}</span>
	                  <h1>{tr("gui.extract.safe_title", "Extract selected files safely")}</h1>
	                  <p>{tr("gui.extract.safe_subtitle", "Destination preview, smart folder behavior, conflicts, passwords, and safety limits are visible before the job starts.")}</p>
                </div>
                <button
                  class="primary sheet-action"
                  disabled={Boolean(extractArchiveRequiredReason())}
                  title={currentArchive ? extractDestinationHint() : extractArchiveRequiredReason()}
                  aria-label={currentArchive ? actionLabel("Extract selected") : labelWithDisabledReason(actionLabel("Extract selected"), extractArchiveRequiredReason())}
                  onclick={() => void submitExtractJob()}
                ><Icon name="archive" size={17} />{actionLabel("Extract selected")}</button>
              </div>

              <div class="extract-layout">
                <section class="extract-main-panel">
                  <div class="path-decision">
	                    <span class="block-label">{tr("gui.extract.final_destination", "Final destination")}</span>
	                    <strong>{effectiveExtractDest()}</strong>
	                    <p>{tr("gui.extract.snapshot_hint", "Smart extract captures the current safety settings when the job starts.")}</p>
                  </div>
                  <div class="destination-grid">
	                    {#each extractDestinationModes as mode}
	                      <button
                          class:selected={extractDestinationMode === mode}
                          disabled={Boolean(extractArchiveRequiredReason())}
                          title={extractArchiveRequiredReason()}
                          aria-label={labelWithDisabledReason(extractDestinationTitle(mode), extractArchiveRequiredReason())}
                          onclick={() => void selectExtractDestination(mode)}
                        >
	                        <strong>{extractDestinationTitle(mode)}</strong>
	                        <span>{extractDestinationDetail(mode)}</span>
	                      </button>
	                    {/each}
                  </div>
                  <div class="extract-options-grid">
	                    <div><span>{tr("common.selection", "Selection")}</span><strong>{extractSelectionLabel()}</strong></div>
	                    <div><span>{tr("gui.extract.conflict_policy", "Conflict policy")}</span><strong>{extractOverwriteLabel()}</strong></div>
	                    <div><span>{tr("gui.extract.password", "Password")}</span><strong>{extractPasswordLabel()}</strong></div>
	                    <div><span>{tr("gui.extract.safety", "Safety")}</span><strong>{tr("gui.extract.safety_guards_on", "Zip Slip + bomb guards on")}</strong></div>
                  </div>
                  <div class="extract-policy-grid" aria-label={tr("gui.extract.conflict_policy", "Conflict policy")}>
                    {#each extractOverwriteModes as mode}
                      <button
                        class:selected={extractOverwriteMode === mode}
                        aria-pressed={extractOverwriteMode === mode}
                        onclick={() => selectExtractOverwrite(mode)}
                      >{extractOverwriteLabel(mode)}</button>
                    {/each}
                  </div>
                  <div class="extract-flow-actions">
	                    <button
                        disabled={Boolean(extractArchiveRequiredReason())}
                        title={extractArchiveRequiredReason()}
                        aria-label={labelWithDisabledReason(tr("gui.extract.review_batch", "Review batch extract"), extractArchiveRequiredReason())}
                        onclick={() => setScreen("batch")}
                      ><Icon name="list" size={16} />{tr("gui.extract.review_batch", "Review batch extract")}</button>
	                    <button
                        disabled={Boolean(extractArchiveRequiredReason())}
                        title={extractArchiveRequiredReason()}
                        aria-label={labelWithDisabledReason(tr("gui.extract.password_prompt", "Password prompt"), extractArchiveRequiredReason())}
                        onclick={() => setScreen("password")}
                      ><Icon name="lock" size={16} />{tr("gui.extract.password_prompt", "Password prompt")}</button>
	                    <button
                        disabled={Boolean(extractArchiveRequiredReason())}
                        title={extractArchiveRequiredReason()}
                        aria-label={labelWithDisabledReason(tr("gui.extract.conflict_preview", "Conflict preview"), extractArchiveRequiredReason())}
                        onclick={() => setScreen("conflict")}
                      ><Icon name="alert-triangle" size={16} />{tr("gui.extract.conflict_preview", "Conflict preview")}</button>
                  </div>
                </section>

                <aside class="extract-side-panel">
                  <section>
	                    <span class="block-label">{tr("gui.inspector.archive", "Archive")}</span>
                    <strong>{archiveTitle()}</strong>
                    <p>{archiveLine()} · {extractSelectionLabel()}</p>
                  </section>
                  <section>
	                    <span class="block-label">{tr("gui.archive.encoding", "Encoding")}</span>
                    <strong>{extractEncodingLabel()}</strong>
                    <p>{currentArchive ? archiveWarningText() : openArchiveFirstLabel()}</p>
                  </section>
                  <section>
	                    <span class="block-label">{tr("gui.extract.blocked_conditions", "Blocked conditions")}</span>
	                    <p>{tr("gui.extract.blocked_conditions_body", "Path traversal, case collision, reserved Windows names, and symlink escapes stop the job before writing.")}</p>
                  </section>
                </aside>
              </div>
            </div>
          {:else if screen === "batch"}
            <div class="batch-view modern-batch">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.batch.review", "Batch extract review")}</span>
                  <h1>{tr("gui.batch.review_count_title", "Review {count} archives before extraction").replace("{count}", String(batchReviewArchives().length))}</h1>
                  <p>{tr("gui.batch.review_subtitle", "Every target folder is previewed before work starts. Password or volume issues block only the affected archive.")}</p>
                </div>
                <button class="primary sheet-action" disabled={batchReviewArchives().length === 0} title={batchReviewArchives().length === 0 ? openArchiveFirstLabel() : ""} onclick={() => void startBatchExtract()}><Icon name="archive" size={17} />{tr("gui.batch.start_batch", "Start batch")}</button>
              </div>
              <div class="batch-summary-strip">
                <div><span>{tr("gui.batch.target_rule", "Target rule")}</span><strong>{tr("gui.batch.each_archive_folder", "Each archive folder")}</strong></div>
                <div><span>{tr("gui.extract.smart_mode", "Smart extract")}</span><strong>{tr("common.on", "On")}</strong></div>
                <div><span>{tr("gui.extract.conflicts", "Conflicts")}</span><strong>{tr("gui.batch.ask_before_replace", "Ask before replace")}</strong></div>
                <div><span>{tr("gui.batch.warnings", "Warnings")}</span><strong>{batchWarningLabel()}</strong></div>
              </div>
              <div class="batch-card-list">
                {#each batchReviewArchives() as archive}
                  <section class:warning={archive.state === "Needs password"} class="batch-card">
                    <div>
                      <strong>{archive.name}</strong>
                      <span>{archive.format} · {tr("gui.archive.entry_count", "{count} entries").replace("{count}", archive.entries)}</span>
                    </div>
                    <div><span>{tr("common.target", "Target")}</span><strong>{archive.target}</strong></div>
                    <em>{batchArchiveStateLabel(archive.state)}</em>
                  </section>
                {:else}
                  <section class="batch-card">
                    <div>
                      <strong>{openArchiveFirstLabel()}</strong>
                      <span>{tr("gui.batch.no_archives_queued", "No archives selected")}</span>
                    </div>
                    <div><span>{tr("common.target", "Target")}</span><strong>-</strong></div>
                    <em>{tr("gui.task.idle", "No task running")}</em>
                  </section>
                {/each}
              </div>
            </div>
          {:else if screen === "checksum"}
            <div class="settings-view modern-checksum">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.checksum.eyebrow", "Tools / Checksum")}</span>
                  <h1>{tr("gui.checksum.title", "Verify files without changing them")}</h1>
                  <p>{tr("gui.checksum.modern_subtitle", "Compute SHA-256, SHA-512, SHA-1, MD5, BLAKE3, or CRC32 with the shared core engine, or verify a checksum manifest.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void submitChecksumJob()}><Icon name="check-circle" size={17} />{tr("gui.checksum.calculate", "Calculate checksum")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="settings-metric-grid">
                    <div><span>{tr("common.target", "Target")}</span><strong>{checksumTargetName()}</strong><small>{checksumTargetLabel()}</small></div>
                    <div><span>{tr("gui.checksum.algorithm", "Algorithm")}</span><strong>{checksumAlgorithmLabel(checksumAlgorithm)}</strong><small>{tr("gui.checksum.matches_cli_algorithm", "Matches sqz checksum --algorithm")}</small></div>
                    <div><span>{tr("gui.checksum.latest_hashed", "Latest hashed")}</span><strong>{checksumResultNumber("checksum", "files_hashed").toLocaleString()}</strong><small>{formatBytes(checksumResultNumber("checksum", "bytes_hashed"))}</small></div>
                    <div><span>{tr("gui.checksum.manifest_check", "Manifest check")}</span><strong>{checksumResultNumber("checksum_check", "passed").toLocaleString()} / {checksumResultNumber("checksum_check", "checked").toLocaleString()}</strong><small>{tr("gui.checksum.failed_count", "{count} failed").replace("{count}", checksumResultNumber("checksum_check", "failed").toLocaleString())}</small></div>
                  </div>

                  <div class="level-control settings-slider">
                    <div><strong>{tr("gui.checksum.target", "Checksum target")}</strong><span>{checksumTargetLabel()}</span></div>
                    <div class="path-preview">{checksumTargetLabel()}</div>
                    <div class="settings-actions-row">
                      <button class="primary-lite" onclick={() => void chooseChecksumFile()}><Icon name="folder-open" size={15} />{tr("gui.checksum.choose_file", "Choose file")}</button>
                      <button onclick={() => void chooseChecksumFolder()}>{tr("gui.checksum.choose_folder", "Choose folder")}</button>
                      <button onclick={useCurrentArchiveForChecksum}>{tr("gui.checksum.use_current_archive", "Use current archive")}</button>
                      <button class="primary-lite" onclick={() => void submitChecksumJob()}><Icon name="check-circle" size={15} />{tr("gui.checksum.calculate_now", "Calculate now")}</button>
                    </div>
                    <div class="algorithm-field worker-field" role="group" aria-label={tr("gui.checksum.algorithm", "Algorithm")}>
                      <span>{tr("gui.checksum.algorithm", "Algorithm")}</span>
                      <ChecksumAlgorithmPicker
                        algorithms={checksumAlgorithms}
                        selected={checksumAlgorithm}
                        labelFor={checksumAlgorithmLabel}
                        hintFor={checksumAlgorithmHint}
                        onSelect={selectChecksumAlgorithm}
                      />
                    </div>
                  </div>

                  <ExcludeRulesEditor
                    title={tr("gui.excludes.title", "Excludes")}
                    hint={tr("gui.excludes.folder_scan_hint", "Applied only when the target is a folder.")}
                    countLabel={tr("gui.excludes.count", "{count} rules").replace("{count}", String(checksumExcludeRules().length))}
                    value={checksumExcludeText}
                    placeholder={tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
                    ariaLabel={tr("gui.checksum.exclude_rules", "Checksum exclude rules")}
                    rules={checksumExcludeRules()}
                    emptyLabel={tr("gui.create.no_rules", "No rules")}
                    onInput={(value) => (checksumExcludeText = value)}
                  />

                  <div class="level-control settings-slider checksum-manifest-card">
                    <div><strong>{tr("gui.checksum.manifest_verification", "Manifest verification")}</strong><span>{checksumManifestLabel()}</span></div>
                    <div class="path-preview">{checksumManifestLabel()}</div>
                    <div class="settings-actions-row checksum-manifest-actions">
                      <button class="primary-lite" onclick={() => void chooseChecksumManifest()}><Icon name="folder-open" size={15} />{tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
                      <button class="primary-lite" onclick={() => void submitChecksumCheckJob()}><Icon name="check-circle" size={15} />{tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
                    </div>
                  </div>

                  <section
                    class="checksum-result-panel"
                    bind:this={checksumResultPanel}
                    tabindex="-1"
                    aria-label={tr("gui.checksum.result", "Checksum result")}
                  >
                    <div class="checksum-result-actions">
                      <div class="checksum-result-title">
                        <strong>{tr("gui.checksum.result", "Checksum result")}</strong>
                        <span>{tr("gui.checksum.result_rows", "{count} rows").replace("{count}", checksumItems("checksum").length.toLocaleString())}</span>
                      </div>
                      <div class="checksum-result-copy">
                        {#if checksumCopyFeedbackFor("checksum")}
                          <span class="checksum-copy-status" class:danger={checksumCopyFeedbackToneFor("checksum") === "danger"} role="status">{checksumCopyFeedbackFor("checksum")}</span>
                        {/if}
                        <button type="button" class="primary-lite" disabled={checksumItems("checksum").length === 0} onclick={() => void copyChecksumResults("checksum")}><Icon name="list" size={14} />{tr("gui.checksum.copy_results", "Copy results")}</button>
                      </div>
                    </div>
                    <div class="limits-table checksum-result-table">
                      <div><b>{tr("gui.checksum.result", "Checksum result")}</b><b>{tr("gui.table.size", "Size")}</b><b>{tr("gui.checksum.digest", "Digest")}</b><b>{tr("common.status", "Status")}</b></div>
                      {#each checksumItems("checksum").slice(0, 20) as item}
                        <div><span>{pathBaseName(checksumItemText(item, "path")) || checksumItemText(item, "path")}</span><span>{formatBytes(checksumItemNumber(item, "size"))}</span><code class="checksum-digest">{checksumItemText(item, "digest")}</code><strong>{checksumItemStatus(item)}</strong></div>
                      {:else}
                        <div><span>{tr("gui.checksum.no_result_yet", "No checksum result yet")}</span><span>-</span><span>-</span><strong>{taskStateLabel(latestChecksumTask("checksum")?.state)}</strong></div>
                      {/each}
                    </div>
                  </section>

                  <section
                    class="checksum-result-panel"
                    bind:this={checksumCheckResultPanel}
                    tabindex="-1"
                    aria-label={tr("gui.checksum.verification_result", "Verification result")}
                  >
                    <div class="checksum-result-actions">
                      <div class="checksum-result-title">
                        <strong>{tr("gui.checksum.verification_result", "Verification result")}</strong>
                        <span>{tr("gui.checksum.result_rows", "{count} rows").replace("{count}", checksumItems("checksum_check").length.toLocaleString())}</span>
                      </div>
                      <div class="checksum-result-copy">
                        {#if checksumCopyFeedbackFor("checksum_check")}
                          <span class="checksum-copy-status" class:danger={checksumCopyFeedbackToneFor("checksum_check") === "danger"} role="status">{checksumCopyFeedbackFor("checksum_check")}</span>
                        {/if}
                        <button type="button" class="primary-lite" disabled={checksumItems("checksum_check").length === 0} onclick={() => void copyChecksumResults("checksum_check")}><Icon name="list" size={14} />{tr("gui.checksum.copy_results", "Copy results")}</button>
                      </div>
                    </div>
                    <div class="limits-table checksum-result-table checksum-verify-table">
                      <div><b>{tr("gui.checksum.verification_result", "Verification result")}</b><b>{tr("gui.checksum.expected", "Expected")}</b><b>{tr("gui.checksum.actual", "Actual")}</b><b>{tr("common.status", "Status")}</b></div>
                      {#each checksumItems("checksum_check").slice(0, 20) as item}
                        <div><span>{pathBaseName(checksumItemText(item, "path")) || checksumItemText(item, "path")}</span><code class="checksum-digest">{checksumItemText(item, "expected")}</code><code class="checksum-digest">{checksumItemText(item, "actual") || checksumItemText(item, "error")}</code><strong>{checksumItemStatus(item)}</strong></div>
                      {:else}
                        <div><span>{tr("gui.checksum.no_manifest_result_yet", "No manifest result yet")}</span><span>-</span><span>-</span><strong>{taskStateLabel(latestChecksumTask("checksum_check")?.state)}</strong></div>
                      {/each}
                    </div>
                  </section>
                </section>

                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="check-circle" size={16} />{tr("gui.checksum.verification_contract", "Verification scope")}</div>
                  <div class="setting-callout">
                    <strong>{tr("gui.checksum.shared_with_cli", "Shared with CLI")}</strong>
                    <span>{tr("gui.checksum.cli_contract_body", "This page maps to sqz checksum and sqz checksum --check; JSON result fields stay aligned with automation output.")}</span>
                  </div>
                  <div class="settings-route-list">
                    <button class="settings-route-card" onclick={() => setScreen("duplicates")}>
                      <Icon name="search" size={16} />
                      <span><strong>{tr("gui.screen.duplicates", "Duplicate Finder")}</strong><small>{tr("gui.duplicates.route_from_checksum", "Find identical local files with BLAKE3")}</small></span>
                    </button>
                    <button class="settings-route-card" onclick={() => setScreen("recovery")}>
                      <Icon name="shield-alert" size={16} />
                      <span><strong>{tr("gui.recovery.title", "Recovery")}</strong><small>{tr("gui.recovery.route_from_checksum", "Test, protect, repair, and export archives")}</small></span>
                    </button>
                  </div>
                </aside>
              </div>
            </div>
          {:else if screen === "duplicates"}
            <div class="settings-view modern-duplicates">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.duplicates.eyebrow", "Tools / Duplicate Finder")}</span>
                  <h1>{tr("gui.duplicates.title", "Find duplicate local files")}</h1>
                  <p>{tr("gui.duplicates.modern_subtitle", "BLAKE3 hashes are computed by the shared core engine; this scan never deletes, moves, links, or modifies files.")}</p>
                  <div class="duplicate-safety-strip" aria-label={tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
                    <span><Icon name="search" size={14} />{tr("gui.duplicates.cli_contract", "CLI parity: sqz duplicates")}</span>
                    <span><Icon name="list" size={14} />{tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
                    <span><Icon name="check-circle" size={14} />{tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
                  </div>
                </div>
                <button class="primary sheet-action" onclick={() => void submitDuplicateScanJob()}><Icon name="search" size={17} />{tr("gui.duplicates.scan", "Scan duplicates")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="settings-metric-grid">
                    <div><span>{tr("common.target", "Target")}</span><strong>{duplicateScanTargetName()}</strong><small>{duplicateScanTargetLabel()}</small></div>
                    <div><span>{tr("gui.duplicates.minimum_size", "Minimum size")}</span><strong>{formatBytes(duplicateMinSize)}</strong><small>{tr("gui.duplicates.smaller_ignored", "Smaller files are ignored before hashing")}</small></div>
                    <div><span>{tr("gui.duplicates.latest_groups", "Latest groups")}</span><strong>{duplicateResultNumber("duplicate_groups").toLocaleString()}</strong><small>{tr("gui.duplicates.duplicate_files_count", "{count} duplicate files").replace("{count}", duplicateResultNumber("duplicate_files").toLocaleString())}</small></div>
                    <div><span>{tr("gui.duplicates.reclaimable", "Reclaimable")}</span><strong>{formatBytes(duplicateResultNumber("reclaimable_bytes"))}</strong><small>{tr("gui.duplicates.potential_space", "Potential space if one copy per group remains")}</small></div>
                  </div>

                  <div class="level-control settings-slider">
                    <div><strong>{tr("gui.duplicates.scan_target", "Scan target")}</strong><span>{duplicateScanTargetLabel()}</span></div>
                    <div class="path-preview">{duplicateScanTargetLabel()}</div>
                    <div class="settings-actions-row">
                      <button class="primary-lite" onclick={() => void chooseDuplicateScanFolder()}><Icon name="folder-open" size={15} />{tr("gui.checksum.choose_folder", "Choose folder")}</button>
                      <button onclick={useCurrentArchiveFolderForDuplicates}>{tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
                      <button class="primary-lite" onclick={() => void submitDuplicateScanJob()}><Icon name="search" size={15} />{tr("gui.duplicates.scan_now", "Scan now")}</button>
                    </div>
                    <label class="number-field worker-field">
                      <span>{tr("gui.duplicates.minimum_hashed_size_bytes", "Minimum hashed size in bytes")}</span>
                      <input
                        type="number"
                        min="0"
                        step="1"
                        value={duplicateMinSize}
                        class:invalid={duplicateMinSizeError.length > 0}
                        aria-label={tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
                        aria-invalid={duplicateMinSizeError ? "true" : "false"}
                        aria-describedby={duplicateMinSizeError ? "duplicate-min-size-error-modern" : undefined}
                        oninput={updateDuplicateMinSizeFromInput}
                      />
                      {#if duplicateMinSizeError}
                        <small id="duplicate-min-size-error-modern" class="duplicate-min-size-error" role="status" data-duplicate-min-size-error>{duplicateMinSizeError}</small>
                      {/if}
                    </label>
                  </div>

                  <ExcludeRulesEditor
                    title={tr("gui.excludes.title", "Excludes")}
                    hint={tr("gui.excludes.duplicate_hint", "Skip noisy folders before duplicate hashing.")}
                    countLabel={tr("gui.excludes.count", "{count} rules").replace("{count}", String(duplicateExcludeRules().length))}
                    value={duplicateExcludeText}
                    placeholder={tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
                    ariaLabel={tr("gui.duplicates.exclude_rules", "Duplicate scan exclude rules")}
                    rules={duplicateExcludeRules()}
                    emptyLabel={tr("gui.create.no_rules", "No rules")}
                    onInput={(value) => (duplicateExcludeText = value)}
                  />

                  <div class="limits-table">
                    <div><b>{tr("gui.duplicates.result", "Result")}</b><b>{tr("gui.duplicates.count", "Count")}</b><b>{tr("gui.duplicates.bytes", "Bytes")}</b><b>{tr("common.status", "Status")}</b></div>
                    <div><span>{tr("gui.duplicates.files_scanned", "Files scanned")}</span><span>{duplicateResultNumber("files_scanned").toLocaleString()}</span><span>{formatBytes(duplicateResultNumber("bytes_scanned"))}</span><strong>{taskStateLabel(latestDuplicateScanTask()?.state)}</strong></div>
                    <div><span>{tr("gui.duplicates.candidates_hashed", "Candidates hashed")}</span><span>{duplicateResultNumber("candidate_files").toLocaleString()}</span><span>{formatBytes(duplicateResultNumber("hashed_bytes"))}</span><strong>BLAKE3</strong></div>
                    <div><span>{tr("gui.duplicates.duplicate_groups", "Duplicate groups")}</span><span>{duplicateResultNumber("duplicate_groups").toLocaleString()}</span><span>{formatBytes(duplicateResultNumber("reclaimable_bytes"))}</span><strong>{duplicateResultNumber("duplicate_groups") > 0 ? tr("gui.duplicates.review_only", "Review only") : tr("gui.duplicates.clean", "Clean")}</strong></div>
                  </div>
                </section>

                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="search" size={16} />{tr("gui.duplicates.scan_contract", "Safe scan scope")}</div>
                  <div class="setting-callout">
                    <strong>{tr("gui.duplicates.non_destructive", "Reads and marks duplicates only")}</strong>
                    <span>{tr("gui.duplicates.non_destructive_body", "The scan never cleans up, hard-links, deletes, moves, or modifies files automatically.")}</span>
                  </div>
                  <div class="settings-route-list">
                    <button class="settings-route-card" onclick={() => setScreen("create")}>
                      <Icon name="sparkles" size={16} />
                      <span><strong>{navLabel("Create")}</strong><small>{tr("gui.duplicates.route_to_create", "Use the same exclude semantics before compression")}</small></span>
                    </button>
                    <button class="settings-route-card" onclick={() => setScreen("batch")}>
                      <Icon name="list" size={16} />
                      <span><strong>{tr("gui.screen.batch", "Batch")}</strong><small>{tr("gui.duplicates.route_to_batch", "Start archive work after reviewing targets")}</small></span>
                    </button>
                  </div>
                </aside>
              </div>
            </div>
          {:else if screen === "password"}
            <div class="password-view modern-password">
              <div class="sheet-head compact-head">
                <div>
                  <span class="eyebrow">{tr("gui.password.required", "Password required")}</span>
                  <h1>{tr("gui.password.unlock_name", "Unlock {name}").replace("{name}", passwordPromptName())}</h1>
                  <p>{passwordPromptDetail()}</p>
                </div>
              </div>
              {#if jobPasswordPrompt}
                <div class="modal-preview password-sheet">
                  <div class="password-lock"><Icon name="lock" size={24} /></div>
                  <div>
                    <span class="secure-label">{tr("gui.password.password", "Password")}</span>
                    <input
                      class="secure-input"
                      type="password"
                      bind:value={jobPasswordValue}
                      autocomplete="current-password"
                      aria-label={tr("gui.password.archive_password", "Archive password")}
                    />
                  </div>
                  <div class="check-row"><span class="fake-check"></span>{tr("gui.password.session_only_separate_book", "Session only for this job; saved passwords use the separate Password Book flow")}</div>
                  <div class="password-policy">
                    <strong>{tr("gui.password.security_boundary", "Security boundary")}</strong>
                    <span>{tr("gui.password.manual_wins_body", "Manual password wins over saved password. Failed saved passwords fall back to this prompt.")}</span>
                  </div>
                  <div class="modal-actions">
                    <button onclick={cancelJobPassword}>{tr("common.cancel", "Cancel")}</button>
                    <button class="primary-lite" onclick={submitJobPassword}>{tr("gui.password.unlock_continue", "Unlock and continue")}</button>
                  </div>
                </div>
              {:else}
                <div class="modal-preview empty-task-state">
                  <div class="password-lock"><Icon name="lock" size={24} /></div>
                  <div>
                    <strong>{tr("gui.password.no_active_request", "No password request is active")}</strong>
                    <span>{tr("gui.password.no_active_request_body", "Password entry appears only when an extract or test task asks for credentials.")}</span>
                  </div>
                  <div class="modal-actions">
                    <button onclick={() => setScreen("extract")}>{tr("gui.nav.back_to_extract", "Back to Extract")}</button>
                  </div>
                </div>
              {/if}
            </div>
          {:else if screen === "conflict"}
            <div class="conflict-view modern-conflict">
              <div class="sheet-head compact-head">
                <div>
                  <span class="eyebrow">{tr("gui.screen.conflict", "Conflict handling")}</span>
                  <h1>{conflictPromptTitle()}</h1>
                  <p>{conflictPromptDetail()}</p>
                </div>
              </div>
              {#if jobConflictPrompt}
                <div class="conflict-table">
                  <div class="conflict-head"><span>{tr("common.path", "Path")}</span><span>{tr("gui.conflict.existing", "Existing")}</span><span>{tr("gui.conflict.incoming", "Incoming")}</span><span>{tr("gui.conflict.decision", "Decision")}</span></div>
                  {#each conflictRowsView() as row}
                    <div class="conflict-row">
                      <strong>{row.path}</strong><span>{row.existing}</span><span>{row.incoming}</span><span class="decision-pill">{conflictDecisionLabel(row.decision)}</span>
                    </div>
                  {/each}
                </div>
                <div class="conflict-actions">
                  <button onclick={cancelConflictPrompt}>{tr("gui.nav.back", "Back")}</button>
                  <button onclick={() => answerConflictDecision("skip", false)}>{tr("gui.conflict.skip", "Skip")}</button>
                  <button onclick={() => answerConflictDecision("overwrite", false)}>{tr("gui.conflict.overwrite", "Replace")}</button>
                  <button class="primary-lite" onclick={() => answerConflictDecision("rename", true)}>{tr("gui.conflict.keep_both_all", "Keep both for all")}</button>
                </div>
              {:else}
                <div class="modal-preview empty-task-state">
                  <div class="password-lock"><Icon name="file" size={24} /></div>
                  <div>
                    <strong>{tr("gui.conflict.no_active_request", "No conflict request is active")}</strong>
                    <span>{tr("gui.conflict.no_active_request_body", "Conflict choices appear only when an extract task finds an existing file.")}</span>
                  </div>
                  <div class="modal-actions">
                    <button onclick={() => setScreen("extract")}>{tr("gui.nav.back_to_extract", "Back to Extract")}</button>
                  </div>
                </div>
              {/if}
            </div>
          {:else if screen === "cannotRepair"}
            <div class="cannot-repair-view modern-cannot-repair">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.screen.cannot_repair", "Recovery limit")}</span>
                  <h1>{recoveryFailureAvailable() ? tr("gui.recovery.cannot_fully_repair", "This archive cannot be fully repaired") : tr("gui.recovery.no_failed_repair_result", "No failed repair result")}</h1>
                  <p>{recoveryFailureAvailable() ? tr("gui.recovery.explicit_capacity_body", "Squallz must be explicit: available recovery data can repair 24 blocks, but 37 blocks are damaged or missing.") : tr("gui.recovery.verify_before_failed_math", "Run a real recovery verify job before showing failed block math.")}</p>
                </div>
                <button
                  class="sheet-action"
                  disabled={Boolean(recoveryFailureDisabledReason())}
                  title={recoveryFailureDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.extract_readable_files", "Extract readable files"), recoveryFailureDisabledReason())}
                  onclick={() => void submitBestEffortExtractJob()}
                ><Icon name="archive" size={17} />{tr("gui.recovery.extract_readable_files", "Extract readable files")}</button>
              </div>
              <div class="repair-limit-grid">
                <section class="repair-limit-card danger">
                  <span>{tr("gui.recovery.damaged_blocks", "Damaged blocks")}</span>
                  <strong>{recoveryFailureAvailable() ? "37" : "-"}</strong>
                  <p>{recoveryFailureAvailable() ? tr("gui.recovery.detected_two_groups", "Detected across two data groups and one missing volume.") : tr("gui.recovery.no_failed_verify_result", "No failed verify result.")}</p>
                </section>
                <section class="repair-limit-card">
                  <span>{tr("gui.recovery.capacity", "Recovery capacity")}</span>
                  <strong>{recoveryFailureAvailable() ? "24" : "-"}</strong>
                  <p>{recoveryFailureAvailable() ? tr("gui.recovery.par2_capacity_24", "Existing PAR2 sidecar can repair up to 24 blocks.") : tr("gui.recovery.verify_par2_capacity_first", "Verify with PAR2 before reporting capacity.")}</p>
                </section>
                <section class="repair-limit-card">
                  <span>{tr("gui.recovery.next_safe_action", "Next safe action")}</span>
                  <strong>{recoveryFailureAvailable() ? tr("gui.recovery.partial_extract", "Partial extract") : tr("gui.recovery.verify_first", "Verify first")}</strong>
                  <p>{recoveryFailureAvailable() ? tr("gui.recovery.list_readable_no_full_claim", "List readable entries and do not claim full repair.") : tr("gui.recovery.open_and_verify_first", "Open an archive and run recovery verification first.")}</p>
                </section>
              </div>
              <div class="repair-log">
                {#if recoveryFailureAvailable()}
                  <span>{tr("gui.recovery.g1_damage_summary", "G1: 18 damaged, 12 recovery blocks available.")}</span>
                  <span>{tr("gui.recovery.g2_damage_summary", "G2: 19 damaged, 12 recovery blocks available.")}</span>
                  <span>{tr("gui.recovery.full_repair_blocked_safe", "Full repair blocked. No destructive write will start.")}</span>
                {:else}
                  <span>{tr("gui.recovery.no_failure_result_loaded", "No recovery failure result is loaded.")}</span>
                  <span>{tr("gui.recovery.no_destructive_write_from_state", "No destructive write can start from this state.")}</span>
                {/if}
              </div>
            </div>
          {:else if screen === "recovery"}
            <div class="recovery-view modern-recovery">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.recovery.title", "Recovery")}</span>
                  <h1>{tr("gui.recovery.protect_repair_title", "Protect and repair archives")}</h1>
                  <p>{tr("gui.recovery.protect_repair_body", "PAR2 protects standard archives. ZIP index rebuild handles missing central directories when payloads are intact.")}</p>
                  <div class="recovery-safety-strip" aria-label={tr("gui.recovery.safety_summary", "Recovery action safety summary")}>
                    <span><Icon name="archive" size={14} />{tr("gui.recovery.source_unchanged", "Source archive stays unchanged")}</span>
                    <span><Icon name="shield-alert" size={14} />{tr("gui.recovery.verify_capacity_first", "Verify capacity before repair")}</span>
                    <span><Icon name="check-circle" size={14} />{tr("gui.recovery.requires_existing_data", "Repair requires PAR2 or SQZ data")}</span>
                  </div>
                </div>
                <div class="sheet-action-row">
                  <button
                    class="sheet-action secondary-action"
                    disabled={Boolean(recoveryZipDisabledReason())}
                    title={recoveryZipDisabledReason()}
                    aria-label={labelWithDisabledReason(tr("gui.recovery.repair_zip_index", "Repair ZIP index"), recoveryZipDisabledReason())}
                    onclick={() => void submitRepairZipJob()}
                  ><Icon name="archive" size={17} />{tr("gui.recovery.repair_zip_index", "Repair ZIP index")}</button>
                  <button
                    class="sheet-action secondary-action"
                    disabled={Boolean(recoverySqzExportDisabledReason())}
                    title={recoverySqzExportDisabledReason()}
                    aria-label={labelWithDisabledReason(tr("gui.recovery.export_sqz", "Export SQZ"), recoverySqzExportDisabledReason())}
                    onclick={() => void submitExportSqzJob()}
                  ><Icon name="archive" size={17} />{tr("gui.recovery.export_sqz", "Export SQZ")}</button>
                  <button
                    class="sheet-action secondary-action"
                    disabled={Boolean(recoverySqzRepairDisabledReason())}
                    title={recoverySqzRepairDisabledReason()}
                    aria-label={labelWithDisabledReason(tr("gui.recovery.repair_sqz", "Repair SQZ"), recoverySqzRepairDisabledReason())}
                    onclick={() => void submitRepairSqzJob()}
                  ><Icon name="rotate-cw" size={17} />{tr("gui.recovery.repair_sqz", "Repair SQZ")}</button>
                  <button
                    class="primary sheet-action"
                    disabled={Boolean(recoveryProtectDisabledReason())}
                    title={recoveryProtectDisabledReason()}
                    aria-label={labelWithDisabledReason(tr("gui.recovery.protect_archive", "Protect archive"), recoveryProtectDisabledReason())}
                    onclick={() => void submitProtectJob()}
                  ><Icon name="shield-alert" size={17} />{tr("gui.recovery.protect_archive", "Protect archive")}</button>
                </div>
              </div>

              <div class="recovery-layout">
                <section class="recovery-main">
                  <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.recovery.protection_mode", "Protection mode")}</div>
                  <div class="recovery-mode-grid">
                    {#each recoveryModes as item, index}
                      <div class:active={index === 0} class={`recovery-mode mode-${item.tone}`}>
                        <strong>{recoveryModeName(index, item.name)}</strong>
                        <span>{recoveryModeDetail(index, item.detail)}</span>
                        <em>{recoveryModeSize(index, item.size)}</em>
                      </div>
                    {/each}
                  </div>

                  <div class="tolerance-panel">
                    <div>
                      <span class="block-label">{tr("gui.recovery.tolerate_loss", "Tolerate loss")}</span>
                      <strong>{tr("gui.recovery.tolerate_loss_value", "1 missing volume or 10% damaged blocks")}</strong>
                      <p>{tr("gui.recovery.tolerate_loss_body", "Shown as real repair capacity first; percentage is secondary to avoid false expectations.")}</p>
                    </div>
                    <div class="tolerance-control readonly">
                      <strong>{tr("gui.recovery.one_volume", "1 volume")}</strong>
                    </div>
                  </div>

	                  <div class="verify-card">
	                    <div class="verify-score">
	                      <span>{tr("gui.recovery.verify_result", "Verify result")}</span>
	                      <strong>{recoveryResultTitle()}</strong>
	                    </div>
	                    <div class="block-math">
	                      {#if recoveryResultAvailable()}
	                        <div><b>2</b><span>{tr("gui.recovery.damaged_blocks", "damaged blocks")}</span></div>
	                        <div><b>24</b><span>{tr("gui.recovery.recovery_blocks", "recovery blocks")}</span></div>
	                        <div><b>22</b><span>{tr("gui.recovery.remaining_margin", "remaining margin")}</span></div>
	                      {:else}
	                        <div><b>-</b><span>{recoveryResultDetail()}</span></div>
	                      {/if}
	                    </div>
	                    <p>{recoveryResultAvailable() ? tr("gui.recovery.damage_within_rs_capacity", "Detected damage is within the available Reed-Solomon recovery capacity.") : recoveryResultDetail()}</p>
	                    <div class="inline-actions">
	                      <button
	                        disabled={Boolean(recoveryZipDisabledReason())}
	                        title={recoveryZipDisabledReason()}
	                        aria-label={labelWithDisabledReason(tr("gui.recovery.repair_zip_index", "Repair ZIP index"), recoveryZipDisabledReason())}
	                        onclick={() => void submitRepairZipJob()}
	                      >{tr("gui.recovery.repair_zip_index", "Repair ZIP index")}</button>
	                      <button
	                        class="primary-lite"
	                        disabled={Boolean(recoveryRepairPar2DisabledReason())}
	                        title={recoveryRepairPar2DisabledReason()}
	                        aria-label={labelWithDisabledReason(tr("gui.recovery.repair_with_par2", "Repair with PAR2"), recoveryRepairPar2DisabledReason())}
	                        onclick={() => void submitRepairRecoveryJob()}
	                      >{tr("gui.recovery.repair_with_par2", "Repair with PAR2")}</button>
                      <button
                        disabled={Boolean(recoveryVerifyDisabledReason())}
                        title={recoveryVerifyDisabledReason()}
                        aria-label={labelWithDisabledReason(tr("gui.recovery.verify_with_par2", "Verify with PAR2"), recoveryVerifyDisabledReason())}
                        onclick={() => void submitVerifyRecoveryJob()}
                      >{tr("gui.recovery.verify_with_par2", "Verify with PAR2")}</button>
	                      <button
	                        disabled={Boolean(recoveryFailureDisabledReason())}
	                        title={recoveryFailureDisabledReason()}
	                        aria-label={labelWithDisabledReason(tr("gui.recovery.show_failed_case", "Show failed case"), recoveryFailureDisabledReason())}
	                        onclick={() => setScreen("cannotRepair")}
	                      >{tr("gui.recovery.show_failed_case", "Show failed case")}</button>
                    </div>
                  </div>
                </section>

                <aside class="recovery-side">
	                  <section class="sqz-recovery-card">
	                    <span class="block-label">{tr("gui.recovery.sqz_status", "SQZ status")}</span>
	                    <strong>{isCurrentArchiveSqz() ? tr("gui.recovery.embedded_available", "Embedded recovery available") : currentArchive ? tr("gui.recovery.standard_sidecar", "Standard sidecar recovery") : openArchiveFirstLabel()}</strong>
	                    <p>{isCurrentArchiveSqz() ? tr("gui.recovery.sqz_embedded_body", "SQZ embedded recovery can rewrite repaired bytes into a new archive when damage is within capacity.") : currentArchive ? tr("gui.recovery.par2_sidecars_body", "PAR2 sidecars are available for standard archives after protection data exists.") : tr("gui.recovery.open_before_capabilities", "Open an archive before checking recovery capabilities.")}</p>
                    <div class="inline-actions">
                      <button
                        disabled={Boolean(recoverySqzRepairDisabledReason())}
                        title={recoverySqzRepairDisabledReason()}
                        aria-label={labelWithDisabledReason(tr("gui.recovery.repair_sqz", "Repair SQZ"), recoverySqzRepairDisabledReason())}
                        onclick={() => void submitRepairSqzJob()}
                      >{tr("gui.recovery.repair_sqz", "Repair SQZ")}</button>
                      <button
                        disabled={Boolean(recoverySqzExportDisabledReason())}
                        title={recoverySqzExportDisabledReason()}
                        aria-label={labelWithDisabledReason(tr("gui.recovery.export_sqz", "Export SQZ"), recoverySqzExportDisabledReason())}
                        onclick={() => void submitExportSqzJob()}
                      >{tr("gui.recovery.export_sqz", "Export SQZ")}</button>
                    </div>
                  </section>
                  <section>
                    <span class="block-label">{tr("gui.recovery.target_archive", "Target archive")}</span>
                    <strong>{archiveTitle()}</strong>
                    <p>{tr("gui.recovery.sidecar_storage_body", "Standard 7Z remains untouched. Sidecar files are stored next to the archive.")}</p>
                  </section>
                  <section>
                    <span class="block-label">{tr("gui.recovery.output_files", "Output files")}</span>
                    <ul>
                      {#if currentArchive}
                        <li>{pathBaseName(defaultRecoveryPath())}</li>
                        <li>{pathBaseName(defaultRecoveryPath()).replace(".par2", ".vol000+001.par2")}</li>
                        <li>{pathBaseName(defaultRecoveryPath()).replace(".par2", ".vol001+002.par2")}</li>
                      {:else}
                        <li>{openArchiveFirstLabel()}</li>
                      {/if}
                    </ul>
                  </section>
                </aside>
              </div>
            </div>
          {:else if screen === "archiveInfo"}
            <div class="settings-view modern-archive-info">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.archive.info_eyebrow", "Archive / Info")}</span>
                  <h1>{tr("gui.archive.info_title", "Archive information")}</h1>
                  <p>{tr("gui.archive.info_subtitle", "Current archive, selection, extraction target, encoding, and volume state.")}</p>
                </div>
                <button class="sheet-action" onclick={() => setScreen("browse")}><Icon name="archive" size={17} />{tr("gui.nav.back_to_archive", "Back to archive")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="limits-table archive-info-table">
                    <div><b>{tr("common.field", "Field")}</b><b>{tr("common.value", "Value")}</b></div>
                    {#each archiveInfoRows() as row}
                      <div><span>{row[0]}</span><strong>{row[1]}</strong></div>
                    {/each}
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.extract.final_destination", "Final destination")}</div>
                  <div class="setting-callout">
                    <strong>{extractDestinationTitle(extractDestinationMode)}</strong>
                    <span>{effectiveExtractDest()}</span>
                  </div>
                  <div class="settings-route-list">
                    <button class="settings-route-card" onclick={() => setScreen("extract")}>
                      <Icon name="archive" size={16} />
                      <span><strong>{tr("gui.screen.extract", "Extract")}</strong><small>{extractDestinationHint()}</small></span>
                    </button>
                    <button class="settings-route-card" onclick={() => setScreen("checksum")}>
                      <Icon name="check-circle" size={16} />
                      <span><strong>{tr("gui.screen.checksum", "Checksum")}</strong><small>{tr("gui.checksum.route_from_info", "Verify this archive or another file")}</small></span>
                    </button>
                  </div>
                </aside>
              </div>
            </div>
          {:else if screen === "appearance"}
            <div class="appearance-view modern-appearance">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.appearance.eyebrow", "Appearance")}</span>
                  <h1>{tr("gui.appearance.title", "Interface and display")}</h1>
                  <p>{tr("gui.appearance.subtitle", "Mode switching is primary. Theme Colors sits under Appearance as a focused second-level page.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void saveAppearanceSettings()}><Icon name="settings" size={17} />{tr("gui.appearance.apply", "Apply appearance")}</button>
              </div>

              <div class="appearance-layout interface-layout">
                <section class="display-settings-panel main-display-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.appearance.display_settings", "Display settings")}</div>
                  <div class="setting-list">
                    <div class="setting-row mode-setting-row">
                      <span>{tr("gui.appearance.interface_mode", "Interface mode")}</span>
                      <div class="mode-segments" aria-label={tr("gui.appearance.interface_mode", "Interface mode")}>
                        <button class:active={modeIs("modern")} onclick={() => setMode("modern")}>{tr("gui.mode.modern", "Modern")}</button>
                        <button class:active={modeIs("classic")} onclick={() => setMode("classic")}>{tr("gui.mode.classic", "Classic")}</button>
                      </div>
                    </div>
                    <div class="setting-row mode-setting-row">
                      <span>{tr("gui.appearance.theme", "Theme")}</span>
                      <div class="mode-segments" aria-label={tr("gui.appearance.theme_preference", "Theme preference")}>
                        <button class:active={activeThemeChoice === "light"} onclick={() => setTheme("light")}>{tr("gui.theme.light", "Light")}</button>
                        <button class:active={activeThemeChoice === "dark"} onclick={() => setTheme("dark")}>{tr("gui.theme.dark", "Dark")}</button>
                        <button class:active={activeThemeChoice === "system"} onclick={() => setTheme("system")}>{tr("gui.theme.system", "System")}</button>
                      </div>
                    </div>
                    <div class="setting-row mode-setting-row">
                      <span>{tr("gui.appearance.density", "Density")}</span>
                      <div class="mode-segments" aria-label={tr("gui.appearance.density_preference", "Density preference")}>
                        <button class:active={activeDensityChoice === "compact"} onclick={() => setDensity("compact")}>{tr("gui.density.compact", "Compact")}</button>
                        <button class:active={activeDensityChoice === "standard"} onclick={() => setDensity("standard")}>{tr("gui.density.standard", "Standard")}</button>
                        <button class:active={activeDensityChoice === "comfort"} onclick={() => setDensity("comfort")}>{tr("gui.density.comfort", "Comfort")}</button>
                      </div>
                    </div>
                    <div><span>{tr("gui.appearance.current_colors", "Current theme colors")}</span><strong>{activePaletteName()}</strong></div>
                  </div>
                </section>

                <aside class="display-settings-panel appearance-side-panel">
                  <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.sections", "Settings sections")}</div>
                  <div class="interface-note compact">
                    <strong>{tr("gui.appearance.switch_context_title", "Switching keeps context")}</strong>
                    <span>{tr("gui.appearance.switch_context_body", "Open archive, selection, task state, theme, and theme colors remain shared.")}</span>
                  </div>
                  <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
                </aside>
              </div>
            </div>
          {:else if screen === "colors"}
            <div class="colors-view modern-colors">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.screen.colors", "Appearance · Theme Colors")}</span>
                  <h1>{tr("gui.colors.title", "Theme colors and custom accent")}</h1>
                  <p>{tr("gui.colors.subtitle", "Recommended theme color presets and custom accent controls live in this Appearance subpage instead of crowding Interface mode.")}</p>
                </div>
                <button
                  class="primary sheet-action"
                  disabled={paletteApplyBlocked}
                  aria-describedby="custom-color-status"
                  title={paletteApplyBlocked ? tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color") : tr("gui.colors.apply", "Apply theme colors")}
                  onclick={() => void savePaletteSettings()}
                ><Icon name="sparkles" size={17} />{tr("gui.colors.apply", "Apply theme colors")}</button>
              </div>

              <div class="appearance-layout">
                <section class="palette-panel">
                  <div class="panel-title"><Icon name="palette" size={16} />{tr("gui.colors.curated_palettes", "Theme color presets")}</div>
                  <div class="palette-grid">
                    {#each builtInPalettes as palette}
                      <button
                        class:selected={palette.id === activePalette}
                        class="palette-card"
                        style={paletteSwatchStyle(palette)}
                        onclick={() => setPalette(palette.id)}
                      >
                        <div class="palette-card-head">
                          <strong>{paletteName(palette)}</strong>
                          <span>{paletteMood(palette)}</span>
                        </div>
                        <div class="palette-swatches"><i></i><i></i><i></i></div>
                        <p>{paletteNote(palette)}</p>
                        <small>{tr("gui.colors.aa_contrast", "AA contrast")} {palettePreviewData(palette).contrast}</small>
                      </button>
                    {/each}
                  </div>
                </section>

                <aside class="color-workbench">
                  <div class="panel-title"><Icon name="sparkles" size={16} />{tr("gui.colors.custom_color_wheel", "Custom color wheel")}</div>
                  <div class="color-wheel-wrap">
                    <div class="color-wheel-picker" style={`${customThemePreviewStyle(activeTheme)}; ${colorWheelMarkerStyle()}`}>
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
                      bind:checked={accentContrastGuard}
                      aria-label={tr("gui.colors.contrast_guard_toggle", "Contrast guard")}
                      aria-describedby="contrast-guard-note"
                      title={accentContrastGuard ? tr("gui.colors.contrast_guard_enabled", "On · readable light/dark variants") : tr("gui.colors.contrast_guard_disabled", "Off · use accent more directly")}
                    />
                    <span>{accentContrastGuard ? tr("gui.colors.contrast_guard_enabled", "On · readable light/dark variants") : tr("gui.colors.contrast_guard_disabled", "Off · use accent more directly")}</span>
                  </label>
                  <div class="theme-preview-pair">
                    <div class="theme-preview custom-preview theme-light" style={customThemePreviewStyle("light")}>
                      <div class="preview-toolbar"><span></span><span></span><span class="preview-theme-pill">{tr("gui.theme.light", "Light")}</span></div>
                      <div class="preview-row selected"><span>archive.7z</span><strong>{tr("common.readiness", "Ready")}</strong></div>
                      <div class="preview-row"><span>sidecar.par2</span><strong>{tr("gui.colors.protected_preview", "Protected")}</strong></div>
                    </div>
                    <div class="theme-preview custom-preview theme-dark" style={customThemePreviewStyle("dark")}>
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
                  <p>{tr("gui.settings.general.subtitle", "Startup, language, default folders, update prompts, and file-open policy stay separate from Appearance.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void saveGeneralSettings()}><Icon name="settings" size={17} />{tr("gui.settings.general.apply", "Apply general")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.section.general", "General")}</div>
                  <div class="setting-list">
                    <div><span>{tr("gui.settings.general.startup", "Startup")}</span><strong>{tr("gui.settings.general.startup_value", "Open last archive workspace")}</strong></div>
                    <div class="setting-control-row">
                      <span>{tr("gui.settings.language", "Language")}</span>
                      <select class="settings-select" bind:value={generalLanguageChoice} aria-label={tr("gui.settings.language.preference_label", "Language preference")}>
                        <option value="">{tr("gui.settings.language.follow_system", "Follow system")}</option>
                        {#each availableLanguages as language}
                          <option value={language.tag}>{language.name} · {language.tag}</option>
                        {/each}
                      </select>
                    </div>
                    <div class="setting-control-row folder-setting-row">
                      <span>{tr("gui.settings.folder.default_extract", "Default extract folder")}</span>
                      <div class="settings-path-control">
                        <input
                          class="settings-path-input"
                          bind:value={generalDefaultExtractDir}
                          placeholder={tr("gui.settings.folder.next_to_archive", "Next to archive")}
                          aria-label={tr("gui.settings.folder.default_extract", "Default extract folder")}
                        />
                        <button type="button" aria-label={tr("gui.settings.folder.choose", "Choose default extract folder")} onclick={() => void chooseDefaultExtractFolder()}>
                          <Icon name="folder-open" size={15} />
                        </button>
                        <button type="button" class="settings-path-reset" onclick={clearDefaultExtractFolder}>{tr("gui.settings.folder.default", "Default")}</button>
                      </div>
                    </div>
                    <div class="setting-control-row">
                      <span>{tr("gui.settings.general.reveal_after_extract", "Reveal after extract")}</span>
                      <label class="settings-switch">
                        <input
                          type="checkbox"
                          bind:checked={generalRevealAfterExtract}
                          aria-label={tr("gui.settings.general.reveal_after_extract_aria", "Reveal extracted destination in {fileManager} after successful extract").replace("{fileManager}", fileManagerLabel())}
                        />
                        <span>{generalRevealAfterExtract ? tr("common.on", "On") : tr("common.off", "Off")} · {tr("gui.settings.general.reveal_after_extract_hint", "Show destination in {fileManager}").replace("{fileManager}", fileManagerLabel())}</span>
                      </label>
                    </div>
                    <div><span>{tr("gui.settings.general.open_with_policy", "{openWith} policy").replace("{openWith}", openWithLabel())}</span><strong>{tr("gui.settings.general.open_with_value", "Candidate only, never steal defaults")}</strong></div>
                    <div><span>{tr("gui.settings.general.updates", "Updates")}</span><strong class="deferred-state">{tr("gui.settings.general.updates_value", "Manual check · updater deferred")}</strong></div>
                  </div>
                  <div class="setting-callout">
                    <strong>{tr("gui.settings.general.boundary_title", "Safety prompts stay visible")}</strong>
                    <span>{tr("gui.settings.general.boundary_body", "Password, recovery, unsafe path, and conflict prompts remain visible in their workflows.")}</span>
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.settings.sections", "Settings sections")}</div>
                  <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
                </aside>
              </div>
            </div>
          {:else if screen === "settingsSecurity"}
            <div class="settings-view modern-settings-security">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.settings.security.eyebrow", "Settings / Security")}</span>
                  <h1>{tr("gui.settings.security.title", "Extraction safety and privacy")}</h1>
                  <p>{tr("gui.settings.security.subtitle", "Dangerous archive behavior is blocked by defaults; advanced overrides require explicit review.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void saveSafetySettings()}><Icon name="shield-alert" size={17} />{tr("gui.settings.security.save", "Save security")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="settings-metric-grid">
                    <div><span>{tr("gui.settings.security.max_entries", "Max entries")}</span><strong>{formattedNumber(safetyMaxEntries, defaultSafety.maxEntries)}</strong><small>{tr("gui.settings.captured_job_start", "Captured when job starts")}</small></div>
                    <div><span>{tr("gui.settings.security.max_output", "Max output")}</span><strong>{formattedNumber(safetyMaxOutputGiB, defaultSafety.maxOutputGiB)} GiB</strong><small>{tr("gui.settings.security.archive_bomb_guard", "Archive bomb guard")}</small></div>
                    <div><span>{tr("gui.settings.security.ratio_guard", "Ratio guard")}</span><strong>{formattedNumber(safetyMaxCompressionRatio, defaultSafety.maxCompressionRatio)}x</strong><small>{tr("gui.settings.security.ratio_hint_short", "Stops suspicious expansion")}</small></div>
                  </div>
                  <div class="settings-input-grid" aria-label={tr("common.safety_limits", "Safety limits")}>
                    <label class="number-field">
                      <span>{tr("gui.settings.security.max_entries", "Max entries")}</span>
                      <input type="number" min="1" max="10000000" step="1000" bind:value={safetyMaxEntries} />
                    </label>
                    <label class="number-field">
                      <span>{tr("gui.settings.security.max_output_gib", "Max output GiB")}</span>
                      <input type="number" min="1" max="8192" step="1" bind:value={safetyMaxOutputGiB} />
                    </label>
                    <label class="number-field">
                      <span>{tr("gui.settings.security.ratio_guard", "Ratio guard")}</span>
                      <input type="number" min="1" max="100000" step="1" bind:value={safetyMaxCompressionRatio} />
                    </label>
                  </div>
                  <div class="settings-actions-row">
                    <button class="primary-lite" onclick={() => void saveSafetySettings()}>{tr("gui.settings.security.save_limits", "Save limits")}</button>
                    <button class="secondary-lite" onclick={() => void resetSafetySettings()}>{tr("gui.settings.reset_defaults", "Reset defaults")}</button>
                    <span>{settingsSnapshotLabel}</span>
                  </div>
                  <div class="setting-callout danger">
                    <strong>{tr("gui.settings.security.disabled_until_connected", "Available after every affected task is connected")}</strong>
                    <span>{tr("gui.settings.security.disabled_until_connected_body", "Stream buffer memory is configured in Performance; per-format sandbox switches stay hidden until every affected task uses the setting snapshot.")}</span>
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.settings.sections", "Settings sections")}</div>
                  <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
                </aside>
              </div>
            </div>
          {:else if screen === "settingsPerformance"}
            <div class="settings-view modern-settings-performance">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.settings.performance.eyebrow", "Settings / Performance")}</span>
                  <h1>{tr("gui.settings.performance.title", "Performance and scale behavior")}</h1>
                  <p>{tr("gui.settings.performance.subtitle", "Only controls already supported by archive engine settings are active; speculative resource switches stay disabled.")}</p>
                </div>
                <button class="primary sheet-action" onclick={() => void savePerformanceSettings()}><Icon name="hourglass" size={17} />{tr("gui.settings.performance.save", "Save performance")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="settings-metric-grid">
                    <div><span>{tr("gui.settings.performance.workers", "Workers")}</span><strong>{performanceThreads === null ? tr("common.auto", "Auto") : formattedNumber(performanceThreads, 4)}</strong><small>{tr("gui.settings.performance.workers_hint", "Zstandard honors manual threads")}</small></div>
                    <div><span>{tr("gui.settings.performance.stream_buffer", "Stream buffer")}</span><strong>{performanceMemoryMiB === null ? tr("common.auto", "Auto") : `${formattedNumber(performanceMemoryMiB, 512)} MiB`}</strong><small>{tr("gui.settings.performance.copy_buffers", "Squallz copy buffers")}</small></div>
                    <div><span>{tr("gui.settings.performance.queue", "Task flow")}</span><strong>{tr("gui.settings.performance.one_active", "1 active")}</strong><small>{tr("gui.settings.performance.safer_disk", "Conservative, safe disk writes")}</small></div>
                    <div><span>{tr("gui.settings.performance.browse_scale", "Browse scale")}</span><strong>100k</strong><small>{tr("gui.settings.performance.indexed_ready", "Indexed browsing ready")}</small></div>
                  </div>
                  <div class="level-control settings-slider">
                    <div><strong>{tr("gui.settings.performance.worker_threads", "Compression worker threads")} · {performanceThreads === null ? tr("common.auto", "Auto") : formattedNumber(performanceThreads, 4)}</strong><span>{tr("gui.settings.performance.worker_threads_body", "Manual thread count is available for supported formats; others keep format defaults.")}</span></div>
                    <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.worker_threads", "Worker threads")}>
                      <button class:active={performanceThreads === null} onclick={() => choosePerformanceThreads(null)}>{tr("common.auto", "Auto")}</button>
                      <button class:active={performanceThreads === 4} onclick={() => choosePerformanceThreads(4)}>4</button>
                      <button class:active={performanceThreads === 8} onclick={() => choosePerformanceThreads(8)}>8</button>
                      <button class:active={performanceThreads === 16} onclick={() => choosePerformanceThreads(16)}>16</button>
                    </div>
                    <label class="number-field worker-field">
                      <span>{tr("gui.settings.performance.custom_threads", "Custom threads")}</span>
                      <input type="number" min="1" max="64" step="1" bind:value={performanceThreads} />
                    </label>
                    <div><strong>{tr("gui.settings.performance.stream_buffer_memory", "Stream buffer memory")} · {performanceMemoryMiB === null ? tr("common.auto", "Auto") : `${formattedNumber(performanceMemoryMiB, 512)} MiB`}</strong><span>{tr("gui.settings.performance.stream_buffer_body", "Caps Squallz-owned copy buffers; format encoders may keep their own dictionaries.")}</span></div>
                    <div class="mode-segments worker-segments" aria-label={tr("gui.settings.performance.stream_buffer_memory", "Stream buffer memory")}>
                      <button class:active={performanceMemoryMiB === null} onclick={() => choosePerformanceMemory(null)}>{tr("common.auto", "Auto")}</button>
                      <button class:active={performanceMemoryMiB === 256} onclick={() => choosePerformanceMemory(256)}>256 MiB</button>
                      <button class:active={performanceMemoryMiB === 512} onclick={() => choosePerformanceMemory(512)}>512 MiB</button>
                      <button class:active={performanceMemoryMiB === 1024} onclick={() => choosePerformanceMemory(1024)}>1 GiB</button>
                      <button class:active={performanceMemoryMiB === 2048} onclick={() => choosePerformanceMemory(2048)}>2 GiB</button>
                    </div>
                    <label class="number-field worker-field">
                      <span>{tr("gui.settings.performance.custom_buffer_mib", "Custom buffer MiB")}</span>
                      <input type="number" min="1" max="262144" step="1" bind:value={performanceMemoryMiB} />
                    </label>
                    <div class="settings-actions-row">
                      <button class="primary-lite" onclick={() => void savePerformanceSettings()}>{tr("gui.settings.performance.save_resources", "Save resources")}</button>
                      <button class="secondary-lite" onclick={() => void resetPerformanceSettings()}>{tr("gui.settings.performance.use_auto", "Use auto")}</button>
                      <span>{settingsSnapshotLabel}</span>
                    </div>
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.settings.sections", "Settings sections")}</div>
                  <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
                </aside>
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
                <button class="sheet-action" disabled={!currentArchive} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void forgetPasswordBookPanel()}><Icon name="lock" size={17} />{tr("gui.settings.password_book.forget_current", "Forget current archive")}</button>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="password-book-grid">
                    <div><span>{tr("gui.settings.password_book.secret_store", "Secret store")}</span><strong>{passwordBookSecretStoreLabel()}</strong><small>{tr("gui.settings.password_book.secret_store_detail", "{platform} {secretStore} when available; other platforms stay behind their own secret stores").replace("{platform}", platformNameLabel()).replace("{secretStore}", secretStoreLabel())}</small></div>
                    <div><span>{tr("gui.settings.password_book.current_archive", "Current archive")}</span><strong>{passwordBookCurrentLabel()}</strong><small>{passwordBookDetailLabel()}</small></div>
                    <div><span>{tr("gui.settings.password_book.frontend_access", "Frontend access")}</span><strong>{tr("gui.settings.password_book.never_plaintext", "Never plaintext")}</strong><small>{tr("gui.settings.password_book.ipc_status_only", "Only available/saved status crosses IPC")}</small></div>
                  </div>
                  <div class="settings-actions-row">
                    <button class="primary-lite" onclick={() => void refreshPasswordBookPanel()}>{tr("gui.settings.password_book.refresh_status", "Refresh status")}</button>
                    <button class="secondary-lite" disabled={!currentArchive} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void forgetPasswordBookPanel()}>{tr("gui.settings.password_book.forget_saved", "Forget saved password")}</button>
                    <span>{currentArchiveName()}</span>
                  </div>
                  <div class="limits-table">
                    <div><b>{tr("gui.settings.password_book.source", "Source")}</b><b>{tr("gui.settings.password_book.priority", "Priority")}</b><b>{tr("gui.settings.password_book.stored_where", "Stored where")}</b><b>{tr("gui.settings.password_book.failure_behavior", "Failure behavior")}</b></div>
                    <div><span>{tr("gui.settings.password_book.manual_prompt", "Manual prompt")}</span><span>1</span><span>{tr("gui.settings.password_book.transient_input", "Transient input")}</span><strong>{tr("gui.settings.password_book.retry_prompt", "Retry prompt")}</strong></div>
                    <div><span>{tr("gui.settings.password_book.session_cache", "Session cache")}</span><span>2</span><span>{tr("gui.settings.password_book.memory_cleared", "Memory cleared after use")}</span><strong>{tr("gui.settings.password_book.expires_session", "Expires with session")}</strong></div>
                    <div><span>{secretStoreLabel()}</span><span>3</span><span>{tr("gui.settings.password_book.system_secret_store", "System secret store")}</span><strong>{tr("gui.settings.password_book.fallback_prompt", "Fallback to prompt")}</strong></div>
                  </div>
                  <div class="setting-callout">
                    <strong>{tr("gui.settings.password_book.no_plaintext_storage_title", "No localStorage, no settings.json, no logs")}</strong>
                    <span>{tr("gui.settings.password_book.no_plaintext_storage_body", "Password Book UI never displays or exports saved password material.")}</span>
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.settings.sections", "Settings sections")}</div>
                  <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
                </aside>
              </div>
            </div>
          {:else if screen === "integration"}
            <div class="integration-view modern-integration">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.settings.integration.eyebrow", "Settings / File Associations")}</span>
                  <h1>{tr("gui.settings.integration.title", "File associations and context menus")}</h1>
                  <p>{tr("gui.settings.integration.subtitle", "{platform} starts as an {openWith} candidate. Other platforms use their own association and file-manager action matrices.").replace("{platform}", platformNameLabel()).replace("{openWith}", openWithLabel())}</p>
                </div>
                <div class="sheet-action-row integration-actions">
                  <button class="sheet-action secondary-action" disabled={integrationStatus === "applying"} onclick={() => void refreshIntegrationStatus()}><Icon name="search" size={17} />{tr("gui.common.refresh", "Refresh")}</button>
                  <button class="sheet-action secondary-action" disabled={integrationStatus === "applying"} onclick={() => void removeIntegrationChanges()}><Icon name="x-circle" size={17} />{tr("gui.settings.integration.uninstall_actions", "Uninstall actions")}</button>
                  <button class="primary sheet-action" disabled={integrationStatus === "applying"} onclick={() => void applyIntegrationChanges()}><Icon name="settings" size={17} />{integrationApplyLabel()}</button>
                </div>
              </div>

              <div class="integration-layout">
                <section class="association-panel">
                  <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.settings.integration.file_types", "File types")}</div>
                  <div class="association-tools">
                    <div class="mini-search"><Icon name="search" size={13} />{tr("gui.settings.integration.filter_extensions", "Filter extensions")}</div>
                    <div class="assoc-count">{formatRegistrySourceLabel()} · {tr("gui.settings.integration.showing_rows", "Showing {count} rows").replace("{count}", String(associationRows().length))}</div>
                  </div>
                  <div class="assoc-chip-row">
                    {#each associationSummary() as item}
                      <span>{item}</span>
                    {/each}
                  </div>
                  <div class="association-table">
                    <div class="assoc-head"><span>{tr("common.type", "Type")}</span><span>{tr("common.format", "Format")}</span><span>{tr("common.status", "Status")}</span><span>{tr("common.action", "Action")}</span></div>
                    {#each associationRows() as row}
                      <div class="assoc-row">
                        <strong>{row.ext}</strong><span>{row.format}</span><span>{row.status}</span><span>{row.action}</span>
                      </div>
                    {/each}
                  </div>
                </section>

                <aside class="context-panel">
                  <div class="panel-title"><Icon name="list" size={16} />{tr("gui.settings.integration.context_menu", "Context menu")}</div>
                  {#each contextActions as action, index}
                    <div class="check-row"><span class:on={index < 7 || action === "Convert archive"} class="fake-check"></span>{contextActionLabel(action)}</div>
                  {/each}
                  <div class="platform-note">
                    <strong>{platformNameLabel()}</strong>
                    <span>{tr("gui.settings.integration.platform_note", "{fileManager} services and quick actions are designed separately from document type registration.").replace("{fileManager}", fileManagerLabel())}</span>
                  </div>
                  <div class={`integration-install-state state-${integrationStatus}`}>
                    <strong>{integrationSummaryLabel()}</strong>
                    <span>{integrationDetailLabel()}</span>
                    {#if integrationScriptDir}
                      <small>{integrationScriptDir}</small>
                    {/if}
                  </div>
                </aside>
              </div>
            </div>
          {:else}
            <div class="archive-workspace" class:no-archive={!currentArchive}>
              <div class="archive-top">
                <div class="archive-hero">
                  <div class="archive-object" aria-hidden="true">
                    <div class="archive-lid"></div>
                    <div class="archive-core">
	                    <span>{archiveFormat()}</span>
                      <i></i>
                    </div>
                  </div>
                  <div class="archive-summary">
                    <span class="eyebrow">{tr("gui.archive.secure_archive", "Secure archive")}</span>
	                  <h1>{archiveTitle()}</h1>
	                  <p>{archiveSummary()}</p>
                    {#if currentArchive}
                      <div class="archive-breadcrumbs" aria-label={tr("gui.nav.archive_breadcrumbs", "Archive breadcrumbs")}>
                        <button type="button" onclick={() => void openArchiveBreadcrumb(-1)}>{archiveTitle()}</button>
                        {#each archiveDirs as dir, index}
                          <i>/</i><button type="button" onclick={() => void openArchiveBreadcrumb(index)}>{dir}</button>
                        {/each}
                      </div>
                    {/if}
                  </div>
                  <div class="summary-actions">
                    {#if currentArchive}
                      <button class="primary large" title={extractDestinationHint()} onclick={() => void submitExtractJob()}><Icon name="archive" size={17} />{actionLabel("Extract selected")}</button>
                      <button class="ghost large" onclick={() => void submitAddToArchiveJob()}><Icon name="file" size={17} />{actionLabel("Add files")}</button>
                      <button class="ghost large" onclick={() => setScreen("recovery")}><Icon name="shield-alert" size={17} />{actionLabel("Protect")}</button>
                      <button class="ghost large" onclick={() => void submitConvertJob()}><Icon name="repeat" size={17} />{actionLabel("Convert")}</button>
                      <button class="ghost large" onclick={() => setScreen("archiveInfo")}><Icon name="info" size={17} />{tr("gui.archive.info", "Info")}</button>
                      <button class="ghost large" disabled={!canRenameSelection()} title={renameSelectedDisabledReason()} aria-label={labelWithDisabledReason(actionLabel("Rename selected"), renameSelectedDisabledReason())} onclick={() => void submitRenameSelectedJob()}><Icon name="repeat" size={17} />{actionLabel("Rename selected")}</button>
                      <button class="ghost large" disabled={!hasArchiveSelection()} title={deleteSelectedDisabledReason()} aria-label={labelWithDisabledReason(actionLabel("Delete selected"), deleteSelectedDisabledReason())} onclick={() => void submitDeleteSelectedJob()}><Icon name="x-circle" size={17} />{actionLabel("Delete selected")}</button>
                      <button class="ghost large" disabled={!hasArchiveSelection()} title={moveSelectedDisabledReason()} aria-label={labelWithDisabledReason(actionLabel("Move selected"), moveSelectedDisabledReason())} onclick={() => void submitMoveSelectedJob()}><Icon name="repeat" size={17} />{actionLabel("Move selected")}</button>
                      <button class="ghost large" onclick={() => void submitNewFolderJob()}><Icon name="folder-open" size={17} />{actionLabel("New folder")}</button>
                      <button class="ghost large" disabled={!canPreviewEntrySelection()} aria-busy={previewBusy()} title={previewSelectedDisabledReason()} aria-label={labelWithDisabledReason(previewActionLabel(), previewSelectedDisabledReason())} onclick={() => void submitPreviewEntry()}><Icon name="eye" size={17} />{previewActionLabel()}</button>
                      {#if nestedPreview}
                        <button class="ghost large" onclick={() => void openNestedPreviewArchive()}><Icon name="folder-open" size={17} />{tr("gui.action.open_nested", "Open")}</button>
                        <button class="ghost large" onclick={() => void extractNestedPreviewArchive()}><Icon name="archive" size={17} />{tr("gui.action.extract_nested", "Extract")}</button>
                      {/if}
                    {:else}
	                    <button class="primary large" aria-busy={archiveOpenStatus === "opening"} onclick={() => void openArchiveFromDialog()}><Icon name="folder-open" size={17} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
	                    <button class="ghost large" onclick={() => setScreen("create")}><Icon name="sparkles" size={17} />{toolbarLabel("Create")}</button>
	                  {/if}
                  </div>
                </div>

                {#if currentArchive && hasArchiveSelection()}
                  <div class="workbench-strip">
                    <div class="update-safety-strip" aria-label={tr("gui.update.safety_summary", "Archive update safety summary")}>
                      <span><Icon name="check-circle" size={14} />{tr("gui.update.selection_scoped", "Selection-scoped updates")}</span>
                      <span><Icon name="list" size={14} />{tr("gui.update.target_review", "Review rename and move targets first")}</span>
                      <span><Icon name="archive" size={14} />{tr("gui.update.format_boundaries", "Write-capable formats only")}</span>
                    </div>
                    <label>
                      <span>{actionLabel("Rename target")}</span>
                      <input aria-label={tr("gui.rename.target_name", "Rename target name")} bind:value={renameTargetName} disabled={!canRenameSelection()} title={canRenameSelection() ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")} onblur={() => commitRenameTargetName()} />
                    </label>
                    <label>
                      <span>{actionLabel("Move target")}</span>
                      <input aria-label={tr("gui.move.target_folder", "Move target folder")} bind:value={moveTargetDir} disabled={!hasArchiveSelection()} title={hasArchiveSelection() ? "" : tr("gui.precondition.select_entries", "Select entries first")} onblur={() => commitMoveTargetDir()} />
                    </label>
                    <small>{renameTargetStatus()}</small>
                    <div class="move-target-presets compact" aria-label={tr("gui.move.target_presets", "Move target presets")}>
                      {#each moveTargetPresets as target}
                        <button class:active={normalizeMoveTargetDir(moveTargetDir) === target} disabled={!hasArchiveSelection()} onclick={() => commitMoveTargetDir(target)}>{target}</button>
                      {/each}
                    </div>
                    <small>{moveTargetStatus()}</small>
                    <label>
                      <span>{actionLabel("New folder")}</span>
                      <input aria-label={tr("gui.new_folder.name", "New folder name")} bind:value={newFolderName} onblur={() => commitNewFolderName()} />
                    </label>
                    <small class="workbench-note">{newFolderStatus()}</small>
                  </div>
                {:else if currentArchive}
                  <div class="workbench-strip empty-workbench-strip">
                    <span>{tr("gui.selection.select_entries_hint", "Select entries to rename, move, preview, or extract.")}</span>
                    <small>{tr("gui.preview.double_click_hint", "Choose one entry to enable Preview.")}</small>
                  </div>
                {/if}

                {#if moveConflictReview}
                  <div class="move-conflict-review" role="dialog" aria-label={tr("gui.move.conflicts", "Move target conflicts")} tabindex="-1">
                    <div>
                      <span class="block-label">{tr("gui.move.conflicts", "Move target conflicts")}</span>
                      <strong>{tr("gui.move.target_conflicts", "{count} target conflicts in {target}").replace("{count}", String(moveConflictCount())).replace("{target}", moveConflictReview.targetDir)}</strong>
                      <p>{tr("gui.move.ready_without_renaming", "{count} entries are ready to move without changing names.").replace("{count}", String(moveReadyCount()))}</p>
                    </div>
                    <div class="move-conflict-list">
                      {#each visibleMoveConflictItems() as item}
                        <div>
                          <strong>{item.from}</strong>
                          <span>{item.reason}</span>
                          <em>{item.to}</em>
                          <b>{item.keepBothTo}</b>
                        </div>
                      {/each}
                    </div>
                    <div class="move-conflict-actions">
                      <button onclick={() => moveConflictReview = null}>{tr("common.cancel", "Cancel")}</button>
                      <button disabled={moveReadyCount() === 0} onclick={() => void submitMoveReadyOnly()}>{tr("gui.move.ready_only", "Move ready only")}</button>
                      <button class="primary-lite" onclick={() => void submitMoveKeepBoth()}>{tr("gui.move.keep_both_all", "Keep both and move all")}</button>
                    </div>
                  </div>
                {/if}

                {#if currentArchive}
                  <div class="recovery-ribbon">
                    <div>
                      <Icon name="shield-alert" size={17} />
                      <strong>{tr("gui.recovery.not_enabled", "Recovery not enabled")}</strong>
                      <span>{tr("gui.recovery.add_par2_or_sqz", "Add PAR2 sidecar files or pack as SQZ before long-term storage.")}</span>
                    </div>
                    <button onclick={() => setScreen("recovery")}>{tr("gui.recovery.protect_archive", "Protect archive")}</button>
                  </div>
                {/if}

	              {#if hasEncodingWarning()}
	                <div class="warning-ribbon">
	                  <Icon name="alert-triangle" size={17} />
	                  <span>{archiveWarningText()}</span>
	                  <button onclick={() => void repairFilenameEncoding("gbk")}>{tr("gui.encoding.repair_with_gbk", "Repair with GBK")}</button>
	                </div>
	              {/if}
              </div>

              {#if currentArchive}
                <div class="modern-list" data-total-rows={totalRows()}>
                <div class="list-head">
                  <span>{tr("gui.list.col.name", "Name")}</span><span>{tr("gui.list.col.size", "Size")}</span><span>{tr("gui.list.col.packed", "Packed")}</span><span>{tr("gui.list.col.modified", "Modified")}</span>
                </div>
                <div class="virtual-scroll modern-virtual-scroll" data-virtual-list="modern" onscroll={onBrowseVirtualScroll}>
                  <div class="virtual-pad" style={`height: ${browsePaddingTop(MODERN_ROW_HEIGHT)}px`}></div>
                  {#each browseEntries(MODERN_ROW_HEIGHT) as entry}
		                <div class:selected={isEntrySelected(entry)} class="modern-row" role="button" tabindex="0" data-row-index={entry.virtualIndex ?? ""} onclick={(event) => selectEntry(entry, event)} ondblclick={(event) => { event.preventDefault(); void activateEntry(entry); }} onkeydown={(event) => onEntryKeydown(event, entry)} oncontextmenu={(event) => openEntryContext(event, entry)}>
                      <div class="file-name">
                        <button
                          type="button"
                          class="row-select-toggle"
                          class:checked={isEntrySelected(entry)}
                          role="checkbox"
                          aria-checked={isEntrySelected(entry)}
                          aria-label={entrySelectionLabel(entry)}
                          title={entrySelectionLabel(entry)}
                          disabled={!entry.source}
                          onclick={(event) => {
                            event.stopPropagation();
                            toggleEntrySelection(entry);
                          }}
                        ></button>
                        <span class:type-folder={entry.type === "folder"} class:type-locked={entry.type === "locked"} class:type-warning={entry.type === "warning"} class="file-badge">
                          {entry.type === "folder" ? "DIR" : entry.type === "pdf" ? "PDF" : entry.type === "sheet" ? "XLS" : entry.type === "locked" ? "AES" : entry.type === "warning" ? "TXT" : "FILE"}
                        </span>
                        <strong>{entry.name}</strong>
                        {#if entry.source}
                          <button
                            class="row-preview-button"
                            disabled={previewBusy()}
                            aria-busy={previewBusy()}
                            title={previewActionLabel(entry.source.path, entry.source.entry_type)}
                            aria-label={`${previewActionLabel(entry.source.path, entry.source.entry_type)} ${entry.name}`}
                            onclick={(event) => {
                              event.stopPropagation();
                              selectOnlyEntry(entry);
                              void submitPreviewEntry(entry.source?.path ?? null, entry.source?.entry_type ?? null);
                            }}
                          ><Icon name={entry.source.entry_type === "dir" ? "folder-open" : "eye"} size={13} /></button>
                        {/if}
                      </div>
                      <span>{entry.size}</span>
                      <span>{entry.packed}</span>
                      <span>{entry.modified}</span>
                    </div>
                  {:else}
                    <div class="modern-row empty-row" role="status">
                      <div class="file-name"><strong>{noEntriesLabel()}</strong></div>
                      <span>{noEntriesLabel()}</span><span>-</span><span>-</span>
                    </div>
                  {/each}
                  <div class="virtual-pad" style={`height: ${browsePaddingBottom(MODERN_ROW_HEIGHT)}px`}></div>
                </div>
              </div>
              {/if}
            </div>
          {/if}
        </section>

        {#if !isSettingsScreen() && screen !== "recent" && screen !== "convert" && (screen !== "browse" || currentArchive)}
        <aside class="modern-inspector" aria-label={tr("gui.aria.archive_inspector", "Archive inspector")}>
          {#if screen === "create"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.create.input_preflight", "Input preflight")}</span>
              <div class="health-score"><strong>{createEstimateTitle()}</strong><span>{createEstimateSubtitle()}</span></div>
              <progress
                class="meter meter-progress"
                value={createEstimateMeterWidth()}
                max="100"
                aria-label={tr("gui.create.input_preflight", "Input preflight")}
              ></progress>
              <p>{createEstimateBody()}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.create.destination_disk", "Destination disk")}</span>
              <strong>{diskPreflightTitle()}</strong>
              <p>{diskPreflightBody()}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.create.temporary_space", "Temporary space")}</span>
              <strong>{tempPreflightTitle()}</strong>
              <p>{tempPreflightBody()}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.create.format_capability", "Format capability")}</span>
              <dl>
                <div><dt>{tr("gui.create.capability_7z_create", "7Z create")}</dt><dd>{tr("common.yes", "Yes")}</dd></div>
                <div><dt>{tr("gui.create.name_encryption", "Name encryption")}</dt><dd>{tr("common.yes", "Yes")}</dd></div>
                <div><dt>{tr("gui.create.split_volumes", "Split volumes")}</dt><dd>.001</dd></div>
                <div><dt>{tr("gui.recovery.title", "Recovery")}</dt><dd>PAR2</dd></div>
                <div><dt>{tr("gui.create.rar_output", "RAR output")}</dt><dd>{tr("common.no", "No")}</dd></div>
              </dl>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.create.rules", "Rules")}</span>
              <strong>{tr("gui.create.protected_profile_archive", "{profile} protected archive").replace("{profile}", createProfileLabel(activeCreateProfile))}</strong>
              <p>{tr("gui.create.active_rules_summary", "{count} active: {summary}").replace("{count}", createExcludeCountLabel()).replace("{summary}", createExcludeSummary())}</p>
            </div>
          {:else if screen === "extract"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.extract.scope", "Extract scope")}</span>
              <div class="health-score"><strong>{currentArchive ? selectedPaths().size.toLocaleString() : "0"}</strong><span>{currentArchive ? tr("gui.selection.items", "selected items") : noArchiveLabel()}</span></div>
              <p>{currentArchive ? tr("gui.extract.smart_folder_hint", "Smart extract creates a containing folder when roots are mixed, preventing files from scattering into Downloads.") : openArchiveFirstLabel()}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.extract.safety_guard", "Safety guard")}</span>
              <dl>
                <div><dt>Zip Slip</dt><dd>{tr("gui.state.blocked", "Blocked")}</dd></div>
                <div><dt>{tr("gui.extract.bomb_ratio", "Bomb ratio")}</dt><dd>{tr("gui.state.guarded", "Guarded")}</dd></div>
                <div><dt>{tr("common.symlinks", "Symlinks")}</dt><dd>{tr("gui.state.contained", "Contained")}</dd></div>
                <div><dt>{tr("gui.extract.conflicts", "Conflicts")}</dt><dd>{tr("gui.extract.overwrite.ask", "Ask")}</dd></div>
              </dl>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.extract.next_reviews", "Next reviews")}</span>
                <div class="inline-actions">
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.batch.title", "Batch Extract"), extractArchiveRequiredReason())}
                    onclick={() => setScreen("batch")}
                  >{tr("gui.batch.title", "Batch Extract")}</button>
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.extract.password", "Password"), extractArchiveRequiredReason())}
                    onclick={() => setScreen("password")}
                  >{tr("gui.extract.password", "Password")}</button>
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.extract.conflicts", "Conflicts"), extractArchiveRequiredReason())}
                    onclick={() => setScreen("conflict")}
                  >{tr("gui.extract.conflicts", "Conflicts")}</button>
              </div>
            </div>
          {:else if screen === "batch"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.batch.readiness", "Batch readiness")}</span>
              <div class="health-score"><strong>{batchReadyCount()} / {batchReviewArchives().length}</strong><span>{tr("gui.state.ready", "Ready")}</span></div>
              <progress
                class="meter meter-progress"
                value={batchReadyPercent()}
                max="100"
                aria-label={tr("gui.batch.readiness", "Batch readiness")}
              ></progress>
              <p>{batchReviewArchives().length === 0 ? openArchiveFirstLabel() : tr("gui.batch.ready_continue_hint", "Ready archives can continue without global failure.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.batch.policy", "Batch policy")}</span>
              <dl>
                <div><dt>{tr("gui.batch.targets", "Targets")}</dt><dd>{tr("gui.batch.per_archive", "Per archive")}</dd></div>
                <div><dt>{tr("gui.extract.conflicts", "Conflicts")}</dt><dd>{tr("gui.extract.overwrite.ask", "Ask")}</dd></div>
                <div><dt>{tr("gui.batch.passwords", "Passwords")}</dt><dd>{tr("gui.batch.per_archive", "Per archive")}</dd></div>
                <div><dt>RAR</dt><dd>{tr("gui.format.extract_only", "Extract only")}</dd></div>
              </dl>
            </div>
          {:else if screen === "password"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.password.boundary", "Password boundary")}</span>
              <strong>{tr("gui.password.no_plaintext_persistence", "No plaintext persistence")}</strong>
              <p>{tr("gui.password.saved_boundary_body", "Saved passwords stay behind the system secret-store boundary; task status and settings only show status.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.password.fallback_order", "Fallback order")}</span>
              <dl>
                <div><dt>{tr("gui.password.manual", "Manual")}</dt><dd>{tr("gui.priority.first", "First")}</dd></div>
                <div><dt>{tr("gui.password.session", "Session")}</dt><dd>{tr("gui.priority.second", "Second")}</dd></div>
                <div><dt>{secretStoreLabel()}</dt><dd>{tr("gui.priority.third", "Third")}</dd></div>
                <div><dt>{tr("gui.password.logs", "Logs")}</dt><dd>{tr("gui.priority.never", "Never")}</dd></div>
              </dl>
            </div>
          {:else if screen === "conflict"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.extract.conflict_policy", "Conflict policy")}</span>
              <div class="health-score"><strong>3</strong><span>{tr("gui.conflict.items", "items")}</span></div>
              <p>{tr("gui.conflict.policy_body", "Decisions can apply per file or to the remaining conflict set; default is never silent overwrite.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.conflict.available_actions", "Available actions")}</span>
              <dl>
                <div><dt>{tr("gui.conflict.overwrite", "Overwrite")}</dt><dd>{tr("gui.conflict.explicit", "Explicit")}</dd></div>
                <div><dt>{tr("gui.conflict.skip", "Skip")}</dt><dd>{tr("gui.conflict.safe", "Safe")}</dd></div>
                <div><dt>{tr("gui.conflict.rename", "Keep Both")}</dt><dd>{tr("gui.conflict.renames", "Renames")}</dd></div>
                <div><dt>{tr("gui.conflict.compare", "Compare")}</dt><dd>{tr("gui.conflict.metadata", "Metadata")}</dd></div>
              </dl>
            </div>
          {:else if screen === "cannotRepair"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.recovery.repair_math", "Repair math")}</span>
              <div class="health-score danger-score"><strong>37 / 24</strong><span>{tr("gui.recovery.blocks", "blocks")}</span></div>
              <progress
                class="meter meter-progress danger"
                value="100"
                max="100"
                aria-label={tr("gui.recovery.repair_math", "Repair math")}
              ></progress>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.recovery.repair_blocked", "Repair blocked")}</span>
              <strong>{tr("gui.recovery.no_overpromise", "Do not overpromise repair")}</strong>
              <p>{tr("gui.recovery.no_overpromise_body", "When damage exceeds recovery capacity, Squallz offers partial extract and report export, not a fake repair button.")}</p>
              <div class="inline-actions">
                <button onclick={() => setScreen("recovery")}>{tr("gui.recovery.back_to_recovery", "Back to Recovery")}</button>
              </div>
            </div>
          {:else if screen === "recovery"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.recovery.repair_math", "Repair math")}</span>
              <div class="health-score"><strong>2 / 24</strong><span>{tr("gui.recovery.blocks_used", "blocks used")}</span></div>
              <progress
                class="meter meter-progress"
                value="8"
                max="100"
                aria-label={tr("gui.recovery.repair_math", "Repair math")}
              ></progress>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.recovery.compatibility", "Compatibility")}</span>
              <dl>
                <div><dt>PAR2</dt><dd>{tr("gui.recovery.standard", "Standard")}</dd></div>
                <div><dt>SQZ</dt><dd>Squallz</dd></div>
                <div><dt>{tr("common.export", "Export")}</dt><dd>7Z/ZIP</dd></div>
                <div><dt>RAR</dt><dd>{tr("gui.format.no_create", "No create")}</dd></div>
              </dl>
            </div>
          {:else if screen === "integration"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.integration.macos_policy", "{platform} policy").replace("{platform}", platformNameLabel())}</span>
              <strong>{tr("gui.settings.integration.open_with_candidate", "{openWith} candidate").replace("{openWith}", openWithLabel())}</strong>
              <p>{tr("gui.settings.integration.dev_build_policy", "Squallz does not take over default apps automatically. File-manager actions are explicit opt-in integrations.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.integration.coverage", "Coverage")}</span>
              <dl>
                <div><dt>{tr("gui.settings.integration.browse_types", "Browse types")}</dt><dd>ZIP, 7Z, TAR</dd></div>
                <div><dt>RAR</dt><dd>{tr("gui.settings.integration.extract_only", "Extract only")}</dd></div>
                <div><dt>.001</dt><dd>{tr("gui.settings.integration.not_claimed", "Not claimed")}</dd></div>
                <div><dt>{tr("gui.settings.integration.context_menu", "Context menu")}</dt><dd>{tr("gui.settings.integration.actions_count", "10 actions")}</dd></div>
              </dl>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.integration.install_scope", "Install scope")}</span>
              <strong>{tr("gui.settings.integration.separate_rule_title", "Associations and context menus are separate")}</strong>
              <p>{tr("gui.settings.integration.separate_rule_body", "They are shown together for review, but installed through platform-specific mechanisms.")}</p>
            </div>
          {:else if screen === "appearance"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.appearance.current_mode", "Current mode")}</span>
              <div class="health-score"><strong>{mode === "modern" ? tr("gui.mode.modern", "Modern") : tr("gui.mode.classic", "Classic")}</strong><span>{tr("gui.appearance.shared_engine", "Shared engine")}</span></div>
              <p>{tr("gui.appearance.current_mode_body", "Mode switching changes shell and density only; archive state and jobs stay intact.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.appearance.config_model", "Configuration model")}</span>
              <dl>
                <div><dt>{tr("gui.appearance.interface_mode", "Interface mode")}</dt><dd>{mode === "modern" ? tr("gui.mode.modern", "Modern") : tr("gui.mode.classic", "Classic")}</dd></div>
                <div><dt>{tr("gui.appearance.theme", "Theme")}</dt><dd>{themeStatusLabel()}</dd></div>
                <div><dt>{tr("gui.appearance.density", "Density")}</dt><dd>{densityLabel(activeDensityChoice)}</dd></div>
              </dl>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.appearance.colors", "Theme Colors")}</span>
              <strong>{activePaletteName()}</strong>
              <p>{tr("gui.appearance.colors_body", "Theme color presets and custom accent controls have their own page.")}</p>
              <div class="inline-actions">
                <button onclick={() => setScreen("colors")}>{tr("gui.appearance.open_colors", "Open Theme Colors")}</button>
              </div>
            </div>
          {:else if screen === "colors"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.colors.current_palette", "Current theme color")}</span>
              <div class="health-score"><strong>{activePaletteName()}</strong><span>{activePaletteMood()}</span></div>
              <div class="palette-mini" style={`--swatch-a: ${activePalettePreviewData.accent}; --swatch-b: ${activePalettePreviewData.support}; --swatch-c: ${activePalettePreviewData.base};`}><i></i><i></i><i></i></div>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.appearance.config_model", "Configuration model")}</span>
              <dl>
                <div><dt>{tr("gui.colors.accent_palette", "Theme color preset")}</dt><dd>{activePaletteName()}</dd></div>
                <div><dt>{tr("gui.colors.custom_accent", "Custom accent")}</dt><dd>{customAccent}</dd></div>
                <div><dt>{tr("gui.colors.semantic_colors", "semantic colors")}</dt><dd>{tr("gui.colors.locked", "locked")}</dd></div>
                <div><dt>{tr("gui.colors.contrast", "Contrast")}</dt><dd>{activePalettePreviewData.contrast}</dd></div>
              </dl>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.colors.accessibility", "Accessibility")}</span>
              <strong>{tr("gui.colors.aa_contrast_guard", "AA contrast guard")}</strong>
              <p>{tr("gui.colors.aa_contrast_guard_body", "Generated hover, focus, and selected colors are clamped before the Apply button becomes available.")}</p>
            </div>
          {:else if screen === "settingsGeneral"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.inspector.model", "Settings model")}</span>
              <div class="health-score"><strong>{tr("gui.settings.section.general", "General")}</strong><span>{tr("gui.settings.inspector.non_secret", "non-secret")}</span></div>
              <p>{tr("gui.settings.inspector.storage_body", "Startup, language, default folders, and update behavior belong in settings storage, not archive job specs.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.inspector.active_sections", "Active sections")}</span>
              <dl>
                <div><dt>{tr("gui.screen.appearance", "Appearance")}</dt><dd>{tr("gui.settings.inspector.separate", "Separate")}</dd></div>
                <div><dt>{tr("gui.screen.colors", "Appearance · Theme Colors")}</dt><dd>{tr("gui.settings.inspector.subpage", "Subpage")}</dd></div>
                <div><dt>{tr("gui.settings.section.security", "Security")}</dt><dd>{tr("gui.settings.inspector.snapshot", "Snapshot")}</dd></div>
                <div><dt>{tr("gui.settings.section.password_book", "Password Book")}</dt><dd>{tr("gui.settings.password_book.secret_store_short", "Secret store")}</dd></div>
              </dl>
            </div>
          {:else if screen === "settingsSecurity"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.security.safety_snapshot", "Safety snapshot")}</span>
              <div class="health-score"><strong>{tr("common.on", "On")}</strong><span>{tr("gui.settings.security.per_job", "per job")}</span></div>
              <p>{tr("gui.settings.security.snapshot_body", "Extraction jobs capture safety limits when submitted so later setting changes do not mutate running work.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.security.hard_guards", "Hard guards")}</span>
              <dl>
                <div><dt>Zip Slip</dt><dd>{tr("gui.settings.security.never_off", "Never off")}</dd></div>
                <div><dt>{tr("gui.settings.security.symlink_escape", "Symlink escape")}</dt><dd>{tr("gui.settings.security.never_off", "Never off")}</dd></div>
                <div><dt>{tr("gui.settings.security.reserved_names", "Reserved names")}</dt><dd>{tr("gui.settings.security.sanitize", "Sanitize")}</dd></div>
                <div><dt>{tr("gui.settings.security.bomb_ratio", "Bomb ratio")}</dt><dd>{tr("gui.settings.security.limited", "Limited")}</dd></div>
              </dl>
            </div>
          {:else if screen === "settingsPerformance"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.performance.resource_policy", "Resource policy")}</span>
              <div class="health-score"><strong>{tr("common.auto", "Auto")}</strong><span>{tr("gui.settings.performance.workers_short", "workers")}</span></div>
              <p>{tr("gui.settings.performance.zstandard_workers_body", "Only Zstandard currently honors manual workers; unsupported format controls stay hidden until their engines use the setting snapshot.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.performance.scale_readiness", "Scale readiness")}</span>
              <dl>
                <div><dt>{tr("gui.settings.performance.browse_100k", "100k browse")}</dt><dd>{tr("gui.settings.performance.indexed_path_ready", "Indexed path ready")}</dd></div>
                <div><dt>{tr("gui.settings.performance.stream_buffer", "Stream buffer")}</dt><dd>{performanceMemoryMiB === null ? tr("common.auto", "Auto") : `${formattedNumber(performanceMemoryMiB, 512)} MiB`}</dd></div>
              </dl>
            </div>
          {:else if screen === "passwordBook"}
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.password_book.secret_boundary", "Secret boundary")}</span>
              <div class="health-score"><strong>{passwordBookSecretStoreLabel()}</strong><span>{currentArchive ? tr("gui.settings.password_book.archive_scoped", "archive scoped") : tr("gui.empty.no_archive_short", "No archive open")}</span></div>
              <p>{tr("gui.settings.password_book.status_only_body", "Saved passwords never cross the frontend boundary as plaintext; UI shows status only.")}</p>
            </div>
            <div class="inspector-block">
              <span class="block-label">{tr("gui.settings.password_book.priority", "Priority")}</span>
              <dl>
                <div><dt>{tr("gui.settings.password_book.manual", "Manual")}</dt><dd>{tr("gui.priority.first", "First")}</dd></div>
                <div><dt>{tr("gui.settings.password_book.session", "Session")}</dt><dd>{tr("gui.priority.second", "Second")}</dd></div>
                <div><dt>{secretStoreLabel()}</dt><dd>{tr("gui.priority.third", "Third")}</dd></div>
                <div><dt>{tr("gui.settings.password_book.logs", "Logs")}</dt><dd>{tr("gui.priority.never", "Never")}</dd></div>
              </dl>
            </div>
          {:else}
            <div class="inspector-block nested-preview-block" data-preview-policy={activePreviewPolicyKind()} data-preview-code={activePreviewPolicyCode()}>
              <span class="block-label">{tr("gui.preview.panel", "Entry preview")}</span>
              {#if nestedPreview}
                <strong>{nestedPreviewTitle()}</strong>
                <p>{nestedPreviewSubtitle()}</p>
                <div class="nested-preview-list">
                  {#each nestedPreviewRows() as item}
                    <div>
                      <span>{item.entry_type === "dir" ? "DIR" : "FILE"}</span>
                      <strong>{item.display}</strong>
                      <small>{formatBytes(item.size)}</small>
                    </div>
                  {/each}
                </div>
                <div class="inline-actions">
                  <button onclick={() => void openNestedPreviewArchive()}><Icon name="folder-open" size={14} />{tr("gui.action.open_nested", "Open")}</button>
                  <button onclick={() => void extractNestedPreviewArchive()}><Icon name="archive" size={14} />{tr("gui.action.extract_nested", "Extract")}</button>
                  <button onclick={() => nestedPreview = null}>{tr("gui.common.clear", "Clear")}</button>
                </div>
              {:else}
                <strong>{entryPreviewTitle()}</strong>
                <p>{entryPreviewSubtitle()}</p>
                {#if previewBusy()}
                  <div class="preview-loading" role="status" aria-live="polite">
                    <span>{tr("gui.preview.loading", "Loading preview")}</span>
                    <small>{entryPreviewSubtitle()}</small>
                  </div>
                {:else if entryPreviewFailure}
                  <div class="inline-actions">
                    <button onclick={() => retryEntryPreview()}><Icon name="rotate-cw" size={14} />{tr("gui.preview.retry", "Retry preview")}</button>
                  </div>
                {:else if entryPreview}
                  {#if entryPreviewImageSrc()}
                    <img class="entry-preview-image" src={entryPreviewImageSrc() ?? ""} alt={entryPreview.display_name} />
                  {/if}
                  <div class="inline-actions">
                    <button class="preview-system-action" onclick={() => void openEntryPreview()}><Icon name="external-link" size={14} />{tr("gui.action.open_preview", "Open")}</button>
                    <button onclick={() => void revealEntryPreview()}><Icon name="folder-open" size={14} />{tr("gui.toast.reveal", "Reveal")}</button>
                    <button onclick={() => entryPreview = null}>{tr("gui.common.clear", "Clear")}</button>
                  </div>
                {:else}
                  <div class="inline-actions">
                    <button disabled={!canPreviewEntrySelection()} aria-busy={previewBusy()} title={previewSelectedDisabledReason()} aria-label={labelWithDisabledReason(previewActionLabel(), previewSelectedDisabledReason())} onclick={() => void submitPreviewEntry()}><Icon name="eye" size={14} />{previewActionLabel()}</button>
                  </div>
                {/if}
              {/if}
            </div>

            {#if canRenameSelection()}
            <div class="inspector-block move-target-block">
              <span class="block-label">{actionLabel("Rename target")}</span>
              <input class="move-target-input" aria-label={tr("gui.rename.target_name", "Rename target name")} bind:value={renameTargetName} onblur={() => commitRenameTargetName()} />
              <p>{renameTargetStatus()}</p>
            </div>
            {/if}

            {#if hasArchiveSelection()}
            <div class="inspector-block move-target-block">
              <span class="block-label">{actionLabel("Move target")}</span>
              <input class="move-target-input" aria-label={tr("gui.move.target_folder", "Move target folder")} bind:value={moveTargetDir} onblur={() => commitMoveTargetDir()} />
              <div class="move-target-presets" aria-label={tr("gui.move.target_presets", "Move target presets")}>
                {#each moveTargetPresets as target}
                  <button class:active={normalizeMoveTargetDir(moveTargetDir) === target} onclick={() => commitMoveTargetDir(target)}>{target}</button>
                {/each}
              </div>
              <p>{moveTargetStatus()}</p>
            </div>
            {/if}

            <div class="inspector-block">
              <span class="block-label">{tr("gui.inspector.health", "Health")}</span>
              <div class="health-score">
                <strong>{currentArchive ? tr("gui.state.ready", "Ready") : tr("gui.state.idle", "Idle")}</strong>
                <span>{currentArchive ? tr("gui.archive.zip_slip_guard_on", "Zip Slip guard on") : openArchiveFirstLabel()}</span>
              </div>
              <progress
                class="meter meter-progress"
                value={currentArchive ? 84 : 0}
                max="100"
                aria-label={tr("gui.inspector.health", "Health")}
              ></progress>
            </div>

            <div class="inspector-block">
              <span class="block-label">{tr("gui.inspector.archive", "Archive")}</span>
              <dl>
                <div><dt>{tr("gui.archive.format", "Format")}</dt><dd>{currentArchive ? archiveFormat() : tr("common.none", "None")}</dd></div>
                <div><dt>{tr("gui.table.entries", "Entries")}</dt><dd>{currentArchive ? currentArchive.entry_count.toLocaleString() : "0"}</dd></div>
                <div><dt>{tr("gui.archive.encoding", "Encoding")}</dt><dd>{currentArchive ? extractEncodingLabel() : tr("gui.archive.open_first", "Open first")}</dd></div>
                <div><dt>{tr("gui.archive.volumes", "Volumes")}</dt><dd>{currentArchive ? (currentArchive.volumes?.length ? archiveVolumeCountLabel(currentArchive.volumes.length) : tr("gui.archive.single", "Single")) : "-"}</dd></div>
              </dl>
            </div>

            <div class="inspector-block recovery-inspector">
              <span class="block-label">{tr("gui.inspector.recovery", "Recovery")}</span>
              <strong>{currentArchive ? tr("gui.archive.unprotected", "Unprotected") : openArchiveFirstLabel()}</strong>
              <p>{currentArchive ? tr("gui.recovery.requires_recovery_data", "Verify can detect corruption, but repair requires PAR2 or SQZ recovery data created earlier.") : openArchiveFirstLabel()}</p>
              <div class="inline-actions">
                <button
                  disabled={!currentArchive}
                  title={archiveActionTitle(hasArchiveOpen())}
                  aria-label={labelWithDisabledReason(actionLabel("Protect"), archiveActionTitle(hasArchiveOpen()))}
                  onclick={openRecoveryConfiguration}
                >{actionLabel("Protect")}</button>
                <button
                  disabled={!currentArchive}
                  title={archiveActionTitle(hasArchiveOpen())}
                  aria-label={labelWithDisabledReason(actionLabel("Test archive"), archiveActionTitle(hasArchiveOpen()))}
                  onclick={() => void submitTestJob()}
                >{actionLabel("Test archive")}</button>
              </div>
            </div>

            <div class="inspector-block">
              <span class="block-label">{tr("gui.inspector.selection", "Selection")}</span>
              <strong>{selectedSummary()}</strong>
              <p>{currentArchive ? tr("gui.selection.actions_hint", "Extract, preview, or copy the selected files without leaving the archive.") : openArchiveFirstLabel()}</p>
              <div class="inline-actions">
                <button disabled={!canPreviewEntrySelection()} aria-busy={previewBusy()} title={previewSelectedDisabledReason()} aria-label={labelWithDisabledReason(previewActionLabel(), previewSelectedDisabledReason())} onclick={() => void submitPreviewEntry()}>{previewActionLabel()}</button>
                <button
                  disabled={!hasArchiveSelection()}
                  title={copyOutSelectedDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.action.copy_out", "Copy out"), copyOutSelectedDisabledReason())}
                  onclick={() => void submitCopyOutSelectedJob()}
                >{tr("gui.action.copy_out", "Copy out")}</button>
              </div>
            </div>
          {/if}

        </aside>
        {/if}
      </div>
    </section>
  </main>
{:else}
  <main class={`design-root classic-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`} style={customPaletteStyle()} class:drop-active={dragActive}>
    <section class="window classic-window" aria-label={tr("gui.aria.classic_archive_browser", "Squallz Classic archive browser")}>
      <header class="classic-titlebar" data-tauri-drag-region>
        <div class="classic-title">
          <AppIcon size={19} title="Squallz" />
          <strong>{titleForScreen()}</strong>
          <span>Squallz Classic</span>
        </div>
        <div class="classic-top-actions">
          <button aria-busy={archiveOpenStatus === "opening"} onclick={() => void openArchiveFromDialog()}><Icon name="folder-open" size={15} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
          {#if nestedPreview}
            <button onclick={() => void openNestedPreviewArchive()}><Icon name="folder-open" size={15} />{tr("gui.action.open_nested", "Open")}</button>
            <button onclick={() => void extractNestedPreviewArchive()}><Icon name="archive" size={15} />{tr("gui.action.extract_nested", "Extract")}</button>
          {/if}
          <button onclick={() => setScreen("settingsGeneral")}><Icon name="settings" size={15} />{navLabel("Settings")}</button>
        </div>
      </header>

      <div class="classic-commandbar" aria-label={tr("gui.aria.archive_commands", "Archive commands")}>
          {#each classicCommands as command}
            <button
              class={`cmd-${command[2]}`}
              disabled={classicCommandDisabled(command[1])}
              title={classicCommandTitle(command[1])}
              aria-label={classicCommandAriaLabel(command[1])}
              onclick={() => handleClassicCommand(command[1])}
            >
              <span><Icon name={command[0]} size={23} /></span>
              <strong>{classicCommandLabel(command[1])}</strong>
            </button>
          {/each}
        </div>

        <div class="classic-pathrow" class:has-move={screen === "browse" && hasArchiveSelection()}>
          <button class="path-button" disabled={!canGoUpArchive()} aria-label={tr("gui.nav.back", "Back")} title={canGoUpArchive() ? tr("gui.nav.back", "Back") : archiveTitle()} onclick={() => void goArchiveUp()}><Icon name="chevron-right" size={14} /></button>
          <div class="address">
            <button type="button" disabled={!currentArchive} onclick={() => void openArchiveBreadcrumb(-1)}>{archiveTitle()}</button>
            {#each archiveDirs as dir, index}
              <i>/</i><button type="button" onclick={() => void openArchiveBreadcrumb(index)}>{dir}</button>
            {/each}
          </div>
          {#if screen === "browse" && hasArchiveSelection()}
            <label class="classic-path-move">
              <span>{actionLabel("Move to")}</span>
              <input aria-label={tr("gui.move.classic_target_folder", "Classic move target folder")} bind:value={moveTargetDir} onblur={() => commitMoveTargetDir()} />
            </label>
          {/if}
          {#if screen === "extract" || screen === "batch"}
            <div class="encoding-chip accent"><Icon name="archive" size={14} />{tr("gui.extract.smart_on", "Smart extract on")}</div>
          {:else if screen === "password"}
            <div class="encoding-chip warning"><Icon name="lock" size={14} />{tr("gui.password.required", "Password required")}</div>
          {:else if screen === "conflict"}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{jobConflictPrompt ? tr("gui.conflict.review", "Conflict review") : tr("gui.conflict.none", "No conflicts")}</div>
          {:else if screen === "cannotRepair"}
            <div class="encoding-chip warning"><Icon name="shield-alert" size={14} />{recoveryFailureAvailable() ? tr("gui.recovery.repair_blocked", "Repair blocked") : tr("gui.recovery.no_failed_result", "No failed result")}</div>
          {:else if screen === "recovery"}
            <div class="encoding-chip accent"><Icon name="shield-alert" size={14} />{currentArchive ? tr("gui.recovery.ready", "Recovery ready") : openArchiveFirstLabel()}</div>
          {:else if screen === "checksum"}
            <div class="encoding-chip accent"><Icon name="check-circle" size={14} />{checksumAlgorithmLabel(checksumAlgorithm)}</div>
          {:else if screen === "duplicates"}
            <div class="encoding-chip accent"><Icon name="search" size={14} />{tr("gui.duplicates.blake3_scan", "BLAKE3 scan")}</div>
          {:else if currentArchive && hasEncodingWarning()}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{tr("gui.encoding.gbk_suggested", "GBK suggested")}</div>
          {:else if currentArchive}
            <div class="encoding-chip accent"><Icon name="archive" size={14} />{tr("gui.archive.open", "Archive open")}</div>
          {:else}
            <div class="encoding-chip accent"><Icon name="folder-open" size={14} />{openArchiveFirstLabel()}</div>
          {/if}
          <button
            bind:this={quickActionButton}
            class="classic-search classic-search-trigger"
            aria-haspopup="dialog"
            aria-expanded={activePopover === "quickActions"}
            onclick={toggleQuickActions}
          ><Icon name="search" size={14} /><span>{tr("gui.quick.title", "Quick actions")}</span></button>
      </div>

      {#if activePopover === "quickActions"}
        <div bind:this={quickActionPopover} class="quick-popover classic-quick-popover" role="dialog" aria-label={tr("gui.quick.title", "Quick actions")}>
          <div class="quick-popover-head">
            <strong>{tr("gui.quick.title", "Quick actions")}</strong>
            <span>{tr("gui.quick.close_hint", "Esc or outside click closes")}</span>
          </div>
          {#each quickActions as action}
            <button onclick={() => chooseQuickAction(action.screen)}>
              <Icon name={action.icon} size={15} />
              <span><strong>{quickActionLabel(action.label)}</strong><small>{quickActionDetail(action.label, action.detail)}</small></span>
            </button>
          {/each}
        </div>
      {/if}

      {#if screen === "archiveInfo"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-info">
            <header>
              <div>
                <h1>{tr("gui.archive.info_title", "Archive information")}</h1>
                <p>{tr("gui.archive.info_subtitle", "Current archive, selection, extraction target, encoding, and volume state.")}</p>
              </div>
              <div class="classic-button-row">
                <button onclick={() => setScreen("browse")}>{tr("gui.nav.back_to_archive", "Back to archive")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(extractArchiveRequiredReason())}
                  title={extractArchiveRequiredReason()}
                  aria-label={labelWithDisabledReason(tr("gui.screen.extract", "Extract"), extractArchiveRequiredReason())}
                  onclick={() => setScreen("extract")}
                >{tr("gui.screen.extract", "Extract")}</button>
              </div>
            </header>
            <div class="classic-batch-grid">
              <section>
                <h2>{tr("gui.inspector.archive", "Archive")}</h2>
                <div class="classic-form-grid compact">
                  {#each archiveInfoRows() as row}
                    <div class="classic-label">{row[0]}</div>
                    <div class="classic-input">{row[1]}</div>
                  {/each}
                </div>
              </section>
              <aside>
                <h2>{tr("gui.extract.final_destination", "Final destination")}</h2>
                <div class="classic-form-grid compact no-pad">
                  <div class="classic-label">{tr("common.mode", "Mode")}</div>
                  <div class="classic-input accent">{extractDestinationTitle(extractDestinationMode)}</div>
                  <div class="classic-label">{tr("common.destination", "Destination")}</div>
                  <div class="classic-input">{effectiveExtractDest()}</div>
                </div>
              </aside>
            </div>
          </section>
        </div>
      {:else if screen === "create"}
        <div class="classic-dialog-body">
          <section class="classic-property-sheet">
            <header>
              <div>
                <h1>{tr("gui.create.add_to_archive", "Add to archive")}</h1>
                <p>{tr("gui.create.classic_intro", "Edit rules first; source preflight runs before the task starts.")}</p>
              </div>
              <div class="classic-button-row">
                <button disabled={createPreflightBusy()} onclick={() => void submitCreateJob("files")}>{createPreflightBusy() ? tr("gui.create.checking", "Checking") : tr("gui.create.files", "Files")}</button>
                <button class="classic-primary" disabled={createPreflightBusy()} onclick={() => void submitCreateJob("folder")}>{createPreflightBusy() ? tr("gui.create.checking", "Checking") : tr("gui.create.folder", "Folder")}</button>
              </div>
            </header>
            {#if createDropInputs.length > 0}
              <div class="classic-drop-summary">
                <Icon name="archive" size={15} />
                <span>{tr("gui.create.dropped_sources", "Dropped sources")}</span>
                <strong>{droppedSourceLabel()}</strong>
              </div>
            {/if}

            <div class="classic-tabs">
              <span class="active">{settingsSectionLabel("General")}</span><span>{tr("gui.extract.advanced", "Advanced")}</span><span>{tr("gui.extract.files", "Files")}</span><span>{settingsSectionLabel("Security")}</span><span>{tr("gui.recovery.title", "Recovery")}</span><span>{tr("gui.create.comment", "Comment")}</span>
            </div>

            <div class="classic-form-grid">
              <div class="classic-label">{tr("gui.create.archive_name", "Archive name")}</div><div class="classic-input">{createArchivePreviewName()}</div>
              <div class="classic-label">{tr("gui.create.archive_format", "Archive format")}</div>
              <div class="classic-segments" aria-label={tr("gui.create.classic_archive_format", "Classic archive format")}>
                {#each createFormatIds as formatId}
                  <button
                    class:active={activeCreateFormat === formatId}
                    aria-pressed={activeCreateFormat === formatId}
                    title={createFormatNoteFor(formatId)}
                    onclick={() => chooseCreateFormat(formatId)}
                  >{createFormats[formatId].label}</button>
                {/each}
                <span class="format-boundary-pill" role="note" title={tr("gui.create.rar_not_launch_claim", "RAR creation is not a launch claim")}>{tr("gui.create.rar_read_only", "RAR read only")}</span>
              </div>
              <div class="classic-label">{tr("gui.create.format_boundary", "Format boundary")}</div><div class="classic-input accent">{createFormatNote()}</div>
              <div class="classic-label">{tr("gui.create.compression_profile", "Compression profile")}</div>
              <div class="classic-segments classic-profile-segments">
                {#each createProfileIds as profileId}
                  <button
                    class:active={activeCreateProfile === profileId}
                    aria-pressed={activeCreateProfile === profileId}
                    onclick={() => chooseCreateProfile(profileId)}
                  >{createProfileLabel(profileId)}</button>
                {/each}
              </div>
              <div class="classic-label">{tr("gui.create.compression_method", "Compression method")}</div><div class="classic-input">{createMethodLabel()}</div>
              {#if activeCreateProfile === "custom"}
                <div class="classic-label">{tr("gui.create.custom_level", "Custom level")}</div>
                <div class="classic-input classic-custom-level">
                  <input
                    type="range"
                    min="1"
                    max="9"
                    value={customCreateLevel}
                    aria-label={tr("gui.create.classic_custom_compression_level", "Classic custom compression level")}
                    oninput={(event) => updateCustomCreateLevelFromInput(event)}
                    onchange={(event) => updateCustomCreateLevelFromInput(event, true)}
                  />
                  <input
                    type="number"
                    class:invalid={customCreateLevelError.length > 0}
                    min="1"
                    max="9"
                    step="1"
                    inputmode="numeric"
                    value={customCreateLevel}
                    aria-label={tr("gui.create.classic_custom_compression_level_number", "Classic custom compression level number")}
                    aria-invalid={customCreateLevelError ? "true" : "false"}
                    aria-describedby={customCreateLevelError ? "custom-create-level-error-classic" : undefined}
                    oninput={(event) => updateCustomCreateLevelFromInput(event)}
                    onchange={(event) => updateCustomCreateLevelFromInput(event, true)}
                  />
                </div>
                {#if customCreateLevelError}
                  <div></div>
                  <small id="custom-create-level-error-classic" class="classic-input custom-level-error" role="status" data-custom-level-error>{customCreateLevelError}</small>
                {/if}
                <div class="classic-label">{tr("gui.create.custom_profiles", "Custom profiles")}</div>
                <div class="classic-input classic-custom-profiles">
                  <label>
                    <span>{tr("common.name", "Name")}</span>
                    <input
                      aria-label={tr("gui.create.classic_custom_profile_name", "Classic custom profile name")}
                      class:invalid={customCreateProfileNameError.length > 0}
                      value={customCreateProfileName}
                      aria-invalid={customCreateProfileNameError ? "true" : "false"}
                      aria-describedby={customCreateProfileNameError ? "custom-create-profile-name-error-classic" : undefined}
                      oninput={updateCustomCreateProfileNameFromInput}
                    />
                  </label>
                  {#if customCreateProfileNameError}
                    <small id="custom-create-profile-name-error-classic" class="custom-profile-name-error" role="status" data-custom-profile-name-error>{customCreateProfileNameError}</small>
                  {/if}
                  <div class="custom-profile-list compact" aria-label={tr("gui.create.classic_saved_custom_profiles", "Classic saved custom profiles")}>
                    {#each customCreateProfiles as profile}
                      <button
                        class:active={profile.id === activeCustomCreateProfileId}
                        aria-pressed={profile.id === activeCustomCreateProfileId}
                        onclick={() => chooseCustomCreateProfile(profile.id)}
                      ><strong>{profile.name}</strong><span>L{profile.level}</span></button>
                    {/each}
                  </div>
                  <div class="custom-profile-actions">
                    <button onclick={saveActiveCustomCreateProfile}>{tr("gui.create.save_profile", "Save profile")}</button>
                    <button
                      onclick={createNewCustomCreateProfile}
                      disabled={customCreateProfiles.length >= maxCustomCreateProfiles}
                      title={customProfileSaveAsNewTitle()}
                      aria-label={`${tr("gui.create.save_as_new", "Save as new")}${customProfileSaveAsNewTitle() ? ` · ${customProfileSaveAsNewTitle()}` : ""}`}
                    >{tr("gui.create.save_as_new", "Save as new")}</button>
                    <button onclick={deleteActiveCustomCreateProfile} disabled={customCreateProfiles.length <= 1} title={customProfileDeleteTitle()}>{tr("common.delete", "Delete")}</button>
                  </div>
                  {#if customCreateProfiles.length >= maxCustomCreateProfiles}
                    <small class="custom-profile-limit" role="status">{customProfileLimitMessage()}</small>
                  {/if}
                </div>
              {/if}
              <div class="classic-label">{tr("gui.create.split_to_volumes", "Split to volumes")}</div><div class="classic-input accent">{createVolumePreview()}</div>
              <div class="classic-label">{tr("gui.recovery.title", "Recovery")}</div><div class="classic-input accent">{createRecoveryCapability()}</div>
              <div class="classic-label">{tr("gui.create.update_mode", "Update mode")}</div><div class="classic-input">{tr("gui.create.add_and_replace_files", "Add and replace files")}</div>
              <div class="classic-label">{tr("gui.create.password", "Password")}</div><div class="classic-input" class:accent={createPasswordDataAvailable()} data-capability="password-data-encryption">{createPasswordCapability()}</div>
              <div class="classic-label">{tr("gui.create.name_encryption", "Name encryption")}</div><div class="classic-input" class:accent={createNameEncryptionAvailable()} class:muted={!createNameEncryptionAvailable()} data-capability="name-encryption" title={createNameEncryptionCapability()}>{createNameEncryptionCapability()}</div>
              <div class="classic-label">{tr("gui.create.excludes", "Exclude")}</div><textarea class="classic-input classic-textarea" rows="3" bind:value={createExcludeText} aria-label={tr("gui.create.classic_exclude_glob_rules", "Classic exclude glob rules")}></textarea>
              <div class="classic-label">{tr("gui.create.input_preflight", "Input preflight")}</div><div class="classic-input accent">{createEstimateStatusbar()}</div>
              <div class="classic-label">{tr("gui.create.temp_preflight", "Temp preflight")}</div><div class="classic-input accent">{tempPreflightStatusbar()}</div>
              <div class="classic-label">{tr("gui.create.disk_preflight", "Disk preflight")}</div><div class="classic-input accent">{diskPreflightStatusbar()}</div>
            </div>

            <div class="classic-capability-grid">
              {#each featuredFormatCards() as format}
                <div>
                  <strong>{format.name}</strong>
                  <span>{format.state}</span>
                  <small>{tr("gui.format.card_capability_line", "Create {create} · Split {split} · {encrypt}").replace("{create}", format.create).replace("{split}", format.split).replace("{encrypt}", format.encrypt)}</small>
                  <em>{format.note}</em>
                </div>
              {/each}
            </div>
          </section>
        </div>
      {:else if screen === "extract"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-extract">
            <header>
              <div>
                <h1>{classicCommandLabel("Extract To")}</h1>
                <p>{tr("gui.extract.classic_subtitle", "Choose the final folder, preview smart extract behavior, and review conflicts before writing files.")}</p>
              </div>
              <div class="classic-button-row">
                <button onclick={() => setScreen("batch")}>{tr("gui.extract.batch_review", "Batch review")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(extractArchiveRequiredReason())}
                  title={currentArchive ? extractDestinationHint() : extractArchiveRequiredReason()}
                  aria-label={currentArchive ? tr("gui.extract.start", "Extract") : labelWithDisabledReason(tr("gui.extract.start", "Extract"), extractArchiveRequiredReason())}
                  onclick={() => void submitExtractJob()}
                >{tr("gui.extract.start", "Extract")}</button>
              </div>
            </header>

            <div class="classic-tabs">
              <span class="active">{settingsSectionLabel("General")}</span><span>{tr("gui.extract.advanced", "Advanced")}</span><span>{tr("gui.extract.files", "Files")}</span><span>{settingsSectionLabel("Security")}</span><span>{tr("gui.extract.log", "Log")}</span>
            </div>

            <div class="classic-extract-grid">
              <section class="classic-extract-form">
                <h2>{tr("gui.batch.destination", "Destination")}</h2>
                <div class="classic-form-grid compact">
                  <div class="classic-label">{tr("gui.inspector.archive", "Archive")}</div><div class="classic-input">{archiveLine()}</div>
                  <div class="classic-label">{tr("common.selection", "Selection")}</div><div class="classic-input accent">{extractSelectionLabel()}</div>
                  <div class="classic-label">{tr("gui.batch.destination", "Destination")}</div><div class="classic-input accent">{effectiveExtractDest()}</div>
                  <div class="classic-label">{tr("common.mode", "Mode")}</div><div class="classic-segments">
                    {#each extractDestinationModes as mode}
                      <button
                        class:active={extractDestinationMode === mode}
                        disabled={Boolean(extractArchiveRequiredReason())}
                        title={extractArchiveRequiredReason()}
                        aria-label={labelWithDisabledReason(extractDestinationTitle(mode), extractArchiveRequiredReason())}
                        onclick={() => void selectExtractDestination(mode)}
                      >{extractDestinationTitle(mode)}</button>
                    {/each}
                  </div>
                  <div class="classic-label">{tr("gui.extract.conflicts", "Conflicts")}</div><div class="classic-segments">
                    {#each extractOverwriteModes as mode}
                      <button
                        class:active={extractOverwriteMode === mode}
                        aria-pressed={extractOverwriteMode === mode}
                        onclick={() => selectExtractOverwrite(mode)}
                      >{extractOverwriteLabel(mode)}</button>
                    {/each}
                  </div>
                  <div class="classic-label">{tr("gui.archive.encoding", "Encoding")}</div><div class="classic-input">{extractEncodingLabel()}</div>
                  <div class="classic-label">{tr("gui.extract.safety", "Safety")}</div><div class="classic-input accent">{tr("gui.extract.safety_blocked", "Zip Slip, bomb ratio, reserved names, symlink escape blocked")}</div>
                </div>
                <div class="classic-extract-actions">
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.extract.password_prompt", "Password prompt"), extractArchiveRequiredReason())}
                    onclick={() => setScreen("password")}
                  ><Icon name="lock" size={15} />{tr("gui.extract.password_prompt", "Password prompt")}</button>
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.extract.conflict_preview", "Conflict preview"), extractArchiveRequiredReason())}
                    onclick={() => setScreen("conflict")}
                  ><Icon name="alert-triangle" size={15} />{tr("gui.extract.conflict_preview", "Conflict preview")}</button>
                  <button
                    disabled={Boolean(extractArchiveRequiredReason())}
                    title={extractArchiveRequiredReason()}
                    aria-label={labelWithDisabledReason(tr("gui.extract.test_first", "Test first"), extractArchiveRequiredReason())}
                    onclick={() => void submitTestJob()}
                  ><Icon name="check-circle" size={15} />{tr("gui.extract.test_first", "Test first")}</button>
                </div>
              </section>

              <aside class="classic-extract-preview">
                <h2>{tr("gui.extract.write_preview", "Write preview")}</h2>
                <div class="classic-preview-table">
                  <div><b>{tr("gui.security.entry", "Entry involved")}</b><b>{tr("common.target", "Target")}</b><b>{tr("common.status", "Status")}</b></div>
                  {#each browseEntries(CLASSIC_ROW_HEIGHT).slice(0, 3) as entry}
                    <div><span>{entry.source?.path ?? entry.name}</span><span>{effectiveExtractDest()}</span><strong>{entry.type === "warning" ? tr("gui.extract.encoding_review", "Encoding review") : entry.type === "locked" ? tr("gui.extract.password", "Password") : tr("gui.state.ready", "Ready")}</strong></div>
                  {:else}
                    <div><span>{openArchiveFirstLabel()}</span><span>-</span><strong>{noEntriesLabel()}</strong></div>
                  {/each}
                </div>
                <div class="classic-mode-note">
                  <strong>{tr("gui.extract.smart_deterministic", "Smart extract is deterministic.")}</strong>
                  <span>{tr("gui.extract.smart_deterministic_body", "Mixed archive roots get a containing folder; a single root folder extracts directly.")}</span>
                </div>
              </aside>
            </div>
          </section>
        </div>
      {:else if screen === "batch"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-batch">
            <header>
              <div>
                <h1>{tr("gui.batch.review", "Batch Extract Review")}</h1>
                <p>{tr("gui.batch.classic_subtitle", "Review every archive, target folder, password state, and blocked item before tasks start.")}</p>
              </div>
              <div class="classic-button-row">
                <button onclick={() => setScreen("extract")}>{tr("gui.nav.back", "Back")}</button>
              <button class="classic-primary" disabled={batchReviewArchives().length === 0} onclick={() => void startBatchExtract()}>{tr("gui.batch.start_batch", "Start batch")}</button>
              </div>
            </header>

            <div class="classic-batch-grid">
              <section>
                <h2>{navLabel("Archives")}</h2>
                <div class="classic-batch-table">
                  <div><b>{tr("gui.inspector.archive", "Archive")}</b><b>{tr("common.format", "Format")}</b><b>{tr("gui.table.entries", "Entries")}</b><b>{tr("common.target", "Target")}</b><b>{tr("common.status", "Status")}</b></div>
                  {#each batchReviewArchives() as archive}
                    <div class:warning={archive.state === "Needs password"}>
                      <strong>{archive.name}</strong><span>{archive.format}</span><span>{archive.entries}</span><span>{archive.target}</span><em>{batchArchiveStateLabel(archive.state)}</em>
                    </div>
                  {:else}
                    <div>
                      <strong>{openArchiveFirstLabel()}</strong><span>-</span><span>0</span><span>-</span><em>{tr("gui.batch.no_archives_queued", "No archives selected")}</em>
                    </div>
                  {/each}
                </div>
              </section>
              <aside>
                <h2>{tr("gui.batch.policy", "Batch policy")}</h2>
                <div class="classic-form-grid compact no-pad">
                  <div class="classic-label">{tr("gui.batch.target_rule", "Target rule")}</div><div class="classic-input accent">{tr("gui.batch.each_archive_folder", "Each archive folder")}</div>
                  <div class="classic-label">{tr("gui.extract.smart_mode", "Smart extract")}</div><div class="classic-input">{tr("gui.batch.smart_per_archive", "On · per archive root analysis")}</div>
                  <div class="classic-label">{tr("gui.extract.conflicts", "Conflicts")}</div><div class="classic-input">{tr("gui.batch.ask_before_replace", "Ask before replace")}</div>
                  <div class="classic-label">{tr("gui.batch.failure_mode", "Failure mode")}</div><div class="classic-input accent">{tr("gui.batch.continue_ready_hold_blocked", "Continue ready archives, hold blocked archive")}</div>
                </div>
	                <button class="classic-color-route" onclick={() => setScreen("password")}><Icon name="lock" size={15} />{tr("gui.batch.resolve_missing_password", "Resolve missing password")}</button>
	              </aside>
	            </div>
	          </section>
	        </div>
	      {:else if screen === "checksum"}
	        <div class="classic-dialog-body" class:with-archive-return={showArchiveReturnBar()}>
	          {#if showArchiveReturnBar()}
	            <ArchiveReturnStrip
	              title={archiveTitle()}
	              detail={archiveReturnDetail()}
	              contextLabel={tr("gui.archive.current_context", "Current archive")}
	              actionLabel={tr("gui.archive.back_to_current", "Back to current archive")}
	              buttonClass="classic-primary"
	              iconSize={15}
	              onReturn={returnToCurrentArchive}
	            />
	          {/if}
	          <section class="classic-extract-sheet classic-checksum">
	            <header>
	              <div>
	                <h1>{tr("gui.screen.checksum", "Checksum")}</h1>
	                <p>{tr("gui.checksum.subtitle", "Calculate local file digests or verify a manifest with the same engine exposed by sqz checksum.")}</p>
	              </div>
	              <div class="classic-button-row">
	                <button onclick={() => void chooseChecksumFile()}>{tr("gui.checksum.choose_file", "Choose file")}</button>
	                <button onclick={() => void chooseChecksumFolder()}>{tr("gui.checksum.choose_folder", "Choose folder")}</button>
	                <button class="classic-primary" onclick={() => void submitChecksumJob()}>{tr("gui.checksum.calculate", "Calculate checksum")}</button>
	              </div>
	            </header>

	            <div class="classic-batch-grid">
	              <section>
	                <h2>{tr("gui.checksum.calculate_title", "Calculate")}</h2>
	                <div class="classic-form-grid compact">
	                  <div class="classic-label">{tr("common.target", "Target")}</div><div class="classic-input accent">{checksumTargetLabel()}</div>
	                  <div class="classic-label">{tr("gui.checksum.algorithm", "Algorithm")}</div>
		                  <ChecksumAlgorithmPicker
		                    algorithms={checksumAlgorithms}
		                    selected={checksumAlgorithm}
		                    labelFor={checksumAlgorithmLabel}
		                    hintFor={checksumAlgorithmHint}
		                    onSelect={selectChecksumAlgorithm}
		                    className="classic-algorithm-grid"
		                  />
	                  <div class="classic-label">{tr("gui.create.excludes", "Excludes")}</div><textarea class="classic-input" rows="4" bind:value={checksumExcludeText} aria-label={tr("gui.checksum.exclude_rules", "Checksum exclude rules")}></textarea>
	                </div>
	                <button class="classic-color-route" onclick={useCurrentArchiveForChecksum}><Icon name="archive" size={15} />{tr("gui.checksum.use_current_archive", "Use current archive")}</button>
	              </section>
	              <aside>
	                <h2>{tr("gui.checksum.verify_manifest", "Verify manifest")}</h2>
	                <div class="classic-form-grid compact no-pad">
	                  <div class="classic-label">{tr("gui.checksum.manifest", "Manifest")}</div><div class="classic-input">{checksumManifestLabel()}</div>
	                  <div class="classic-label">{tr("gui.checksum.passed", "Passed")}</div><div class="classic-input success">{checksumResultNumber("checksum_check", "passed").toLocaleString()}</div>
	                  <div class="classic-label">{tr("gui.checksum.failed", "Failed")}</div><div class="classic-input danger">{checksumResultNumber("checksum_check", "failed").toLocaleString()}</div>
	                  <div class="classic-label">{tr("gui.checksum.checked", "Checked")}</div><div class="classic-input">{checksumResultNumber("checksum_check", "checked").toLocaleString()}</div>
	                </div>
	                <div class="classic-button-row checksum-manifest-actions">
	                  <button onclick={() => void chooseChecksumManifest()}>{tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
	                  <button class="classic-primary" onclick={() => void submitChecksumCheckJob()}>{tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
	                </div>
	              </aside>
	            </div>

	            <section
	              class="checksum-result-panel classic-checksum-result-panel"
	              bind:this={checksumResultPanel}
	              tabindex="-1"
	              aria-label={tr("gui.checksum.result", "Checksum result")}
	            >
	              <div class="checksum-result-actions">
	                <div class="checksum-result-title">
	                  <strong>{tr("gui.checksum.result", "Checksum result")}</strong>
	                  <span>{tr("gui.checksum.result_rows", "{count} rows").replace("{count}", checksumItems("checksum").length.toLocaleString())}</span>
	                </div>
	                <div class="checksum-result-copy">
	                  {#if checksumCopyFeedbackFor("checksum")}
	                    <span class="checksum-copy-status" class:danger={checksumCopyFeedbackToneFor("checksum") === "danger"} role="status">{checksumCopyFeedbackFor("checksum")}</span>
	                  {/if}
	                  <button type="button" class="classic-primary" disabled={checksumItems("checksum").length === 0} onclick={() => void copyChecksumResults("checksum")}>{tr("gui.checksum.copy_results", "Copy results")}</button>
	                </div>
	              </div>
	              <div class="classic-form-grid compact checksum-result-summary">
	                <div class="classic-label">{tr("gui.checksum.latest_files", "Latest files")}</div><div class="classic-input">{checksumResultNumber("checksum", "files_hashed").toLocaleString()}</div>
	                <div class="classic-label">{tr("gui.checksum.latest_bytes", "Latest bytes")}</div><div class="classic-input">{formatBytes(checksumResultNumber("checksum", "bytes_hashed"))}</div>
	                <div class="classic-label">{tr("gui.checksum.latest_state", "Latest state")}</div><div class="classic-input accent">{taskStateLabel(latestChecksumTask("checksum")?.state)}</div>
	              </div>
	              <div class="classic-checksum-table">
	                <div><b>{tr("gui.checksum.result", "Checksum result")}</b><b>{tr("gui.checksum.digest", "Digest")}</b><b>{tr("common.status", "Status")}</b></div>
	                {#each checksumItems("checksum").slice(0, 20) as item}
	                  <div><span>{pathBaseName(checksumItemText(item, "path")) || checksumItemText(item, "path")}</span><code class="checksum-digest">{checksumItemText(item, "digest")}</code><strong>{checksumItemStatus(item)}</strong></div>
	                {:else}
	                  <div><span>{tr("gui.checksum.no_result_yet", "No checksum result yet")}</span><code>-</code><strong>{taskStateLabel(latestChecksumTask("checksum")?.state)}</strong></div>
	                {/each}
	              </div>
	            </section>
	          </section>
	        </div>
	      {:else if screen === "duplicates"}
	        <div class="classic-dialog-body" class:with-archive-return={showArchiveReturnBar()}>
	          {#if showArchiveReturnBar()}
	            <ArchiveReturnStrip
	              title={archiveTitle()}
	              detail={archiveReturnDetail()}
	              contextLabel={tr("gui.archive.current_context", "Current archive")}
	              actionLabel={tr("gui.archive.back_to_current", "Back to current archive")}
	              buttonClass="classic-primary"
	              iconSize={15}
	              onReturn={returnToCurrentArchive}
	            />
	          {/if}
	          <section class="classic-extract-sheet classic-duplicates">
	            <header>
		              <div>
		                <h1>{tr("gui.screen.duplicates", "Duplicate Finder")}</h1>
		                <p>{tr("gui.duplicates.subtitle", "Scan local folders with the same BLAKE3 duplicate detector exposed by sqz duplicates; no cleanup action is run.")}</p>
		                <div class="duplicate-safety-strip classic-duplicate-safety" aria-label={tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
		                  <span><Icon name="search" size={13} />{tr("gui.duplicates.cli_contract", "CLI parity: sqz duplicates")}</span>
		                  <span><Icon name="list" size={13} />{tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
		                  <span><Icon name="check-circle" size={13} />{tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
		                </div>
		              </div>
		              <div class="classic-button-row">
		                <button onclick={() => void chooseDuplicateScanFolder()}>{tr("gui.checksum.choose_folder", "Choose folder")}</button>
		                <button onclick={useCurrentArchiveFolderForDuplicates}><Icon name="archive" size={15} />{tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
		                <button class="classic-primary" onclick={() => void submitDuplicateScanJob()}>{tr("gui.duplicates.scan", "Scan duplicates")}</button>
		              </div>
	            </header>

	            <div class="classic-batch-grid">
	              <section>
	                <h2>{tr("gui.duplicates.scan_setup", "Scan setup")}</h2>
	                <div class="classic-form-grid compact">
	                  <div class="classic-label">{tr("common.target", "Target")}</div><div class="classic-input accent">{duplicateScanTargetLabel()}</div>
	                  <div class="classic-label">{tr("gui.duplicates.min_size", "Min size")}</div>
	                  <input
	                    class="classic-input"
	                    class:invalid={duplicateMinSizeError.length > 0}
	                    type="number"
	                    min="0"
	                    step="1"
	                    value={duplicateMinSize}
	                    oninput={updateDuplicateMinSizeFromInput}
	                    aria-label={tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
	                    aria-invalid={duplicateMinSizeError ? "true" : "false"}
	                    aria-describedby={duplicateMinSizeError ? "duplicate-min-size-error-classic" : undefined}
	                  />
	                  {#if duplicateMinSizeError}
	                    <div></div>
	                    <small id="duplicate-min-size-error-classic" class="classic-input duplicate-min-size-error" role="status" data-duplicate-min-size-error>{duplicateMinSizeError}</small>
	                  {/if}
	                  <div class="classic-label">{tr("gui.create.excludes", "Excludes")}</div><textarea class="classic-input" rows="4" bind:value={duplicateExcludeText} aria-label={tr("gui.duplicates.exclude_rules", "Duplicate exclude rules")}></textarea>
	                </div>
	              </section>
	              <aside>
	                <h2>{tr("gui.duplicates.latest_result", "Latest result")}</h2>
	                <div class="classic-form-grid compact no-pad">
	                  <div class="classic-label">{tr("common.status", "State")}</div><div class="classic-input">{taskStateLabel(latestDuplicateScanTask()?.state)}</div>
	                  <div class="classic-label">{tr("gui.duplicates.files", "Files")}</div><div class="classic-input">{duplicateResultNumber("files_scanned").toLocaleString()}</div>
	                  <div class="classic-label">{tr("gui.duplicates.groups", "Groups")}</div><div class="classic-input accent">{duplicateResultNumber("duplicate_groups").toLocaleString()}</div>
	                  <div class="classic-label">{tr("gui.duplicates.reclaimable", "Reclaimable")}</div><div class="classic-input accent">{formatBytes(duplicateResultNumber("reclaimable_bytes"))}</div>
	                </div>
		              </aside>
	            </div>
	          </section>
	        </div>
      {:else if screen === "password"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-password">
            <header>
              <div>
                <h1>{tr("gui.screen.password", "Password Required")}</h1>
                <p>{tr("gui.password.prompt_boundary_body", "Unlock only the archive that requested credentials. No password is written to logs, settings, or task status.")}</p>
              </div>
              {#if jobPasswordPrompt}
                <button class="classic-primary" onclick={submitJobPassword}>{tr("gui.password.unlock", "Unlock")}</button>
              {:else}
                <button onclick={() => setScreen("extract")}>{tr("gui.nav.back_to_extract", "Back to Extract")}</button>
              {/if}
            </header>

            {#if jobPasswordPrompt}
              <div class="classic-password-grid">
                <section class="classic-password-panel">
                  <h2>{passwordPromptName()}</h2>
                  <div class="classic-form-grid compact">
                    <div class="classic-label">{tr("gui.password.password", "Password")}</div><input class="classic-input password-obscured" type="password" bind:value={jobPasswordValue} autocomplete="current-password" aria-label={tr("gui.password.archive_password", "Archive password")} />
                    <div class="classic-label">{tr("gui.password.remember_short", "Remember")}</div><div class="classic-input">{tr("gui.password.session_only_book_flow", "Session only · Password Book saves through unlock flow")}</div>
                    <div class="classic-label">{tr("gui.password.fallback", "Fallback")}</div><div class="classic-input">{tr("gui.password.manual_overrides_saved", "Manual input overrides saved password")}</div>
                    <div class="classic-label">{tr("gui.password.on_failure", "On failure")}</div><div class="classic-input accent">{tr("gui.password.return_to_prompt", "Return to prompt, do not fail whole batch")}</div>
                  </div>
                  <div class="classic-extract-actions">
                    <button onclick={cancelJobPassword}>{tr("common.cancel", "Cancel")}</button>
                    <button onclick={() => void forgetPasswordBookPanel()}>{tr("gui.settings.password_book.forget_saved", "Forget saved password")}</button>
                  </div>
                </section>
                <aside class="classic-password-panel">
                  <h2>{tr("gui.password.security_boundary", "Security boundary")}</h2>
                  <div class="classic-mode-note no-margin">
                    <strong>{tr("gui.password.frontend_never_owns_saved", "Frontend never owns saved secrets.")}</strong>
                    <span>{tr("gui.password.secret_store_supplies_directly", "It only shows available/saved status; the system secret store supplies passwords directly to archive operations.")}</span>
                  </div>
                  <div class="repair-log">
                    <span>{tr("gui.password.manual_transient", "Manual password: user-entered, transient.")}</span>
                    <span>{tr("gui.password.session_zeroize", "Session cache: memory cleared after use.")}</span>
                    <span>{tr("gui.password.keychain_opt_in", "{secretStore}: opt-in, per archive account.").replace("{secretStore}", secretStoreLabel())}</span>
                  </div>
                </aside>
              </div>
            {:else}
              <div class="classic-mode-note classic-task-empty">
                <strong>{tr("gui.password.no_active_request", "No password request is active")}</strong>
                <span>{tr("gui.password.no_active_request_body", "Password entry appears only when an extract or test task asks for credentials.")}</span>
              </div>
            {/if}
          </section>
        </div>
      {:else if screen === "conflict"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-conflict">
	            <header>
              <div>
                <h1>{tr("gui.screen.conflict", "Conflict Handling")}</h1>
                <p>{conflictPromptDetail()}</p>
              </div>
              <div class="classic-button-row">
                {#if jobConflictPrompt}
                  <button onclick={cancelConflictPrompt}>{tr("gui.conflict.skip", "Skip")}</button>
                  <button class="classic-primary" onclick={() => answerConflictDecision("rename", false)}>{tr("gui.conflict.rename", "Keep both")}</button>
                {:else}
                  <button onclick={() => setScreen("extract")}>{tr("gui.nav.back_to_extract", "Back to Extract")}</button>
                {/if}
              </div>
            </header>

            {#if jobConflictPrompt}
              <div class="classic-conflict-grid">
                <section>
                  <h2>{tr("gui.conflict.existing_files", "Existing files")}</h2>
                  <div class="classic-conflict-table">
                    <div><b>{tr("common.path", "Path")}</b><b>{tr("gui.conflict.existing", "Existing")}</b><b>{tr("gui.conflict.incoming", "Incoming")}</b><b>{tr("gui.conflict.decision", "Decision")}</b></div>
                    {#each conflictRowsView() as row}
                      <div><strong>{row.path}</strong><span>{row.existing}</span><span>{row.incoming}</span><span class="decision-pill">{conflictDecisionLabel(row.decision)}</span></div>
                    {/each}
                  </div>
                </section>
                <aside>
                  <h2>{tr("gui.conflict.policy", "Policy")}</h2>
                  <div class="classic-segments conflict-policy">
                    <button onclick={() => answerConflictDecision("skip", false)}>{tr("gui.conflict.skip", "Skip")}</button><button onclick={() => answerConflictDecision("overwrite", false)}>{tr("gui.conflict.overwrite", "Replace")}</button><button class="active" onclick={() => answerConflictDecision("rename", false)}>{tr("gui.conflict.rename", "Keep both")}</button><span class="format-boundary-pill" role="note">{tr("gui.extract.overwrite.ask", "Ask")}</span>
                  </div>
                  <div class="classic-mode-note no-margin">
                    <strong>{tr("gui.conflict.apply_all_explicit", "Apply to all is explicit.")}</strong>
                    <span>{tr("gui.conflict.dialog_boundary_body", "The decision never silently escapes this dialog; batch jobs preserve per-archive conflict state.")}</span>
                  </div>
                  <button class="classic-color-route" onclick={() => answerConflictDecision("rename", true)}><Icon name="check-circle" size={15} />{tr("gui.conflict.keep_both_all", "Keep both for all conflicts")}</button>
                </aside>
              </div>
            {:else}
              <div class="classic-mode-note classic-task-empty">
                <strong>{tr("gui.conflict.no_active_request", "No conflict request is active")}</strong>
                <span>{tr("gui.conflict.no_active_request_body", "Conflict choices appear only when an extract task finds an existing file.")}</span>
              </div>
            {/if}
	          </section>
        </div>
      {:else if screen === "cannotRepair"}
        <div class="classic-dialog-body">
          <section class="classic-extract-sheet classic-cannot-repair">
            <header>
	              <div>
	                <h1>{tr("gui.screen.cannot_repair", "Recovery Limit")}</h1>
	                <p>{recoveryFailureAvailable() ? tr("gui.recovery.full_repair_blocked_body", "Full repair is blocked because damaged or missing blocks exceed available recovery data.") : tr("gui.recovery.no_failed_result_loaded", "No failed recovery result is loaded.")}</p>
	              </div>
	              <div class="classic-button-row">
	                <button onclick={() => setScreen("recovery")}>{tr("gui.nav.back", "Back")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(recoveryFailureDisabledReason())}
                  title={recoveryFailureDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.partial_extract", "Partial extract"), recoveryFailureDisabledReason())}
                  onclick={() => void submitBestEffortExtractJob()}
                >{tr("gui.recovery.partial_extract", "Partial extract")}</button>
	              </div>
	            </header>

            <div class="classic-repair-limit-grid">
              <section>
	                <h2>{tr("gui.recovery.block_math", "Block math")}</h2>
	                <div class="repair-summary danger">
	                  <strong>{recoveryFailureAvailable() ? tr("gui.recovery.not_repairable", "Not repairable") : tr("gui.recovery.no_result", "No result")}</strong>
	                  <span>{recoveryFailureAvailable() ? tr("gui.recovery.damage_over_capacity", "37 damaged blocks > 24 recovery blocks") : tr("gui.recovery.run_verify_before_failure", "Run Verify before reporting failure.")}</span>
	                </div>
	                <div class="block-table">
	                  <div><b>{tr("gui.recovery.group", "Group")}</b><b>{tr("gui.recovery.data", "Data")}</b><b>{tr("gui.recovery.recovery_blocks", "Recovery")}</b><b>{tr("gui.recovery.damaged", "Damaged")}</b><b>{tr("common.status", "Status")}</b></div>
	                  {#if recoveryFailureAvailable()}
	                    <div><span>G1</span><span>192</span><span>12</span><span>18</span><strong>{tr("gui.recovery.over_limit", "Over limit")}</strong></div>
	                    <div><span>G2</span><span>188</span><span>12</span><span>19</span><strong>{tr("gui.recovery.over_limit", "Over limit")}</strong></div>
	                  {:else}
	                    <div><span>-</span><span>-</span><span>-</span><span>-</span><strong>{tr("gui.recovery.no_result", "No result")}</strong></div>
	                  {/if}
	                </div>
	                <div class="repair-log">
	                  <span>{recoveryFailureAvailable() ? tr("gui.recovery.full_repair_blocked_safe", "Full repair blocked. No destructive write will start.") : tr("gui.recovery.no_failure_result_loaded", "No recovery failure result is loaded.")}</span>
	                  <span>{recoveryFailureAvailable() ? tr("gui.recovery.readable_entries_partial", "Readable entries can be listed and extracted to a separate folder.") : tr("gui.recovery.open_and_verify_first", "Open an archive and run recovery verification first.")}</span>
	                </div>
	              </section>
              <aside>
                <h2>{tr("gui.recovery.allowed_actions", "Allowed actions")}</h2>
                <div class="classic-form-grid compact no-pad">
	                  <div class="classic-label">{tr("gui.recovery.full_repair", "Full repair")}</div><div class="classic-input danger">{tr("common.unavailable", "Unavailable")}</div>
	                  <div class="classic-label">{tr("gui.recovery.partial_extract", "Partial extract")}</div><div class="classic-input accent">{recoveryFailureAvailable() ? tr("gui.recovery.available_for_readable_entries", "Available for readable entries") : tr("gui.recovery.verify_first", "Verify first")}</div>
	                  <div class="classic-label">{tr("gui.recovery.report", "Report")}</div><div class="classic-input">{recoveryFailureAvailable() ? tr("gui.recovery.export_failure_report", "Export failure report") : tr("gui.recovery.no_report_yet", "No report yet")}</div>
                  <div class="classic-label">{tr("gui.recovery.promise", "Promise")}</div><div class="classic-input warning">{tr("gui.recovery.do_not_claim_success", "Do not claim repair success")}</div>
                </div>
              </aside>
            </div>
          </section>
        </div>
      {:else if screen === "recovery"}
        <div class="classic-dialog-body" class:with-archive-return={showArchiveReturnBar()}>
          {#if showArchiveReturnBar()}
            <ArchiveReturnStrip
              title={archiveTitle()}
              detail={archiveReturnDetail()}
              contextLabel={tr("gui.archive.current_context", "Current archive")}
              actionLabel={tr("gui.archive.back_to_current", "Back to current archive")}
              buttonClass="classic-primary"
              iconSize={15}
              onReturn={returnToCurrentArchive}
            />
          {/if}
          <section class="classic-recovery-sheet">
            <header>
              <div>
                <h1>{tr("gui.recovery.protect_verify_repair", "Recovery · Protect / Verify / Repair")}</h1>
                <p>{tr("gui.recovery.block_math_visible_body", "Block math is visible so repair promises match the actual Reed-Solomon capacity.")}</p>
              </div>
              <div class="classic-button-row">
	                <button
	                  disabled={Boolean(recoveryVerifyDisabledReason())}
	                  title={recoveryVerifyDisabledReason()}
	                  aria-label={labelWithDisabledReason(tr("gui.recovery.verify", "Verify"), recoveryVerifyDisabledReason())}
	                  onclick={() => void submitVerifyRecoveryJob()}
	                >{tr("gui.recovery.verify", "Verify")}</button>
	                <button
	                  disabled={Boolean(recoveryFailureDisabledReason())}
	                  title={recoveryFailureDisabledReason()}
	                  aria-label={labelWithDisabledReason(tr("gui.recovery.failed_case", "Failed case"), recoveryFailureDisabledReason())}
	                  onclick={() => setScreen("cannotRepair")}
	                >{tr("gui.recovery.failed_case", "Failed case")}</button>
                <button
                  disabled={Boolean(recoveryZipDisabledReason())}
                  title={recoveryZipDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.repair_zip_index", "Repair ZIP index"), recoveryZipDisabledReason())}
                  onclick={() => void submitRepairZipJob()}
                >{tr("gui.recovery.repair_zip_index", "Repair ZIP index")}</button>
                <button
                  disabled={Boolean(recoverySqzRepairDisabledReason())}
                  title={recoverySqzRepairDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.repair_sqz", "Repair SQZ"), recoverySqzRepairDisabledReason())}
                  onclick={() => void submitRepairSqzJob()}
                >{tr("gui.recovery.repair_sqz", "Repair SQZ")}</button>
                <button
                  disabled={Boolean(recoverySqzExportDisabledReason())}
                  title={recoverySqzExportDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.export_sqz", "Export SQZ"), recoverySqzExportDisabledReason())}
                  onclick={() => void submitExportSqzJob()}
                >{tr("gui.recovery.export_sqz", "Export SQZ")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(recoveryRepairPar2DisabledReason())}
                  title={recoveryRepairPar2DisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.recovery.repair_par2", "Repair PAR2"), recoveryRepairPar2DisabledReason())}
                  onclick={() => void submitRepairRecoveryJob()}
                >{tr("gui.recovery.repair_par2", "Repair PAR2")}</button>
              </div>
            </header>

            <div class="classic-recovery-grid">
              <section class="classic-recovery-form">
                <h2>{tr("gui.recovery.protection_settings", "Protection settings")}</h2>
                <div class="classic-form-grid compact">
                  <div class="classic-label">{tr("common.target", "Target")}</div><div class="classic-input">{archiveTitle()}</div>
                  <div class="classic-label">{tr("common.mode", "Mode")}</div><div class="classic-input classic-recovery-mode-summary"><strong>PAR2</strong><span>{tr("gui.recovery.par2_sidecars_body", "PAR2 sidecars are available for standard archives after protection data exists.")}</span></div>
	                  <div class="classic-label">{tr("gui.recovery.redundancy", "Redundancy")}</div><div class="classic-input accent">{currentArchive ? tr("gui.recovery.redundancy_configured", "10% · configured before protect") : openArchiveFirstLabel()}</div>
	                  <div class="classic-label">{tr("gui.recovery.loss_tolerance", "Loss tolerance")}</div><div class="classic-input">{currentArchive ? tr("gui.recovery.shown_after_verify", "Shown after verify") : openArchiveFirstLabel()}</div>
	                  <div class="classic-label">{tr("common.output", "Output")}</div><div class="classic-input">{currentArchive ? pathBaseName(defaultRecoveryPath()) : openArchiveFirstLabel()}</div>
                </div>
                <div class="classic-recovery-actions">
                  <button disabled={Boolean(recoveryProtectDisabledReason())} title={recoveryProtectDisabledReason()} aria-label={labelWithDisabledReason(tr("gui.action.protect", "Protect"), recoveryProtectDisabledReason())} onclick={() => void submitProtectJob()}>{tr("gui.action.protect", "Protect")}</button><button disabled={Boolean(recoveryZipDisabledReason())} title={recoveryZipDisabledReason()} aria-label={labelWithDisabledReason(tr("gui.recovery.repair_zip_index", "Repair ZIP index"), recoveryZipDisabledReason())} onclick={() => void submitRepairZipJob()}>{tr("gui.recovery.repair_zip_index", "Repair ZIP index")}</button><button disabled={Boolean(recoverySqzRepairDisabledReason())} title={recoverySqzRepairDisabledReason()} aria-label={labelWithDisabledReason(tr("gui.recovery.repair_sqz", "Repair SQZ"), recoverySqzRepairDisabledReason())} onclick={() => void submitRepairSqzJob()}>{tr("gui.recovery.repair_sqz", "Repair SQZ")}</button><button disabled={Boolean(recoverySqzExportDisabledReason())} title={recoverySqzExportDisabledReason()} aria-label={labelWithDisabledReason(tr("gui.recovery.export_sqz", "Export SQZ"), recoverySqzExportDisabledReason())} onclick={() => void submitExportSqzJob()}>{tr("gui.recovery.export_sqz", "Export SQZ")}</button><button disabled={Boolean(recoveryVerifyDisabledReason())} title={recoveryVerifyDisabledReason()} aria-label={labelWithDisabledReason(tr("gui.recovery.verify_par2", "Verify PAR2"), recoveryVerifyDisabledReason())} onclick={() => void submitVerifyRecoveryJob()}>{tr("gui.recovery.verify_par2", "Verify PAR2")}</button>
                </div>
              </section>

              <section class="classic-block-table">
	                <h2>{tr("gui.recovery.verify_result", "Verify result")}</h2>
	                <div class="repair-summary">
	                  <strong>{recoveryResultTitle()}</strong>
	                  <span>{recoveryResultDetail()}</span>
	                </div>
	                <div class="block-table">
	                  <div><b>{tr("gui.recovery.group", "Group")}</b><b>{tr("gui.recovery.data", "Data")}</b><b>{tr("gui.recovery.recovery_blocks", "Recovery")}</b><b>{tr("gui.recovery.damaged", "Damaged")}</b><b>{tr("common.status", "Status")}</b></div>
	                  {#each recoveryBlocksView() as row}
	                    <div><span>{row.group}</span><span>{row.data}</span><span>{row.recovery}</span><span>{row.damage}</span><strong>{recoveryBlockStatusLabel(row.status)}</strong></div>
	                  {:else}
	                    <div><span>-</span><span>-</span><span>-</span><span>-</span><strong>{recoveryResultTitle()}</strong></div>
	                  {/each}
	                </div>
	                <div class="repair-log">
	                  <span>{recoveryResultDetail()}</span>
	                  <span>{tr("gui.recovery.rerun_before_success", "Repair or extract re-runs verification before reporting success.")}</span>
	                </div>
              </section>
            </div>
          </section>
        </div>
      {:else}
        <div class="classic-body">
        <aside class="classic-tree" aria-label={tr("gui.aria.archive_folders", "Archive folders")}>
	          <div class="classic-tree-item active"><Icon name="archive" size={15} />{archiveTitle()}</div>
	          {#if currentArchive}
	            <div class="classic-tree-item"><Icon name="folder" size={15} />root</div>
	          {:else}
	            <div class="classic-tree-item muted" title={openArchiveFirstLabel()} aria-label={openArchiveFirstLabel()}><Icon name="folder-open" size={15} />{openArchiveFirstLabel()}</div>
	          {/if}
	          <div class="tree-note">
	            <strong>{tr("gui.archive.format", "Format")}</strong>
	            <span>{currentArchive ? `${archiveFormat()} · ${archiveEntryCountLabel(currentArchive.entry_count)}` : openArchiveFirstLabel()}</span>
	          </div>
	          <div class="tree-note nested-tree-note" data-preview-policy={activePreviewPolicyKind()} data-preview-code={activePreviewPolicyCode()}>
	            <strong>{tr("gui.preview.panel", "Entry preview")}</strong>
	            <span>{nestedPreview ? nestedPreviewTitle() : entryPreviewTitle()}</span>
	            <small>{nestedPreview ? nestedPreviewSubtitle() : entryPreviewSubtitle()}</small>
	            {#if nestedPreview}
	              {#each nestedPreviewRows() as item}
	                <em>{item.display}</em>
	              {/each}
	              <button onclick={() => void openNestedPreviewArchive()}><Icon name="folder-open" size={13} />{tr("gui.action.open_nested", "Open")}</button>
	              <button onclick={() => void extractNestedPreviewArchive()}><Icon name="archive" size={13} />{tr("gui.action.extract_nested", "Extract")}</button>
	              <button onclick={() => nestedPreview = null}>{tr("gui.common.clear", "Clear")}</button>
	            {:else if previewBusy()}
	              <div class="preview-loading compact" role="status" aria-live="polite">
	                <span>{tr("gui.preview.loading", "Loading preview")}</span>
	                <small>{entryPreviewSubtitle()}</small>
	              </div>
	            {:else if entryPreview}
	              {#if entryPreviewImageSrc()}
	                <img class="classic-preview-image" src={entryPreviewImageSrc() ?? ""} alt={entryPreview.display_name} />
	              {/if}
	              <button class="preview-system-action" onclick={() => void openEntryPreview()}><Icon name="external-link" size={13} />{tr("gui.action.open_preview", "Open")}</button>
	              <button onclick={() => void revealEntryPreview()}><Icon name="folder-open" size={13} />{tr("gui.toast.reveal", "Reveal")}</button>
	              <button onclick={() => entryPreview = null}>{tr("gui.common.clear", "Clear")}</button>
	            {:else if entryPreviewFailure}
	              <button onclick={() => retryEntryPreview()}><Icon name="rotate-cw" size={13} />{tr("gui.preview.retry", "Retry preview")}</button>
	            {:else}
	              <button disabled={!canPreviewEntrySelection()} aria-busy={previewBusy()} title={previewSelectedDisabledReason()} aria-label={labelWithDisabledReason(previewActionLabel(), previewSelectedDisabledReason())} onclick={() => void submitPreviewEntry()}><Icon name="eye" size={13} />{previewActionLabel()}</button>
	            {/if}
	          </div>
	          {#if canRenameSelection()}
	            <div class="tree-note move-tree-note">
	              <strong>{actionLabel("Rename target")}</strong>
	              <input class="classic-input" aria-label={tr("gui.rename.classic_target_name", "Classic rename target name")} bind:value={renameTargetName} onblur={() => commitRenameTargetName()} />
	              <small>{renameTargetStatus()}</small>
	            </div>
	          {/if}
	          {#if hasArchiveSelection()}
	            <div class="tree-note move-tree-note">
	              <strong>{actionLabel("Move target")}</strong>
	              <input class="classic-input" aria-label={tr("gui.move.classic_target_folder", "Classic move target folder")} bind:value={moveTargetDir} onblur={() => commitMoveTargetDir()} />
	              <small>{moveTargetStatus()}</small>
	            </div>
	          {/if}
        </aside>

        <section class="classic-table-wrap">
          {#if currentArchive && hasArchiveSelection()}
            <div class="classic-workbench-strip">
              <label>
                <span>{actionLabel("Rename to")}</span>
                <input aria-label={tr("gui.rename.classic_table_target_name", "Classic table rename target name")} bind:value={renameTargetName} disabled={!canRenameSelection()} title={canRenameSelection() ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")} onblur={() => commitRenameTargetName()} />
              </label>
              <label>
                <span>{actionLabel("Move to")}</span>
                <input aria-label={tr("gui.move.classic_table_target_folder", "Classic table move target folder")} bind:value={moveTargetDir} onblur={() => commitMoveTargetDir()} />
              </label>
              <label>
                <span>{actionLabel("New folder")}</span>
                <input aria-label={tr("gui.new_folder.classic_name", "Classic new folder name")} bind:value={newFolderName} onblur={() => commitNewFolderName()} />
              </label>
              <small>{renameTargetStatus()} · {moveTargetStatus()} · {newFolderStatus()}</small>
            </div>
          {:else}
            <div class="classic-workbench-strip empty-workbench-strip">
              <span>{currentArchive ? selectedSummary() : openArchiveFirstLabel()}</span>
              <small>{currentArchive ? tr("gui.preview.double_click_hint", "Choose one entry to enable Preview.") : tr("gui.classic.empty_workbench_hint", "Archive editing controls appear after an archive is open.")}</small>
            </div>
          {/if}
          {#if moveConflictReview}
            <div class="classic-move-conflict-review" role="dialog" aria-label={tr("gui.move.conflicts", "Move target conflicts")} tabindex="-1">
              <header>
                <strong>{tr("gui.move.conflict_count", "{count} move conflicts").replace("{count}", String(moveConflictCount()))}</strong>
                <span>{tr("gui.move.ready_target", "{count} ready · target {target}").replace("{count}", String(moveReadyCount())).replace("{target}", moveConflictReview.targetDir)}</span>
              </header>
              <div class="classic-move-conflict-table">
                <div><b>{tr("common.source", "Source")}</b><b>{tr("gui.move.existing_target", "Existing target")}</b><b>{tr("gui.move.keep_both_target", "Keep both target")}</b></div>
                {#each visibleMoveConflictItems() as item}
                  <div><strong>{item.from}</strong><span>{item.to}</span><em>{item.keepBothTo}</em></div>
                {/each}
              </div>
              <div class="classic-button-row compact-row">
                <button onclick={() => moveConflictReview = null}>{tr("gui.common.cancel", "Cancel")}</button>
                <button disabled={moveReadyCount() === 0} onclick={() => void submitMoveReadyOnly()}>{tr("gui.move.ready_only", "Move ready only")}</button>
                <button class="classic-primary" onclick={() => void submitMoveKeepBoth()}>{tr("gui.move.keep_both_all", "Keep both and move all")}</button>
              </div>
            </div>
          {/if}
          <div class="classic-table" role="table" aria-label={tr("gui.table.archive", "Archive table")} data-total-rows={currentArchive ? totalRows() : 0}>
            <div class="classic-head" role="row">
              <span>{tr("gui.table.name", "Name")}</span><span>{tr("gui.table.size", "Size")}</span><span>{tr("gui.table.packed", "Packed")}</span><span>{tr("gui.table.ratio", "Ratio")}</span><span>{tr("gui.table.modified", "Modified")}</span><span>{tr("gui.table.crc", "CRC")}</span><span>{tr("gui.table.method", "Method")}</span><span>{tr("gui.table.attr", "Attr")}</span>
            </div>
            <div class="virtual-scroll classic-virtual-scroll" data-virtual-list="classic" onscroll={onBrowseVirtualScroll}>
              <div class="virtual-pad" style={`height: ${browsePaddingTop(CLASSIC_ROW_HEIGHT)}px`}></div>
	            {#each browseEntries(CLASSIC_ROW_HEIGHT) as entry}
	              <div class:selected={isEntrySelected(entry)} class="classic-row" role="row" tabindex="0" data-row-index={entry.virtualIndex ?? ""} onclick={(event) => selectEntry(entry, event)} ondblclick={(event) => { event.preventDefault(); void activateEntry(entry); }} onkeydown={(event) => onEntryKeydown(event, entry)} oncontextmenu={(event) => openEntryContext(event, entry)}>
                  <span class="table-name">
                    <button
                      type="button"
                      class="row-select-toggle"
                      class:checked={isEntrySelected(entry)}
                      role="checkbox"
                      aria-checked={isEntrySelected(entry)}
                      aria-label={entrySelectionLabel(entry)}
                      title={entrySelectionLabel(entry)}
                      disabled={!entry.source}
                      onclick={(event) => {
                        event.stopPropagation();
                        toggleEntrySelection(entry);
                      }}
                    ></button>
                    {entry.name}
                    {#if entry.source}
                        <button
                        class="row-preview-button compact"
                        disabled={previewBusy()}
                        aria-busy={previewBusy()}
                        title={previewActionLabel(entry.source.path, entry.source.entry_type)}
                        aria-label={`${previewActionLabel(entry.source.path, entry.source.entry_type)} ${entry.name}`}
                        onclick={(event) => {
                          event.stopPropagation();
                          selectOnlyEntry(entry);
                          void submitPreviewEntry(entry.source?.path ?? null, entry.source?.entry_type ?? null);
                        }}
                      ><Icon name={entry.source.entry_type === "dir" ? "folder-open" : "eye"} size={12} /></button>
                    {/if}
                  </span>
                  <span>{entry.size}</span>
                  <span>{entry.packed}</span>
                  <span>{entry.ratio}</span>
                  <span>{entry.modified}</span>
                  <span>{entry.crc}</span>
                  <span>{entry.method}</span>
                  <span>{entry.attr}</span>
                </div>
              {:else}
                <div class="classic-row empty-row" role="row">
                  <span class="table-name">{openArchiveFirstLabel()}</span>
                  <span>{noEntriesLabel()}</span>
                  <span>-</span>
                  <span>-</span>
                  <span>-</span>
                  <span>-</span>
                  <span>-</span>
                  <span>-</span>
                </div>
              {/each}
              <div class="virtual-pad" style={`height: ${browsePaddingBottom(CLASSIC_ROW_HEIGHT)}px`}></div>
            </div>
          </div>
        </section>
      </div>
      {/if}

      <footer class="classic-statusbar">
        {#if screen === "create"}
          <span>{lastCreateEstimate ? tr("gui.create.source_files_count", "{count} source files").replace("{count}", lastCreateEstimate.files.toLocaleString()) : tr("gui.create.source_files_pending", "Source files pending")}</span>
          <span>{activeCreateFormatData().label} · {createMethodLabel()}</span>
          <span>{createSplitCapability()} · {createRecoveryCapability()}</span>
          <strong>{diskPreflightStatusbar()}</strong>
        {:else if screen === "extract"}
          <span>{actionLabel("Extract selected")}</span>
          <span>{extractSelectionLabel()} · {extractDestinationTitle(extractDestinationMode)}</span>
          <span>{tr("gui.extract.status_conflicts", "Conflicts: {mode} · {password}").replace("{mode}", extractOverwriteLabel()).replace("{password}", extractPasswordLabel())}</span>
          <strong>{tr("gui.extract.status_destination", "Destination: {destination}").replace("{destination}", effectiveExtractDest())}</strong>
        {:else if screen === "batch"}
          <span>{tr("gui.batch.title", "Batch Extract")}</span>
          <span>{tr("gui.batch.status_counts", "{archives} archives · {ready} ready").replace("{archives}", String(batchReviewArchives().length)).replace("{ready}", String(batchReadyCount()))}</span>
          <span>{batchWarningLabel()}</span>
          <strong>{tr("gui.batch.status_ready_rule", "Ready archives can continue; blocked archive waits")}</strong>
        {:else if screen === "checksum"}
          <span>{tr("gui.screen.checksum", "Checksum")}</span>
          <span>{tr("gui.status.target", "Target: {target}").replace("{target}", checksumTargetName())}</span>
          <span>{tr("gui.checksum.status_algorithm_excludes", "{algorithm} · {count} excludes").replace("{algorithm}", checksumAlgorithmLabel(checksumAlgorithm)).replace("{count}", String(checksumExcludeRules().length))}</span>
          <strong>{tr("gui.checksum.status_failed_latest", "{count} failed in latest manifest check").replace("{count}", checksumResultNumber("checksum_check", "failed").toLocaleString())}</strong>
        {:else if screen === "duplicates"}
          <span>{tr("gui.screen.duplicates", "Duplicate Finder")}</span>
          <span>{tr("gui.status.target", "Target: {target}").replace("{target}", duplicateScanTargetName())}</span>
          <span>{tr("gui.duplicates.status_min_excludes", "Min: {min} · {count} excludes").replace("{min}", formatBytes(duplicateMinSize)).replace("{count}", String(duplicateExcludeRules().length))}</span>
          <strong>{tr("gui.duplicates.status_groups_reclaimable", "{groups} groups · {size} reclaimable").replace("{groups}", duplicateResultNumber("duplicate_groups").toLocaleString()).replace("{size}", formatBytes(duplicateResultNumber("reclaimable_bytes")))}</strong>
        {:else if screen === "password"}
          <span>{tr("gui.screen.password", "Password Required")}</span>
          <span>{passwordPromptName()}</span>
          <span>{tr("gui.password.keychain_opt_in_short", "{secretStore} opt-in only").replace("{secretStore}", secretStoreLabel())}</span>
          <strong>{tr("gui.password.no_plaintext_storage", "No plaintext password stored in settings or task status")}</strong>
	        {:else if screen === "conflict"}
	          <span>{tr("gui.screen.conflict", "Conflict Handling")}</span>
	          <span>{jobConflictPrompt ? tr("gui.conflict.existing_files_loaded", "Existing files loaded") : tr("gui.conflict.no_prompt", "No conflict prompt")}</span>
	          <span>{tr("gui.conflict.default_ask_before_replace", "Default: ask before replace")}</span>
	          <strong>{jobConflictPrompt ? tr("gui.conflict.silent_overwrite_disabled", "Silent overwrite disabled") : tr("gui.conflict.no_active_request", "No conflict request is active")}</strong>
	        {:else if screen === "cannotRepair"}
	          <span>{tr("gui.screen.cannot_repair", "Recovery Limit")}</span>
	          <span>{recoveryFailureAvailable() ? tr("gui.recovery.status_damage_blocks", "Damage: 37 blocks") : tr("gui.recovery.no_failure_result", "No failure result")}</span>
	          <span>{recoveryFailureAvailable() ? tr("gui.recovery.status_capacity_blocks", "Capacity: 24 blocks") : tr("gui.recovery.verify_first", "Verify first")}</span>
	          <strong>{recoveryFailureAvailable() ? tr("gui.recovery.full_blocked_partial_only", "Full repair blocked · partial extract only") : tr("gui.recovery.no_repair_failure_loaded", "No repair failure loaded")}</strong>
	        {:else if screen === "recovery"}
	          <span>{tr("gui.recovery.status_par2_sidecar", "Recovery: PAR2 sidecar")}</span>
	          <span>{currentArchive ? tr("gui.archive.loaded", "Archive loaded") : noArchiveLabel()}</span>
	          <span>{recoveryResultAvailable() ? tr("gui.recovery.status_capacity_blocks", "Capacity: 24 blocks") : tr("gui.recovery.verify_first", "Verify first")}</span>
	          <strong>{recoveryResultFooter()}</strong>
        {:else if screen === "archiveInfo"}
          <span>{tr("gui.screen.archive_info", "Archive Info")}</span>
          <span>{currentArchive ? archiveTitle() : openArchiveFirstLabel()}</span>
          <span>{currentArchive ? archiveFormat() : "-"}</span>
          <strong>{extractDestinationHint()}</strong>
        {:else if screen === "integration"}
          <span>{tr("gui.screen.integration", "File Associations")}</span>
          <span>{tr("gui.settings.integration.status_rows", "{extensions} registry extensions · {rows} rows").replace("{extensions}", String(registryFormatExtensions().length)).replace("{rows}", String(associationRows().length))}</span>
          <span>{integrationResult ? tr("gui.settings.integration.platform_actions_installed_count", "{count} {fileManager} actions installed").replace("{count}", String(integrationResult.installed.length)).replace("{fileManager}", fileManagerLabel()) : tr("gui.settings.integration.platform_actions_ready", "5 {fileManager} actions ready").replace("{fileManager}", fileManagerLabel())}</span>
          <strong>{integrationStatus === "installed" ? tr("gui.settings.integration.file_manager_actions_installed_no_takeover", "{fileManager} actions installed · no default takeover").replace("{fileManager}", fileManagerLabel()) : tr("gui.settings.integration.open_with_no_takeover", "{platform} {openWith} · no default takeover").replace("{platform}", platformNameLabel()).replace("{openWith}", openWithLabel())}</strong>
        {:else if screen === "settingsGeneral"}
          <span>{tr("gui.settings.general.eyebrow", "Settings / General")}</span>
          <span>{tr("gui.settings.general.status_detail", "Language: {language} · Folder: {folder} · Reveal: {reveal}").replace("{language}", languageLabel(generalLanguageChoice || null)).replace("{folder}", defaultExtractFolderLabel()).replace("{reveal}", generalRevealAfterExtract ? tr("common.on", "on") : tr("common.off", "off"))}</span>
          <span>{tr("gui.settings.general.status_updates", "Updates: manual check")}</span>
          <strong>{tr("gui.settings.general.status_rule", "Safety and password warnings remain visible")}</strong>
        {:else if screen === "appearance"}
          <span>{tr("gui.screen.appearance", "Appearance")}</span>
          <span>{tr("gui.appearance.status_mode", "Mode: {mode}").replace("{mode}", mode === "classic" ? tr("gui.mode.classic", "Classic") : tr("gui.mode.modern", "Modern"))}</span>
          <span>{tr("gui.appearance.theme", "Theme")}: {themeStatusLabel()} · {tr("gui.appearance.density", "Density")}: {densityLabel()}</span>
          <strong>{tr("gui.appearance.status_colors", "Theme Colors available as Appearance subpage")}</strong>
        {:else if screen === "colors"}
          <span>{tr("gui.screen.colors", "Appearance · Theme Colors")}</span>
          <span>{tr("gui.colors.status_palette", "Theme color: {palette}").replace("{palette}", activePaletteName())}</span>
          <span>{tr("gui.colors.status_accent", "Accent: {accent}").replace("{accent}", activePalettePreviewData.accent)}</span>
          <strong>{tr("gui.colors.status_contrast", "Contrast guard active · semantic colors locked")}</strong>
        {:else if screen === "settingsSecurity"}
          <span>{tr("gui.settings.security.eyebrow", "Settings / Security")}</span>
          <span>{tr("gui.settings.security.status_never_disabled", "Zip Slip and symlink escape never disabled")}</span>
          <span>{tr("gui.settings.security.status_caps", "Output cap: {output} GiB · entries: {entries}").replace("{output}", formattedNumber(safetyMaxOutputGiB, defaultSafety.maxOutputGiB)).replace("{entries}", formattedNumber(safetyMaxEntries, defaultSafety.maxEntries))}</span>
          <strong>{tr("gui.settings.security.status_limits", "Limits captured when job starts")}</strong>
        {:else if screen === "settingsPerformance"}
          <span>{tr("gui.settings.performance.eyebrow", "Settings / Performance")}</span>
          <span>{tr("gui.settings.performance.status_resources", "Workers: {workers} · buffer: {buffer}").replace("{workers}", performanceThreads === null ? tr("common.auto", "auto") : formattedNumber(performanceThreads, 4)).replace("{buffer}", performanceMemoryMiB === null ? tr("common.auto", "auto") : `${formattedNumber(performanceMemoryMiB, 512)} MiB`)}</span>
          <span>{tr("gui.task.status_prefix", "Task: {status}").replace("{status}", currentTaskStatusLabel())}</span>
          <strong>{tr("gui.settings.performance.status_snapshot", "Resource snapshot captured when jobs start")}</strong>
        {:else if screen === "passwordBook"}
          <span>{tr("gui.settings.password_book.eyebrow", "Settings / Password Book")}</span>
          <span>{passwordBookSecretStoreLabel()} · {passwordBookCurrentLabel()}</span>
          <span>{tr("gui.settings.password_book.status_frontend", "Frontend shows status only")}</span>
          <strong>{tr("gui.settings.password_book.status_boundary", "No plaintext password leaves secret-store boundary")}</strong>
        {:else}
	          <span>{currentArchive ? archiveEntryCountLabel(currentArchive.entry_count) : noArchiveLabel()}</span>
	          <span>{selectedSummary()}</span>
	          <span>{currentArchive ? `${archiveFormat()} · ${currentArchive.volumes?.length ? archiveVolumeCountLabel(currentArchive.volumes.length) : tr("gui.archive.single_file", "single file")}` : openArchiveFirstLabel()}</span>
          <strong>{currentTaskStatusLabel()}</strong>
        {/if}
      </footer>
    </section>
  </main>
{/if}
