<script lang="ts">
  import { onMount, tick } from "svelte";
  import ArchiveStartState from "./components/ArchiveStartState.svelte";
  import ArchiveReturnStrip from "./components/ArchiveReturnStrip.svelte";
  import AppIcon from "./components/AppIcon.svelte";
  import ClassicArchiveBrowserHost from "./components/ClassicArchiveBrowserHost.svelte";
  import type { ClassicArchiveBrowserSurfaceProps } from "./components/ClassicArchiveBrowserHost.svelte";
  import ArchiveOperationWorkspaceHost from "./components/ArchiveOperationWorkspaceHost.svelte";
  import type {
    CreateWorkspaceSurface,
    CreateWorkspaceVariant,
  } from "./components/ArchiveOperationWorkspaceHost.svelte";
  import type {
    ExtractWorkspaceSurface,
    ExtractWorkspaceVariant,
  } from "./components/ArchiveOperationWorkspaceHost.svelte";
  import Icon from "./components/Icon.svelte";
  import ModernArchiveBrowserHost from "./components/ModernArchiveBrowserHost.svelte";
  import type { ModernArchiveBrowserSurfaceProps } from "./components/ModernArchiveBrowserHost.svelte";
  import ModernInspectorHost from "./components/ModernInspectorHost.svelte";
  import type { ModernInspectorSurfaceProps } from "./components/ModernInspectorHost.svelte";
  import RecoveryWorkspaceHost from "./components/RecoveryWorkspaceHost.svelte";
  import SettingsRouteList from "./components/SettingsRouteList.svelte";
  import SettingsWorkspaceHost from "./components/SettingsWorkspaceHost.svelte";
  import type {
    SettingsScreen,
    SettingsWorkspaceProps,
  } from "./components/SettingsWorkspace.svelte";
  import TaskCenterHost from "./components/TaskCenterHost.svelte";
  import type { TaskCenterSurfaceProps } from "./components/TaskCenterHost.svelte";
  import TaskInteractionWorkspaceHost from "./components/TaskInteractionWorkspaceHost.svelte";
  import type {
    TaskInteractionWorkspaceKind,
    TaskInteractionWorkspaceSurface,
    TaskInteractionWorkspaceVariant,
  } from "./components/TaskInteractionWorkspaceHost.svelte";
  import TaskProgressDialogHost from "./components/TaskProgressDialogHost.svelte";
  import type { TaskProgressDialogSurfaceProps } from "./components/TaskProgressDialogHost.svelte";
  import type MacosSfxPublisherComponent from "./components/MacosSfxPublisher.svelte";
  import ToastHost from "./components/ToastHost.svelte";
  import ToolsWorkspaceHost from "./components/ToolsWorkspaceHost.svelte";
  import type {
    BatchWorkspaceSurface,
    ChecksumResultKind,
    ChecksumWorkspaceSurface,
    DuplicatesWorkspaceSurface,
    ToolsWorkspaceVariant,
  } from "./components/ToolsWorkspaceHost.svelte";
  import {
    operationHistory,
    recordOperation,
  } from "./lib/history.svelte";
  import { copyTextToClipboard } from "./lib/clipboard";
  import type { UpdateCheckPreview } from "./lib/app-update.svelte";
  import {
    adoptOpenedArchive,
    allRowsLoaded,
    archive,
    archiveBrowseError,
    archiveOpenError,
    archiveHasSessionPassword,
    archivePasswordBookStatus,
    allCurrentRowsSelected,
    cancelPasswordPrompt as cancelArchivePasswordPrompt,
    clearSelection,
    currentDirs,
    enterDirPath,
    filterPending,
    filterText,
    findLoadedRow,
    forgetCurrentArchivePassword,
    gotoBreadcrumb,
    goUp,
    installArchivePreview,
    loadRowAt,
    loadedRows,
    openArchive as openArchiveStore,
    openPasswordPrompt,
    reopenWithEncoding,
    refreshCurrentArchive,
    refreshArchivePasswordBookStatus,
    recentFiles,
    rememberRecent,
    PAGE_SIZE as ARCHIVE_PAGE_SIZE,
    prefetchAround,
    rowAt,
    selectAllRows,
    selectedPaths,
    selectedSize,
    setFilter,
    toggleSelect,
    totalRows,
  } from "./lib/archive.svelte";
  import {
    archiveNameWithoutVolumeSuffix,
    archiveVolumeFamilyKeys,
    isLegacyRarVolumeName,
    isNativeSplitZipVolumeName,
    legacyRarVolumeExtensions,
    nativeSplitZipVolumeExtensions,
  } from "./lib/archive-names";
  import {
    ipc,
    isErrorDto,
    type CreatePlanDto,
    type ExtractPlanPreflightDto,
    type CreateArchivePresetOptions,
    type CreateCompletionAction,
    type CreateContentPolicy,
    type CreateDestinationBase,
    type CreateDestinationInspectionDto,
    type DiskSpaceDto,
    type EntryPreviewDto,
    type EntryDto,
    type ErrorDto,
    type JobSpec,
    type ArchivePresetDocument,
    type ExtractArchivePresetOptions,
    type NamedArchivePreset,
    type PostSuccessAction,
    type LanguageDto,
    type NestedArchivePreviewDto,
    type SettingsDto,
    type SfxCreateCapabilityDto,
    type SourceCleanupRecoveryNotice,
    type FormatDto,
    type ExistingOutputPolicy,
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
  import type {
    ConvertRouteBridge,
    ConvertRouteHandle,
    ConvertRouteOwner,
    ConvertRouteStatus,
  } from "./lib/convert-route";
  import {
    fat32CompatibleSplitSizeBytes,
    resolveSplitSizeBytes,
  } from "./lib/archive-output-options";
  import {
    archiveBaseOrDefault,
    desktopBasename,
    desktopDirname,
    joinDesktopPath,
    normalizeDesktopFolder,
    sameDesktopPath,
  } from "./lib/desktop-path";
  import {
    createSourcePaths,
    includesCreateSourcePath,
    mergeCreateSources,
    removeCreateSourcesByPaths,
    toggleCreateSourcePath,
    type CreateSourceKind,
    type CreateSourceRoot,
  } from "./lib/create-sources";
  import { platformTrashName } from "./lib/platform-labels";
  import { cssVariables, type CssVariableMap } from "./lib/css-variables";
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
  import { currentLang, listBundledLanguages, loadLocale, t, tError } from "./lib/i18n.svelte";
  import { pushToast, removeToastByKey } from "./lib/toasts.svelte";
  import { isNewSourceCleanupRecoveryGeneration } from "./lib/source-cleanup";
  import { currentWebviewWindowListener } from "./lib/tauri-events";
  import { previewSystemOpenRequiresConfirmation } from "./lib/preview-presentation";
  import {
    previewResponseIsCurrent,
    type PreviewResponseIdentity,
  } from "./lib/preview-response";
  import {
    activeTask,
    answerConflict as answerJobConflict,
    answerPassword as answerJobPassword,
    cancelTask,
    clearFinished,
    initJobEvents,
    pauseTask,
    pendingConflict,
    pendingPassword,
    resumeTask,
    installActiveTaskPreview,
    installCompletedTaskPreview,
    installTaskQueuePreview,
    moveTaskBefore,
    moveTaskEarlier,
    moveTaskLater,
    setCreateCompletionHandler,
    setRevealAfterExtractPreference,
    setSourceCleanupRecoveryRefreshHandler,
    setTaskExpanded,
    submitJob as submitArchiveJob,
    tasks,
    titleFor as titleForJobSpec,
    type Task,
  } from "./lib/jobs.svelte";
  import {
    clearableTaskIds,
    taskCenterActionableCount,
    taskCenterCounts,
    taskSubmissionBlockReason,
    type TaskSubmissionBlockReason,
  } from "./lib/task-center";
  import {
    applyCreateDestinationAuthorization,
    checksumItemStatus,
    checksumItemText,
    checksumResultLine,
    isTaskActiveState,
    normalizeTaskConflictAnswer,
    taskPasswordReady,
    taskChecksumResultText,
    taskFailureReviewScreen,
    taskOutputCanOpen,
    taskOutputIsFolder,
    taskOutputPath,
    taskHasInlineResults,
    taskResultScreen,
    taskStateLabel,
    type TaskConflictDecision,
    type TaskDialogModel,
  } from "./lib/task-model";
  import {
    latestMatchingRecoveryTask,
    recoveryRouteForOpen,
    recoveryRepairGate,
    recoveryResultBoolean as recoveryResultMetricBoolean,
    recoveryResultConfirmsRepairCapacity,
    recoveryResultHasNoDamage,
    recoveryResultMetrics,
    recoveryResultNumber as recoveryResultMetricNumber,
    recoveryResultOk,
    recoveryResultOperation,
    recoveryResultTone as recoveryResultToneFor,
  } from "./lib/recovery-result";
  import type {
    RecoveryWorkspaceActions,
    RecoveryWorkspaceView,
  } from "./lib/recovery-workspace";
  import {
    activeUiMode,
    initUiMode,
    setUiMode as persistUiMode,
    uiModeChoice,
    type UiMode,
  } from "./lib/uiMode.svelte";
  import {
    deriveCustomPaletteTokens,
    normalizeHexColor,
  } from "./lib/theme";
  import {
    checksumAlgorithms,
    classicCommands,
    createFormatIds,
    createFormats,
    createProfileIds,
    createProfiles,
    defaultCustomAccent,
    moveTargetPresets,
    nav,
    paletteIds,
    quickActions,
    screenIds,
    settingsSections,
  } from "./lib/ui-model";
  import type {
    ChecksumAlgorithmId,
    CreateFormatId,
    CreateProfileId,
    CreateSplitMode,
    CreateSplitPreset,
    CreateSplitUnit,
    DensityChoice,
    NumericSetting,
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
  type NativeSplitKind = "zip" | "wim" | null;
  type MacosSfxPublisherComponentType = typeof MacosSfxPublisherComponent;
  type ThemeChoice = "system" | "light" | "dark";
  type PersistedSettingsSection = "general" | "security" | "performance" | "colors";
  type SettingsSaveOutcome = "idle" | "saved" | "session" | "error";
  type SettingsSaveState = "saved" | "dirty" | "saving" | "session" | "error";
  type ArchivePresetMutationState = "idle" | "saving" | "error";
  type AppearanceSetting = "mode" | "theme" | "density";
  type AppearanceSaveState = Exclude<SettingsSaveState, "dirty">;
  type ExtractDestinationMode = "smart" | "archive" | "same" | "choose";
  type ExtractScope = "all" | "selection";
  type ExtractOverwriteMode = "ask" | "skip" | "overwrite" | "rename";
  type ExtractSymlinkMode = "preserve" | "skip" | "follow";
  type ExtractPlanRequest = Readonly<{
    key: string;
    generation: number;
    requestId: string;
    path: string;
    displayPath: string;
    dest: string;
    selection: string[] | null;
    smart: boolean;
    encoding: string | null;
    promise: Promise<void>;
    resolve: () => void;
    control: { cancelRequested: boolean };
  }>;
  type PresetSfxTarget = Extract<CreateArchivePresetOptions["output"], { kind: "self_extracting" }>["target"];
  type PresetSqzInnerFormat = Extract<CreateArchivePresetOptions["format_options"], { kind: "sqz" }>["inner_format"];
  type ClassicCreateSection = "general" | "compression" | "content" | "security" | "volumes" | "recovery" | "preflight";
  type CreatePreflightStage = "source" | "temp" | "destination" | "submit";
  type CreatePreflightStepState = "pending" | "active" | "ready" | "blocked" | "cancelled";
  type CreatePreflightPhase =
    | "idle"
    | "selecting"
    | "measuring"
    | "checkingTemp"
    | "choosingDest"
    | "checkingDest"
    | "reviewing"
    | "submitting"
    | "ready"
    | "cancelled"
    | "blocked";
  type ResolvedCreateDestination = Readonly<{
    path: string;
    replaceExisting: boolean;
    replacementGuard: string | null;
    confirmLateConflict: boolean;
  }>;
  type AuthorizedArchiveOutput = Readonly<{
    replaceExisting: boolean;
    replacementGuard: string | null;
  }>;
  type CreateRunDraft = Readonly<{
    format: CreateFormatId;
    profile: CreateProfileId;
    level: number;
    password: string | null;
    encryptNames: boolean;
    splitSize: number | null;
    splitMode: CreateSplitMode;
    contentPolicy: CreateContentPolicy;
    excludes: readonly string[];
    sqzInnerFormat: "sqz" | "zip" | "7z" | null;
    sfxEnabled: boolean;
    sfxTarget: PlatformKind | null;
    outputExtension: string;
    destination: Readonly<{
      base: CreateDestinationBase;
      existing_output: ExistingOutputPolicy;
    }>;
    completion: CreateCompletionAction;
    postSuccess: PostSuccessAction;
    testAfterCreate: boolean;
    defaultCreateDir: string | null;
    restoreCredentialPrompt: boolean;
    restoreEncryptNames: boolean;
  }>;
  type PendingCreateSubmission = Readonly<{
    spec: JobSpec;
    source: "dialog" | "drop";
    format: CreateFormatId;
    profile: CreateProfileId;
    creatingSfx: boolean;
    artifactLabel: string;
    splitSize: number | null;
    confirmLateConflict: boolean;
    restoreCredentialPrompt: boolean;
    restoreEncryptNames: boolean;
  }>;
  class JobSubmitBlockedError extends Error {
    readonly reason: TaskSubmissionBlockReason;

    constructor(reason: TaskSubmissionBlockReason) {
      super(`job-submit-blocked:${reason}`);
      this.reason = reason;
    }
  }

  class CreateDestinationInspectionError extends Error {
    readonly detail: ErrorDto | null;
    readonly cancelled: boolean;

    constructor(error?: unknown, cancelled = false) {
      super("create-destination-inspection-failed");
      this.detail = isErrorDto(error) ? error : null;
      this.cancelled = cancelled;
    }
  }

  const devToolsChordKeys = new Set(["i", "j", "c"]);
  const balancedCreatePresetId = "builtin.create.balanced-7z";
  const crossPlatformCreatePresetId = "builtin.create.cross-platform-7z";
  const smartExtractPresetId = "builtin.extract.smart";
  const maxArchivePresets = 65;
  const maxArchivePresetExcludeRules = 64;
  const maxArchivePresetExcludeRuleBytes = 256;
  const extractPlanDebounceMs = 140;
  type FormatCapabilityCard = {
    id: string;
    name: string;
    state: string;
    create: string;
    volumes: string;
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
    request_id?: string;
    phase?: string;
    scanned?: number;
    processed_bytes?: number;
    total_bytes?: number;
    current?: string;
  };
  type OpenFilesPayload = {
    paths: string[];
    action?: string | null;
    output?: string | null;
  };
  type DisplayEntry = {
    name: string;
    location: string;
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
  type EntryContext = {
    x: number;
    y: number;
    name: string;
    path: string | null;
    canRename: boolean;
    isDir: boolean;
  };
  type PreviewPhase = "idle" | "entry" | "nested";
  type PreviewPolicyKind = "none" | "folder" | "nested" | "system-file";
  type PreviewPolicyCode =
    | "no_archive"
    | "select_one"
    | "folder"
    | "nested"
    | "system_type"
    | "system_unknown"
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
    outerSource: string;
    outerDisplayPath: string;
    message: string;
    retryAction: "preview" | "open";
  };
  type ValidationWindow = Window & {
    __squallzValidationSetScreen?: (next: Screen) => boolean;
    __squallzValidationJobSubmitAttempts?: number;
    __squallzValidationJobSubmitBlockedWhileStarting?: number;
  };

  const params = new URLSearchParams(window.location.search);
  const modeParam = params.get("mode");
  const defaultExtractDirParam = params.get("defaultExtractDir");
  const initialMode: Mode | null = modeParam === "classic" || modeParam === "modern" ? modeParam : null;
  const forceFirstRun = params.get("firstRun") === "1" || modeParam === "unset";
  const runtimePreviews = readRuntimePreviews(params, ARCHIVE_PAGE_SIZE);
  const previewDestinationRequestId = import.meta.env.DEV && runtimePreviews.preflightDestinationBytes > 0
    ? "dev-preview-destination"
    : null;
  const hideHistoryParam = params.get("hideHistory") === "1";
  const createFormatParam = params.get("createFormat");
  const previewDelayMs = Math.max(0, Math.min(500, Number(params.get("previewDelayMs") ?? 0) || 0));
  const updateCheckPreview: UpdateCheckPreview = import.meta.env.DEV
    ? (() => {
        const value = params.get("previewUpdate");
        return value === "available"
          || value === "manifest"
          || value === "current"
          || value === "ahead"
          || value === "error"
          ? value
          : null;
      })()
    : null;
  initUiMode(forceFirstRun ? null : initialMode);

  let mode = $derived(activeUiMode());
  let settingsStatus = $state<"loading" | "ready" | "preview">(forceFirstRun || initialMode ? "preview" : "loading");
  let hideOperationHistory = $state(hideHistoryParam);
  let firstRunRequired = $derived(settingsStatus !== "loading" && uiModeChoice() === null);
  let firstRunPanel = $state<HTMLElement | null>(null);
  let firstRunRecommendedButton = $state<HTMLButtonElement | null>(null);
  let firstRunDropFeedback = $state<string | null>(null);
  let currentArchive = $derived(archive());
  let archiveDirs = $derived(currentDirs());
  let passwordBookStatus = $derived(archivePasswordBookStatus());
  let jobRows = $derived(tasks());
  let activeCurrentTask = $derived(activeTask());
  let jobPasswordPrompt = $derived(pendingPassword());
  let archivePasswordPrompt = $derived(openPasswordPrompt());
  let activePasswordPromptIdentity = $derived(
    jobPasswordPrompt
      ? `job:${jobPasswordPrompt.id}`
      : archivePasswordPrompt
        ? `archive:${archivePasswordPrompt.path}`
        : null,
  );
  let previousPasswordPromptIdentity: string | null = null;
  let jobConflictPrompt = $derived(pendingConflict());
  let activeConflictPromptIdentity = $derived(
    jobConflictPrompt
      ? `${jobConflictPrompt.id}:${jobConflictPrompt.incoming_path}`
      : null,
  );
  let previousConflictPromptIdentity: string | null = null;
  let taskDialogTaskId = $state<number | null>(null);
  let taskDialogDismissedId = $state<number | null>(null);
  let macosSfxPublisherTask = $state<TaskDialogModel | null>(null);
  let LoadedMacosSfxPublisher = $state<MacosSfxPublisherComponentType | null>(null);
  let macosSfxPublisherLoad: Promise<MacosSfxPublisherComponentType> | null = null;
  let taskCenterOpen = $state(false);
  let taskCenterSelectedTaskId = $state<number | null>(null);
  let taskCenterFocusTaskId = $state<number | null>(null);
  let taskCenterReturnFocus: HTMLElement | null = null;
  let jobEventsReady = $state(false);
  let initialTaskWindowSubmitted = false;
  const initialTaskWindowLaunchState = taskWindowLaunchStateFromParams(params);
  let taskWindowLaunchState = $state(initialTaskWindowLaunchState);
  const initialTaskWindowLaunch = initialTaskWindowLaunchState.launch;
  let taskWindowMode = $derived(taskWindowLaunchState.mode);
  let modeSelectionBlocked = $derived(!taskWindowMode && uiModeChoice() === null);
  let taskWindowPendingAction = $derived(taskWindowLaunchState.pendingAction);
  let taskWindowShellTitleCopy = $derived(taskWindowShellTitle(taskWindowLaunchState, tr));
  let taskWindowShellCopy = $derived(taskWindowShellMessage(taskWindowLaunchState, tr));
  let jobSubmitInFlight = $state(false);
  let submittingJobSpec = $state<JobSpec | null>(null);
  let jobPasswordValue = $state("");
  let standalonePasswordInput = $state<HTMLInputElement | null>(null);
  let standalonePasswordFocusedInput: HTMLInputElement | null = null;
  let passwordSubmissionAttempted = $state(false);
  let passwordSubmissionError = $derived(
    passwordSubmissionAttempted && !taskPasswordReady(jobPasswordValue)
      ? tr("gui.password.empty_error", "Enter a password to continue.")
      : null,
  );
  let conflictApplyAll = $state(false);
  let appNotice = $state<string | null>(null);
  let sourceCleanupRecoveryReady = $state(false);
  let sourceCleanupRecoveryStartupRequested = false;
  let sourceCleanupRecoveryLastGeneration = 0;
  let sourceCleanupRecoveryRequestInFlight = false;
  let sourceCleanupRecoveryRefreshPending = false;
  let sourceCleanupRecoveryRetry: ReturnType<typeof setTimeout> | null = null;
  const sourceCleanupBusyToastKey = "source-cleanup-recovery-busy";
  let checksumResultPanel = $state<HTMLElement | null>(null);
  let checksumCheckResultPanel = $state<HTMLElement | null>(null);
  let checksumCopyFeedbackKind = $state<"checksum" | "checksum_check" | "task" | null>(null);
  let checksumCopyFeedbackTaskId = $state<number | null>(null);
  let checksumCopyFeedbackMessage = $state<string | null>(null);
  let checksumCopyFeedbackTone = $state<"success" | "danger" | null>(null);
  let browseScrollTop = $state(0);
  let browseViewportHeight = $state(0);
  const refreshedUpdateJobs = new Set<number>();
  const recoveryContextTaskIds = new Set<number>();
  let recoverySubmissionPending = $state(false);
  let outputAuthorizationPending = $state(false);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  let checksumCopyFeedbackTimer: ReturnType<typeof setTimeout> | null = null;
  const screenParam = params.get("screen");
  let screen = $state<Screen>(
    screenIds.includes(screenParam as Screen) ? (screenParam as Screen) : "browse",
  );
  const archiveReturnScreens: Screen[] = ["checksum", "duplicates", "recovery"];
  const previewLanguageKey = import.meta.env.DEV ? "squallz.previewLanguage.v1" : "";

  function previewStorage(): Storage | null {
    if (!import.meta.env.DEV) return null;
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
  let savedModeChoice = $state<Mode | null>(initialMode);
  let savedThemeChoice = $state<ThemeChoice>(initialThemeChoice ?? "system");
  let savedDensityChoice = $state<DensityChoice>(
    hasDensityOverride ? densityParam : "standard",
  );
  let appearanceSaveStates = $state<Record<AppearanceSetting, AppearanceSaveState>>({
    mode: "saved",
    theme: "saved",
    density: "saved",
  });
  const appearanceSaveGenerations: Record<AppearanceSetting, number> = {
    mode: 0,
    theme: 0,
    density: 0,
  };
  let appearanceSaveState = $derived<AppearanceSaveState>(
    Object.values(appearanceSaveStates).includes("error")
      ? "error"
      : Object.values(appearanceSaveStates).includes("session")
        ? "session"
        : Object.values(appearanceSaveStates).includes("saving")
          ? "saving"
          : "saved",
  );
  const initialPlatform = buildTargetPlatform();
  let activePlatform = $state<PlatformKind>(initialPlatform);
  let prefersDarkTheme = $state(
    typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  let activeTheme = $derived<ResolvedTheme>(
    activeThemeChoice === "system" ? (prefersDarkTheme ? "dark" : "light") : activeThemeChoice,
  );
  const bytesPerKiB = 1024;
  const bytesPerMiB = 1024 ** 2;
  const bytesPerGiB = 1024 ** 3;
  const defaultSafety = {
    maxOutputGiB: 256,
    maxEntries: 1_000_000,
    maxCompressionRatio: 2048,
  };
  const extractDestinationModes: ExtractDestinationMode[] = ["smart", "archive", "same", "choose"];
  const extractOverwriteModes: ExtractOverwriteMode[] = ["ask", "skip", "overwrite", "rename"];
  const extractSymlinkModes: ExtractSymlinkMode[] = ["preserve", "skip", "follow"];
  const presetSqzInnerFormats: PresetSqzInnerFormat[] = ["entry_set", "zip", "seven_zip"];
  const numberFormatter = new Intl.NumberFormat("en-US");
  let safetyMaxOutputGiB = $state<NumericSetting>(defaultSafety.maxOutputGiB);
  let safetyMaxEntries = $state<NumericSetting>(defaultSafety.maxEntries);
  let safetyMaxCompressionRatio = $state<NumericSetting>(defaultSafety.maxCompressionRatio);
  let performanceParallelJobs = $state<NumericSetting>(null);
  let performanceThreads = $state<NumericSetting>(null);
  let performanceMemoryKiB = $state<NumericSetting>(null);
  let settingsSnapshotLabel = $state(
    tr("gui.settings.snapshot.defaults_active", "Saved settings · defaults"),
  );
  let availableLanguages = $state<LanguageDto[]>([]);
  let generalLanguageChoice = $state("");
  let generalDefaultCreateDir = $state("");
  let generalDefaultExtractDir = $state(defaultExtractDirParam?.trim() ?? "");
  let appliedGeneralLanguageChoice = $state("");
  let appliedDefaultCreateDir = $state("");
  let appliedDefaultExtractDir = $state(defaultExtractDirParam?.trim() ?? "");
  let generalRevealAfterExtract = $state(false);
  let generalAutomaticUpdateChecks = $state(true);
  let appliedGeneralRevealAfterExtract = $state(false);
  let appliedGeneralAutomaticUpdateChecks = $state(true);
  let savedAccentPalette = $state<PaletteId>("aqua");
  let savedCustomAccent = $state(defaultCustomAccent);
  let savedAccentContrastGuard = $state(true);
  let savedGeneralLanguageChoice = $state("");
  let savedGeneralDefaultCreateDir = $state("");
  let savedGeneralDefaultExtractDir = $state(defaultExtractDirParam?.trim() ?? "");
  let savedGeneralRevealAfterExtract = $state(false);
  let savedGeneralAutomaticUpdateChecks = $state(true);
  let savedSafetyMaxOutputGiB = $state<NumericSetting>(defaultSafety.maxOutputGiB);
  let savedSafetyMaxEntries = $state<NumericSetting>(defaultSafety.maxEntries);
  let savedSafetyMaxCompressionRatio = $state<NumericSetting>(defaultSafety.maxCompressionRatio);
  let savedSafetyCustom = $state(false);
  let savedPerformanceParallelJobs = $state<NumericSetting>(null);
  let savedPerformanceThreads = $state<NumericSetting>(null);
  let savedPerformanceMemoryKiB = $state<NumericSetting>(null);
  let settingsSaveTarget = $state<PersistedSettingsSection | null>(null);
  let settingsSaveOutcomes = $state<Record<PersistedSettingsSection, SettingsSaveOutcome>>({
    general: "saved",
    security: "saved",
    performance: "saved",
    colors: "saved",
  });
  const settingsDraftGenerations: Record<PersistedSettingsSection, number> = {
    general: 0,
    security: 0,
    performance: 0,
    colors: 0,
  };
  let defaultCreateFolderError = $derived(folderSettingValidationError(
    generalDefaultCreateDir,
    tr("gui.settings.folder.default_create", "Default create folder"),
  ));
  let defaultExtractFolderError = $derived(folderSettingValidationError(
    generalDefaultExtractDir,
    tr("gui.settings.folder.default_extract", "Default extract folder"),
  ));
  let generalSettingsValidationError = $derived(
    defaultCreateFolderError || defaultExtractFolderError,
  );
  let generalSettingsDirty = $derived(
    generalSettingsValidationError !== "" ||
      generalLanguageChoice.trim() !== savedGeneralLanguageChoice ||
      (normalizedDefaultCreateDir() ?? "") !== savedGeneralDefaultCreateDir ||
      (normalizedDefaultExtractDir() ?? "") !== savedGeneralDefaultExtractDir ||
      generalRevealAfterExtract !== savedGeneralRevealAfterExtract ||
      generalAutomaticUpdateChecks !== savedGeneralAutomaticUpdateChecks ||
      generalLanguageChoice.trim() !== appliedGeneralLanguageChoice ||
      (normalizedDefaultCreateDir() ?? "") !== appliedDefaultCreateDir ||
      (normalizedDefaultExtractDir() ?? "") !== appliedDefaultExtractDir ||
      generalRevealAfterExtract !== appliedGeneralRevealAfterExtract ||
      generalAutomaticUpdateChecks !== appliedGeneralAutomaticUpdateChecks,
  );
  let safetySettingsDirty = $derived(
    safetyMaxOutputGiB !== savedSafetyMaxOutputGiB ||
      safetyMaxEntries !== savedSafetyMaxEntries ||
      safetyMaxCompressionRatio !== savedSafetyMaxCompressionRatio,
  );
  let performanceSettingsDirty = $derived(
    performanceParallelJobs !== savedPerformanceParallelJobs ||
      performanceThreads !== savedPerformanceThreads ||
      performanceMemoryKiB !== savedPerformanceMemoryKiB,
  );
  let colorSettingsDirty = $derived(
    activePalette !== savedAccentPalette ||
      customAccentInput.trim().toUpperCase() !== savedCustomAccent ||
      accentContrastGuard !== savedAccentContrastGuard,
  );
  let safetyMaxOutputError = $derived(requiredWholeSettingError(
    safetyMaxOutputGiB,
    1,
    8192,
    tr("gui.settings.security.max_output_gib", "Max output GiB"),
  ));
  let safetyMaxEntriesError = $derived(requiredWholeSettingError(
    safetyMaxEntries,
    1,
    10_000_000,
    tr("gui.settings.security.max_entries", "Max entries"),
  ));
  let safetyMaxCompressionRatioError = $derived(requiredWholeSettingError(
    safetyMaxCompressionRatio,
    1,
    100_000,
    tr("gui.settings.security.ratio_guard", "Ratio guard"),
  ));
  let performanceThreadsError = $derived(optionalWholeSettingError(
    performanceThreads,
    1,
    64,
    tr("gui.settings.performance.custom_threads", "Custom threads"),
  ));
  let performanceParallelJobsError = $derived(optionalWholeSettingError(
    performanceParallelJobs,
    1,
    8,
    tr("gui.settings.performance.custom_parallel_jobs", "Custom parallel tasks"),
  ));
  let performanceMemoryError = $derived(optionalWholeSettingError(
    performanceMemoryKiB,
    8,
    64,
    tr("gui.settings.performance.custom_buffer_kib", "Custom buffer KiB"),
  ));
  let extractDestinationMode = $state<ExtractDestinationMode>("smart");
  let extractScope = $state<ExtractScope>("all");
  let extractSelectionSnapshot = $state<string[]>([]);
  let extractCustomDest = $state("");
  let extractOverwriteMode = $state<ExtractOverwriteMode>("ask");
  let extractSymlinkMode = $state<ExtractSymlinkMode>("preserve");
  let currentExtractOverwriteLabel = $derived(extractOverwriteLabel(extractOverwriteMode));
  let currentExtractSymlinkLabel = $derived(extractSymlinkLabel(extractSymlinkMode));
  let extractPresetEncodingLabel = $state<string | null>(null);
  let extractPlan = $state<ExtractPlanPreflightDto | null>(null);
  let extractPlanPhase = $state<"idle" | "loading" | "ready" | "blocked" | "error">("idle");
  let extractPlanErrorKey = $state("");
  let extractPlanRequestKey = "";
  let extractPlanGeneration = 0;
  let extractPlanDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  let extractPlanQueued: ExtractPlanRequest | null = null;
  let extractPlanActive: ExtractPlanRequest | null = null;
  let presetDocument = $state<ArchivePresetDocument | null>(null);
  let presetLoadState = $state<"loading" | "ready" | "error">("loading");
  let selectedCreatePresetId = $state<string | null>(null);
  let selectedExtractPresetId = $state<string | null>(null);
  let createPresetDraftName = $state("");
  let extractPresetDraftName = $state("");
  let createPresetMutationState = $state<ArchivePresetMutationState>("idle");
  let extractPresetMutationState = $state<ArchivePresetMutationState>("idle");
  let createPresetCredentialIntent = $state<"none" | "prompt">("none");
  let createPresetSfxTarget = $state<PresetSfxTarget>("current_platform");
  let createPresetSqzInnerFormat = $state<PresetSqzInnerFormat>("entry_set");
  let createPresetSplitSizeBytes = $state<string | null>(null);
  let createPresetDraftTouched = isCreateFormatId(createFormatParam);
  let extractPresetDraftTouched = false;
  let archiveOpenStatus = $state<"idle" | "opening">("idle");
  let archiveOpenGeneration = 0;
  let archiveSelectAllProgress = $state<{ loaded: number; total: number } | null>(null);
  let recoveryPickerStatus = $state<"idle" | "archive" | "par2">("idle");
  let recoverySourceMode = $state<"none" | "current" | "selected">(
    runtimePreviews.archive ? "current" : "none",
  );
  let recoverySourceOverride = $state<string | null>(null);
  let recoveryPar2Override = $state<string | null>(null);
  let recoveryRedundancyDraft = $state("10");
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
  let createSources = $state<CreateSourceRoot[]>([]);
  let selectedCreateSourcePaths = $state<string[]>([]);
  let createSourcePickerBusy = $state<"files" | "folder" | null>(null);
  let createSourceInputs = $derived(createSourcePaths(createSources));
  let classicCreateSection = $state<ClassicCreateSection>("general");
  let dragActive = $state(false);
  let lastDropKind = $state<"none" | "archives" | "create" | "recovery">("none");
  let customCreateLevel = $state(loadCustomCreateLevel());
  let customCreateLevelError = $state("");
  let activeCreateProfile = $state<CreateProfileId>(loadCreateProfile());
  let activeCreateFormat = $state<CreateFormatId>(isCreateFormatId(createFormatParam) ? createFormatParam : loadCreateFormat());
  const convertRouteOwner: ConvertRouteOwner = {};
  let convertRouteHandle = $state<ConvertRouteHandle | null>(null);
  let convertRouteStatus = $derived<ConvertRouteStatus>(convertRouteHandle?.status() ?? {
    sourceFormat: currentArchive?.format.toUpperCase() ?? "-",
    targetLabel: "-",
    profileLabel: "-",
    methodLabel: "-",
    destination: currentArchive?.path ?? openArchiveFirstLabel(),
  });
  let createPassword = $state("");
  let createPasswordConfirmation = $state("");
  let createPasswordVisible = $state(false);
  let createEncryptNames = $state(false);
  let createSplitPreset = $state<CreateSplitPreset>("none");
  let createSplitMode = $state<CreateSplitMode>("generic");
  let createCustomSplitAmount = $state("100");
  let createCustomSplitUnit = $state<CreateSplitUnit>("mib");
  let createContentPolicy = $state<CreateContentPolicy>("cross_platform_clean");
  let createDestinationBase = $state<CreateDestinationBase>("ask");
  let createExistingOutputPolicy = $state<ExistingOutputPolicy>("ask");
  let createCompletion = $state<CreateCompletionAction>("none");
  let createPostSuccess = $state<PostSuccessAction>("keep_source");
  let createTestAfterCreate = $state(false);
  let createOptionsValidationAttempted = $state(false);
  let createAdvancedOpen = $state(false);
  let createSfxEnabled = $state(false);
  let sfxCreateCapability = $state<SfxCreateCapabilityDto>({
    target: initialPlatform,
    extension: initialPlatform === "macos" ? "app" : initialPlatform === "windows" ? "exe" : "run",
    available: true,
    status: "available",
    requires_signing: true,
  });
  let sfxCreateCapabilityReady = $state(false);
  let createExcludeText = $state("");
  let lastCreatePlan = $state<CreatePlanDto | null>(null);
  let lastDiskSpace = $state<DiskSpaceDto | null>(null);
  let lastTempDiskSpace = $state<DiskSpaceDto | null>(null);
  let lastSystemTempDiskSpace = $state<DiskSpaceDto | null>(null);
  let lastCreateDest = $state<string | null>(null);
  let pendingCreateSubmission = $state<PendingCreateSubmission | null>(null);
  let createPreflightPhase = $state<CreatePreflightPhase>(
    runtimePreviews.preflightDestinationBytes > 0
      ? "choosingDest"
      : runtimePreviews.preflightScanned > 0
        ? "measuring"
        : "idle",
  );
  let createPreflightScanned = $state(runtimePreviews.preflightScanned);
  let createPreflightCurrent = $state(
    runtimePreviews.preflightDestinationCurrent || runtimePreviews.preflightCurrent,
  );
  let createPreflightExcludeCount = $state(0);
  let createPreflightIssue = $state("");
  let createPreflightIssueStage = $state<CreatePreflightStage | null>(null);
  let createPreflightCreatingSfx = $state(false);
  let createPreflightCleanup: (() => void) | null = null;
  let createPreflightListenPromise: Promise<void> | null = null;
  let createPreflightRequestId: string | null = previewDestinationRequestId;
  let createPreflightRequestKind = $state<"source" | "destination" | null>(
    previewDestinationRequestId ? "destination" : null,
  );
  let createPreflightProcessedBytes = $state(runtimePreviews.preflightDestinationBytes);
  let createPreflightCancelPending = $state(false);
  let createPreflightClosed = false;
  let nestedPreview = $state<NestedArchivePreviewDto | null>(null);
  let entryPreview = $state<EntryPreviewDto | null>(null);
  let entryPreviewFailure = $state<PreviewFailure | null>(null);
  let previewOriginEntryPath: string | null = null;
  let previewOriginVirtualIndex: number | null = null;
  let previewPhase = $state<PreviewPhase>("idle");
  let previewTargetName = $state("");
  let previewRequestGeneration = 0;
  let previewActionGeneration = 0;
  let entryPreviewPreparationTail: Promise<void> = Promise.resolve();
  let renameTargetName = $state("renamed.txt");
  let moveTargetDir = $state("moved/");
  let newFolderName = $state("New Folder");
  let moveConflictReview = $state<MoveConflictReview | null>(null);
  let historyRows = $derived(operationHistory());
  let activePopover = $state<"quickActions" | null>(null);
  let archiveSearchInput = $state<HTMLInputElement | null>(null);
  let classicArchiveAddress = $state<HTMLDivElement | null>(null);
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

  function customPaletteVariables(): CssVariableMap {
    if (activePalette !== "custom") return {};
    return customPaletteVariablesFor(activeTheme);
  }

  function customPaletteVariablesFor(theme: ResolvedTheme): CssVariableMap {
    return deriveCustomPaletteTokens(customAccent, theme, accentContrastGuard);
  }

  function entryContextCssVariables(context: EntryContext): CssVariableMap {
    return {
      ...customPaletteVariables(),
      "--entry-context-left": `${context.x}px`,
      "--entry-context-top": `${context.y}px`,
    };
  }

  const customAccentValid = $derived(normalizeHexColor(customAccentInput) !== null);
  const paletteApplyBlocked = $derived(activePalette === "custom" && !customAccentValid);
  let generalSaveState = $derived(settingsSaveState("general", generalSettingsDirty));
  let securitySaveState = $derived(settingsSaveState("security", safetySettingsDirty));
  let performanceSaveState = $derived(settingsSaveState("performance", performanceSettingsDirty));
  let colorsSaveState = $derived(settingsSaveState("colors", colorSettingsDirty));
  let safetyValidationError = $derived(
    safetyMaxOutputError || safetyMaxEntriesError || safetyMaxCompressionRatioError,
  );
  let performanceValidationError = $derived(
    performanceParallelJobsError || performanceThreadsError || performanceMemoryError,
  );
  $effect(() => {
    document.documentElement.dataset.theme = activeTheme;
    document.documentElement.dataset.palette = activePalette;
    document.documentElement.dataset.density = activeDensityChoice;
  });

  $effect(() => {
    const address = classicArchiveAddress;
    archiveDirs.join("\u0000");
    if (!address) return;
    void tick().then(() => {
      if (classicArchiveAddress !== address) return;
      address.scrollLeft = address.scrollWidth;
    });
  });

  $effect(() => {
    const current = currentArchive;
    convertRouteHandle?.syncArchive(current);
  });

  $effect(() => {
    if (jobPasswordPrompt) {
      setScreen("password");
    } else if (jobConflictPrompt) {
      setScreen("conflict");
    } else if (archivePasswordPrompt) {
      setScreen("password");
    }
  });

  $effect(() => {
    const identity = activePasswordPromptIdentity;
    if (identity === previousPasswordPromptIdentity) return;
    previousPasswordPromptIdentity = identity;
    jobPasswordValue = "";
    passwordSubmissionAttempted = false;
  });

  $effect(() => {
    const promptPath = archivePasswordPrompt?.path ?? null;
    const input = standalonePasswordInput;
    const ready =
      promptPath !== null &&
      jobPasswordPrompt === null &&
      screen === "password" &&
      archiveOpenStatus === "idle" &&
      !modeSelectionBlocked &&
      !blockingModalVisible() &&
      !taskWindowMode &&
      input !== null;
    if (!ready) {
      standalonePasswordFocusedInput = null;
      return;
    }
    if (standalonePasswordFocusedInput === input) return;
    standalonePasswordFocusedInput = input;
    void tick().then(() => {
      if (
        standalonePasswordFocusedInput === input &&
        standalonePasswordInput === input &&
        archivePasswordPrompt?.path === promptPath &&
        jobPasswordPrompt === null &&
        screen === "password" &&
        archiveOpenStatus === "idle" &&
        !modeSelectionBlocked &&
        !blockingModalVisible() &&
        !taskWindowMode
      ) {
        input.focus({ preventScroll: true });
      }
    });
  });

  $effect(() => {
    const identity = activeConflictPromptIdentity;
    if (identity === previousConflictPromptIdentity) return;
    previousConflictPromptIdentity = identity;
    conflictApplyAll = false;
  });

  $effect(() => {
    const questionTaskId = jobPasswordPrompt?.id ?? jobConflictPrompt?.id ?? null;
    if (questionTaskId !== null) {
      taskDialogTaskId = questionTaskId;
      taskDialogDismissedId = null;
      return;
    }
    if (!taskWindowMode) return;
    const active = blockingTask();
    if (!active) return;
    taskDialogTaskId = active.id;
    taskDialogDismissedId = null;
  });

  $effect(() => {
    if (!blockingModalVisible() && !modeSelectionBlocked) return;
    activePopover = null;
    entryContext = null;
    if (modeSelectionBlocked) taskCenterOpen = false;
  });

  $effect(() => {
    if (!firstRunRequired || taskWindowMode || blockingModalVisible()) return;
    void tick().then(() => {
      if (!firstRunRequired || taskWindowMode || blockingModalVisible()) return;
      if (firstRunPanel?.contains(document.activeElement)) return;
      (firstRunRecommendedButton ?? firstRunPanel)?.focus();
    });
  });

  $effect(() => {
    const selectedId = taskCenterSelectedTaskId;
    if (selectedId === null || jobRows.some((task) => task.id === selectedId)) return;
    taskCenterSelectedTaskId = null;
    taskCenterFocusTaskId = selectedId;
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

  $effect(() => {
    const waitReason = runtimePreviews.taskQueue;
    if (!waitReason) return;
    if (installTaskQueuePreview(waitReason) === null) return;
    taskCenterReturnFocus = null;
    taskCenterFocusTaskId = null;
    taskCenterSelectedTaskId = null;
    taskCenterOpen = true;
  });

  $effect(() => {
    if (
      !jobEventsReady ||
      !taskWindowMode ||
      !initialTaskWindowLaunch ||
      initialTaskWindowSubmitted
    ) return;
    initialTaskWindowSubmitted = true;
    void submitExternalTaskWindow(
      initialTaskWindowLaunch.action,
      initialTaskWindowLaunch.paths,
      initialTaskWindowLaunch.output,
    );
  });

  $effect(() => {
    if (!taskWindowMode) return;
    const task = taskDialogTask();
    if (!task || task.id === null || task.expanded || task.state !== "done") return;
    if (taskResultScreen(task) || taskHasInlineResults(task)) {
      setTaskExpanded(task.id, true);
    }
  });

  $effect(() => {
    if (!sourceCleanupRecoveryReady || sourceCleanupRecoveryStartupRequested) return;
    sourceCleanupRecoveryStartupRequested = true;
    void reportStartupSourceCleanupRecovery();
  });

  $effect(() => {
    const current = currentArchive;
    if (screen !== "extract" || !current) {
      resetExtractPlanRequestState();
      return;
    }
    const selection = extractJobPaths();
    const dest = extractJobDestination();
    const smart = extractDestinationMode === "smart";
    const encoding = extractEncodingForJob();
    const key = extractPlanKey(
      current.id,
      current.source,
      current.path,
      dest,
      selection,
      smart,
      encoding,
    );
    if (key === extractPlanRequestKey) return;
    void requestExtractPlan(
      current.id,
      current.source,
      current.path,
      dest,
      selection,
      smart,
      encoding,
      true,
    );
  });

  onMount(() => () => {
    cancelActiveExtractPlan();
    extractPlanGeneration += 1;
    discardQueuedExtractPlan();
  });

  function isTextEditingTarget(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    return target.isContentEditable
      || target instanceof HTMLInputElement
      || target instanceof HTMLTextAreaElement
      || target instanceof HTMLSelectElement;
  }

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
    if (taskWindowMode) return;
    return setCreateCompletionHandler((path) => openArchivePath(path, "open-file"));
  });

  onMount(() => {
    const stopRefresh = setSourceCleanupRecoveryRefreshHandler(
      reportStartupSourceCleanupRecovery,
    );
    return () => {
      stopRefresh();
      if (sourceCleanupRecoveryRetry !== null) {
        clearTimeout(sourceCleanupRecoveryRetry);
        sourceCleanupRecoveryRetry = null;
      }
    };
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
    const ownsMainOpenFileQueue = !taskWindowMode;

    const openQueuedPaths = (payload: OpenFilesPayload) => {
      if (cancelled) return;
      void handleOpenFilesPayload(payload);
    };

    const startRealtimeOpenFileListener = async () => {
      try {
        const listen = await currentWebviewWindowListener();
        if (cancelled) return;
        const dispose = await listen<OpenFilesPayload>("app://open-files", (event) => {
          openQueuedPaths(event.payload);
        });
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
        if (ownsMainOpenFileQueue) {
          const queued = await ipc.openFileListenerReady();
          openQueuedPaths(queued);
        }
      } catch {
        // Dev preview has no native Tauri event bus.
      }
    };

    if (ownsMainOpenFileQueue) {
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
    }

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
          jobEventsReady = true;
        }
      })
      .catch(() => {
        // Dev preview has no native Tauri job event bus.
        if (!cancelled) jobEventsReady = true;
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
    convertRouteHandle?.dispose();
  });

  onMount(() => {
    void loadFormats().catch(() => {
      // Dev preview uses the release-scope fallback below.
    });
  });

  onMount(() => {
    let cancelled = false;
    void ipc.getArchivePresets()
      .then(async (document) => {
        if (cancelled) return;
        const readyDocument = await migrateLegacyCreatePresets(document).catch(() => {
          showNotice(tr("gui.presets.legacy_import_failed", "Older compression presets were left in place because they could not be imported"));
          return document;
        });
        if (cancelled) return;
        presetDocument = readyDocument;
        presetLoadState = "ready";
        applyDefaultCreatePresetWhenReady();
        if (!extractPresetDraftTouched && readyDocument.bindings.app_default_extract) {
          applyExtractPreset(readyDocument.bindings.app_default_extract, false);
        }
      })
      .catch(() => {
        if (cancelled) return;
        presetLoadState = "error";
        createPresetMutationState = "error";
        extractPresetMutationState = "error";
      });
    return () => {
      cancelled = true;
    };
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
    void ipc.getSfxCreateCapability()
      .then((capability) => {
        sfxCreateCapability = capability;
        sfxCreateCapabilityReady = true;
        if (!capability.available) createSfxEnabled = false;
        applyDefaultCreatePresetWhenReady();
      })
      .catch(() => {
        sfxCreateCapabilityReady = true;
        createSfxEnabled = false;
        if (import.meta.env.DEV) {
          // Browser preview has no native capability service.
          applyDefaultCreatePresetWhenReady();
        } else {
          sfxCreateCapability = { ...sfxCreateCapability, available: false, status: "invalid" };
          applyDefaultCreatePresetWhenReady();
        }
      });
  });

  onMount(() => {
    if (forceFirstRun) {
      void loadLocale(null).finally(() => {
        sourceCleanupRecoveryReady = true;
      });
      return;
    }

    let cancelled = false;
    const requestedDraftGenerations = { ...settingsDraftGenerations };
    const requestedAppearanceGenerations = { ...appearanceSaveGenerations };
    void ipc.getSettings()
      .then(async (settings) => {
        if (cancelled) return;
        if (
          !initialMode &&
          appearanceSaveGenerations.mode === requestedAppearanceGenerations.mode
        ) {
          initUiMode(settings.ui_mode);
        }
        if (
          !initialThemeChoice &&
          appearanceSaveGenerations.theme === requestedAppearanceGenerations.theme
        ) {
          activeThemeChoice = isThemeChoice(settings.theme) ? settings.theme : "system";
        }
        applySettingsSnapshot(
          settings,
          requestedDraftGenerations,
          appearanceSaveGenerations.density !== requestedAppearanceGenerations.density,
        );
        await loadLocale(settings.language).catch(() => undefined);
        if (cancelled) return;
        updateSettingsSnapshotLabel();
        settingsStatus = initialMode ? "preview" : "ready";
        sourceCleanupRecoveryReady = true;
        startAutomaticUpdateCheck(settings.check_updates_automatically !== false);
      })
      .catch(async () => {
        if (cancelled) return;
        if (
          !initialMode &&
          appearanceSaveGenerations.mode === requestedAppearanceGenerations.mode
        ) {
          initUiMode(null);
        }
        const previewLanguage = storedPreviewLanguage();
        const savedPreviewLanguage = previewLanguage ?? "";
        savedGeneralLanguageChoice = savedPreviewLanguage;
        appliedGeneralLanguageChoice = savedPreviewLanguage;
        if (settingsDraftGenerations.general === requestedDraftGenerations.general) {
          generalLanguageChoice = savedPreviewLanguage;
        }
        await loadLocale(previewLanguage).catch(() => undefined);
        if (cancelled) return;
        settingsSnapshotLabel = tr(
          "gui.settings.snapshot.defaults_active",
          "Saved settings · defaults",
        );
        settingsStatus = "preview";
        sourceCleanupRecoveryReady = true;
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
    if (!import.meta.env.DEV || runtimePreviews.toast === null) return;
    if (runtimePreviews.toast === "danger") {
      showSourceCleanupRecovery({
        generation: 1,
        status: "needs_attention",
        path: null,
        reason: "journal_invalid",
        journal_path: "/Users/alex/Library/Application Support/Squallz/source-cleanup.json",
      });
      return;
    }
    const action = {
      label: tr("gui.common.close", "Close"),
      run: () => undefined,
    };
    pushToast({
      kind: "warning",
      title: tr(
        "gui.toast.compress_done_preserved",
        "Created {name} · Review {count} preserved backups",
      )
        .replace("{name}", "reports.7z.001")
        .replace("{count}", "3"),
      body: tr(
        "gui.toast.compress_done_preserved_detail",
        "Verify the new archive before deleting any preserved backup.",
      ),
      action,
    });
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
            dragActive = !modeSelectionBlocked;
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
      if (modeSelectionBlocked) {
        if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
        dragActive = false;
        return;
      }
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
      if (modeSelectionBlocked) return;
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
      if (modeSelectionBlocked) return;
      if (
        (event.metaKey || event.ctrlKey) &&
        event.key.toLowerCase() === "f" &&
        screen === "browse" &&
        currentArchive &&
        !taskWindowMode
      ) {
        event.preventDefault();
        closeQuickActions(false);
        archiveSearchInput?.focus();
        archiveSearchInput?.select();
        return;
      }
      if (
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "a" &&
        screen === "browse" &&
        currentArchive &&
        !taskWindowMode &&
        !isTextEditingTarget(event.target)
      ) {
        event.preventDefault();
        closeQuickActions(false);
        void selectAllArchiveEntries();
        return;
      }
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
        archiveOpenStatus === "idle" &&
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

  function appearanceSettingMatchesSaved(setting: AppearanceSetting): boolean {
    if (setting === "mode") return uiModeChoice() === savedModeChoice;
    if (setting === "theme") return activeThemeChoice === savedThemeChoice;
    return activeDensityChoice === savedDensityChoice;
  }

  function trackAppearanceSave(
    setting: AppearanceSetting,
    request: Promise<unknown>,
    previewFailureLabel: string,
    commitPersistedValue: () => void,
  ) {
    const generation = ++appearanceSaveGenerations[setting];
    appearanceSaveStates[setting] = "saving";
    void request
      .then(() => {
        commitPersistedValue();
        if (appearanceSaveGenerations[setting] === generation) {
          appearanceSaveStates[setting] = "saved";
        }
      })
      .catch((error) => {
        if (appearanceSaveGenerations[setting] !== generation) return;
        if (appearanceSettingMatchesSaved(setting)) {
          appearanceSaveStates[setting] = "saved";
          return;
        }
        appearanceSaveStates[setting] = isSettingsPersistenceFailure(error) ? "error" : "session";
        showNotice(
          isSettingsPersistenceFailure(error)
            ? settingsPersistenceFailureLabel()
            : previewFailureLabel,
        );
      });
  }

  function setMode(next: Mode) {
    firstRunDropFeedback = null;
    trackAppearanceSave(
      "mode",
      persistUiMode(next),
      tr("gui.mode.saved_preview_desktop_unavailable", "Interface mode changed for this session but was not saved"),
      () => {
        savedModeChoice = next;
      },
    );
    syncUrl(next);
  }

  async function startFirstRun(action: "create" | "open"): Promise<void> {
    setMode("modern");
    await tick();
    if (action === "create") {
      setScreen("create");
      return;
    }
    await openArchiveFromDialog();
  }

  function reviewFirstRunSettings(): void {
    setMode("modern");
    setScreen("settingsGeneral");
  }

  function firstRunFocusableElements(): HTMLElement[] {
    if (!firstRunPanel) return [];
    return Array.from(
      firstRunPanel.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => !element.hasAttribute("hidden"));
  }

  function onFirstRunKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = firstRunFocusableElements();
    if (focusable.length === 0) {
      event.preventDefault();
      firstRunPanel?.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (document.activeElement === firstRunPanel) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function preventCreateSubmissionNavigation(next: Screen): boolean {
    if (screen !== "create" || next === "create") return false;
    if (createPreflightPhase === "submitting") {
      showNotice(
        tr(
          "gui.create.wait_for_submission_before_leaving",
          "Wait until Squallz finishes adding this create task to the queue",
        ),
      );
      return true;
    }
    if (createPreflightPhase === "choosingDest" && createDestinationInspectionCancellable()) {
      void cancelCreateDestinationInspection({ announce: false, keepIntentOnFailure: true });
    }
    return false;
  }

  function preventConvertSubmissionNavigation(next: Screen): boolean {
    if (screen !== "convert" || next === "convert") return false;
    if (convertRouteHandle && !convertRouteHandle.canLeave()) return true;
    convertRouteHandle?.leave();
    return false;
  }

  function setScreen(next: Screen) {
    if (preventCreateSubmissionNavigation(next)) return;
    if (preventConvertSubmissionNavigation(next)) return;
    if (screen === "create" && next !== "create" && pendingCreateSubmission) {
      discardPendingCreatePlan();
      createOptionsValidationAttempted = false;
    }
    if (next === "create" && screen !== "create") {
      classicCreateSection = "general";
    }
    screen = next;
    syncUrl();
    void tick().then(() => {
      document.documentElement.scrollTop = 0;
      document.body.scrollTop = 0;
      for (const element of document.querySelectorAll<HTMLElement>(
        ".modern-content, .modern-content.settings-workspace > :not(.settings-workspace-rail), .classic-dialog-body",
      )) {
        element.scrollTop = 0;
        element.scrollLeft = 0;
      }
    });
  }

  function setScreenRespectingJobQuestion(fallback: Screen) {
    if (jobPasswordPrompt) {
      setScreen("password");
      return;
    }
    if (jobConflictPrompt) {
      setScreen("conflict");
      return;
    }
    setScreen(fallback);
  }

  async function showClassicCreateSection(section: ClassicCreateSection, targetId: string): Promise<void> {
    classicCreateSection = section;
    await tick();
    document.getElementById(targetId)?.scrollIntoView({ block: "start", inline: "nearest" });
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
      answerConflictDecision("abort", false);
    } else {
      setScreen("extract");
    }
  }

  function handleWorkflowEscape(): boolean {
    if (screen === "browse" && (previewBusy() || nestedPreview || entryPreview || entryPreviewFailure)) {
      clearEntryPreviewState();
      return true;
    }
    if (screen === "browse" && filterText()) {
      clearArchiveFilter();
      return true;
    }
    if (screen === "password") {
      cancelPasswordRequest();
      return true;
    }
    if (screen === "conflict") {
      cancelConflictPrompt();
      return true;
    }
    return false;
  }

  function applyCreatePreflightEvent(event: CreatePreflightEvent) {
    if (convertRouteHandle?.applyPreflightEvent(event)) return;
    if (!createPreflightRequestId || event.request_id !== createPreflightRequestId) return;
    if (event.phase === "destination" && createPreflightRequestKind === "destination") {
      const processedBytes = Number(event.processed_bytes ?? 0);
      if (Number.isFinite(processedBytes)) createPreflightProcessedBytes = processedBytes;
      createPreflightCurrent = String(event.current ?? "");
      return;
    }
    const scanned = Number(event.scanned ?? 0);
    if (Number.isFinite(scanned)) createPreflightScanned = scanned;
    createPreflightCurrent = String(event.current ?? "");
  }

  function nextPreflightRequestId(): string {
    return globalThis.crypto?.randomUUID?.()
      ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
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
    markSettingsDraft("colors");
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
    markSettingsDraft("colors");
    syncUrl();
  }

  function onCustomAccentHexInput(event: Event) {
    updateCustomAccent((event.currentTarget as HTMLInputElement).value, "hex");
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

  function setTheme(next: ThemeChoice) {
    activeThemeChoice = next;
    syncUrl();
    trackAppearanceSave(
      "theme",
      ipc.setTheme(next),
      tr("gui.theme.saved_preview_desktop_unavailable", "Theme changed for this session but was not saved"),
      () => {
        savedThemeChoice = next;
      },
    );
  }

  function setDensity(next: DensityChoice) {
    activeDensityChoice = next;
    syncUrl();
    trackAppearanceSave(
      "density",
      ipc.setUiDensity(next),
      tr("gui.density.saved_preview_desktop_unavailable", "Spacing changed for this session but was not saved"),
      () => {
        savedDensityChoice = next;
      },
    );
  }

  function toggleQuickActions() {
    activePopover = activePopover === "quickActions" ? null : "quickActions";
  }

  function closeQuickActions(restoreFocus = true) {
    activePopover = null;
    if (restoreFocus) queueMicrotask(() => quickActionButton?.focus());
  }

  function chooseQuickAction(next: Screen) {
    navigateToScreen(next);
    closeQuickActions();
  }

  function navigateToScreen(next: Screen) {
    if (next === "extract") {
      openExtractWorkspace("all");
      return;
    }
    setScreen(next);
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
    if (format.id === "rar") return tr("gui.format.state.open_only", "External read-only");
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
    if (format.id === "rar") return tr(
      "gui.format.note.rar_read_only",
      "Squallz groups and validates RAR volume sets; external 7zz/7z decodes them read-only, including encrypted archives. RAR creation and recovery-record repair are not supported",
    );
    if (format.id === "wim") return tr("gui.format.note.wim_external", "WIM create requires wimlib-imagex; read uses 7zz/7z");
    if (format.id === "sqz") return tr("gui.format.note.sqz_recovery", "Embedded recovery container with export");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.format.note.7zz_bridge", "Unpack-only through the 7zz/7z bridge");
    if (format.id.startsWith("tar.")) return tr("gui.format.note.compound_tar", "Compound TAR stream; no encryption claim");
    return format.extensions.length > 0
      ? tr("gui.format.note.extensions", "Extensions {extensions}").replace("{extensions}", `.${format.extensions.slice(0, 3).join(", .")}`)
      : tr("gui.format.note.registry_capability", "Registry capability");
  }

  function formatSortRank(format: FormatDto): string {
    const featured = featuredFormatIds.indexOf(format.id);
    if (featured >= 0) return `0-${featured.toString().padStart(2, "0")}`;
    if (format.can_create && format.can_extract) return `1-${format.id}`;
    if (format.can_extract) return `2-${format.id}`;
    return `3-${format.id}`;
  }

  function formatVolumeLabel(format: FormatDto): string {
    if (format.id === "zip") {
      return tr("gui.format.volume.zip", "Create/open .001 or native .z01");
    }
    if (format.id === "rar") {
      return tr("gui.format.volume.rar", "Open native RAR volumes");
    }
    if (format.id === "wim") {
      return tr("gui.format.volume.wim", "Create/open native .swm volumes");
    }
    if (format.id === "sqz") {
      return tr("gui.format.volume.sqz", "Create/open SQZV .001");
    }
    if (format.can_split) {
      return tr("gui.format.volume.generic", "Create/open .001");
    }
    return tr("gui.format.volume.single", "Single archive");
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
        volumes: formatVolumeLabel(format),
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

  function persistCustomCreateLevel(next: number) {
    try {
      window.localStorage.setItem("squallz.customCreateLevel", String(next));
    } catch {
      // Custom profile is non-critical; private-mode failures should not block jobs.
    }
  }

  function createProfileData(profileId: CreateProfileId) {
    if (profileId === "custom") {
      return {
        label: tr("gui.create.profile.custom", "Custom"),
        level: customCreateLevel,
        detail: tr("gui.create.profile.custom.detail", "Choose an exact compression level"),
      };
    }
    return createProfiles[profileId];
  }

  function markCreatePresetDraftTouched() {
    createPresetDraftTouched = true;
    invalidateCreatePreflightResult();
  }

  function markExtractPresetDraftTouched() {
    extractPresetDraftTouched = true;
  }

  function updateCreateContentPolicy(policy: CreateContentPolicy) {
    if (policy === createContentPolicy) return;
    markCreatePresetDraftTouched();
    createContentPolicy = policy;
    showNotice(
      tr("gui.create.content_policy.changed", "Archive contents: {policy}")
        .replace("{policy}", createContentPolicyLabel(policy)),
    );
    normalizeUnsupportedCreatePostSuccess();
  }

  function createTrashSourceDisabledReason(): string {
    const excludesContent = createContentPolicy === "cross_platform_clean" ||
      (createContentPolicy === "custom" && createExcludeRules().length > 0);
    return excludesContent
      ? tr(
        "gui.create.output.source.trash_disabled_excludes",
        "Remove exclusion rules or choose Keep all files to move originals to {trash}",
      ).replace("{trash}", trashNameLabel())
      : "";
  }

  function normalizeUnsupportedCreatePostSuccess(announce = true): boolean {
    const reason = createTrashSourceDisabledReason();
    if (!reason || createPostSuccess !== "trash_source") return false;
    createPostSuccess = "keep_source";
    if (announce) {
      showNotice(
        tr(
          "gui.create.output.source.changed_to_keep",
          "Originals will stay in place · {reason}",
        ).replace("{reason}", reason),
      );
    }
    return true;
  }

  function createOpenCompletionDisabledReason(): string {
    if (createSfxEnabled) {
      return tr("gui.create.output.completion.open_disabled_sfx", "Self-extracting outputs must be revealed and tested on their target system");
    }
    if (createSplitSizeBytes() !== null) {
      return tr("gui.create.output.completion.open_disabled_split", "Split archives must stay together, so Squallz reveals the primary volume instead");
    }
    return "";
  }

  function normalizeUnsupportedCreateCompletion(announce = true) {
    const reason = createOpenCompletionDisabledReason();
    if (!reason || createCompletion !== "open_in_squallz") return;
    createCompletion = "reveal_output";
    if (announce) {
      showNotice(
        tr("gui.create.output.completion.changed_to_reveal", "When finished changed to Reveal · {reason}")
          .replace("{reason}", reason),
      );
    }
  }

  function updateCreateDestinationBase(value: CreateDestinationBase) {
    if (value === createDestinationBase) return;
    markCreatePresetDraftTouched();
    createDestinationBase = value;
    createExistingOutputPolicy = value === "ask" ? "ask" : "rename";
  }

  function updateCreateCompletion(value: CreateCompletionAction) {
    if (value === "open_in_squallz" && createOpenCompletionDisabledReason()) {
      showNotice(createOpenCompletionDisabledReason());
      return;
    }
    if (value === createCompletion) return;
    markCreatePresetDraftTouched();
    createCompletion = value;
  }

  function updateCreatePostSuccess(value: PostSuccessAction) {
    if (value === "trash_source" && createTrashSourceDisabledReason()) {
      showNotice(createTrashSourceDisabledReason());
      return;
    }
    if (value === createPostSuccess) return;
    markCreatePresetDraftTouched();
    createPostSuccess = value;
    if (value === "trash_source") {
      showNotice(
        tr("gui.create.output.source.trash_selected", "Originals will move to {trash} only after the new archive passes a full integrity test")
          .replace("{trash}", trashNameLabel()),
      );
    }
  }

  function effectiveCreateTestAfterCreate(): boolean {
    return createTestAfterCreate || createPostSuccess === "trash_source";
  }

  function updateCreateTestAfterCreate(value: boolean) {
    if (createPostSuccess === "trash_source") {
      showNotice(
        tr(
          "gui.create.output.integrity.required_notice",
          "A full integrity test is required before originals can move to {trash}",
        ).replace("{trash}", trashNameLabel()),
      );
      return;
    }
    if (value === createTestAfterCreate) return;
    markCreatePresetDraftTouched();
    createTestAfterCreate = value;
  }

  function chooseCreateProfile(next: CreateProfileId) {
    markCreatePresetDraftTouched();
    activeCreateProfile = next;
    persistCreateProfile(next);
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
    if (createSfxEnabled && next !== "zip") {
      showNotice(tr("gui.create.sfx_zip_only_notice", "Turn off self-extracting output before choosing another format"));
      return;
    }
    markCreatePresetDraftTouched();
    activeCreateFormat = next;
    if (nativeSplitKind(next, createSfxEnabled) === null) createSplitMode = "generic";
    if (next === "sqz") createPresetSqzInnerFormat = "entry_set";
    persistCreateFormat(next);
    const format = activeCreateFormatData();
    if (!format.can_encrypt_data) {
      clearCreatePasswordFields();
    } else if (!format.can_encrypt_names) {
      createEncryptNames = false;
    }
    createOptionsValidationAttempted = false;
    recordOperation({
      status: "info",
      title: tr("gui.create.format_selected_operation", "Create format selected"),
      detail: `${format.label} · .${format.extension}`,
    });
    showNotice(tr("gui.create.format_selected_notice", "{format} format selected").replace("{format}", format.label));
  }

  function clearCreatePasswordFields() {
    createPassword = "";
    createPasswordConfirmation = "";
    createPasswordVisible = false;
    createEncryptNames = false;
    createPresetCredentialIntent = "none";
  }

  function updateCreatePassword(value: string) {
    markCreatePresetDraftTouched();
    const hadPassword = createPassword.length > 0;
    createPassword = value;
    createOptionsValidationAttempted = false;
    if (value.length > 0) createPresetCredentialIntent = "prompt";
    if (value.length === 0) {
      createPasswordConfirmation = "";
      createEncryptNames = false;
      if (hadPassword) createPresetCredentialIntent = "none";
    }
  }

  function updateCreatePasswordConfirmation(value: string) {
    markCreatePresetDraftTouched();
    createPasswordConfirmation = value;
    createOptionsValidationAttempted = false;
  }

  function updateCreateEncryptNames(enabled: boolean) {
    markCreatePresetDraftTouched();
    createEncryptNames = enabled && createNameEncryptionAvailable() && createPassword.length > 0;
  }

  function updateCreateSplitPreset(preset: CreateSplitPreset) {
    if (createSfxEnabled && preset !== "none") {
      showNotice(tr("gui.create.sfx_no_split_notice", "Self-extracting output requires one complete ZIP payload"));
      return;
    }
    markCreatePresetDraftTouched();
    createPresetSplitSizeBytes = null;
    createSplitPreset = preset;
    if (preset === "none") createSplitMode = "generic";
    createOptionsValidationAttempted = false;
    normalizeUnsupportedCreateCompletion();
  }

  function updateCreateSplitMode(mode: CreateSplitMode) {
    if (mode === "native" && nativeSplitKind(activeCreateFormat, createSfxEnabled) === null) {
      showNotice(tr(
        "gui.create.native_layout_unavailable",
        "Native volume layout is available for ZIP and WIM; self-extracting output must remain a single ZIP.",
      ));
      return;
    }
    markCreatePresetDraftTouched();
    createSplitMode = mode;
    createOptionsValidationAttempted = false;
  }

  function updateCreateCustomSplitAmount(value: string) {
    markCreatePresetDraftTouched();
    createPresetSplitSizeBytes = null;
    createCustomSplitAmount = value;
    createOptionsValidationAttempted = false;
    normalizeUnsupportedCreateCompletion();
  }

  function updateCreateCustomSplitUnit(unit: CreateSplitUnit) {
    markCreatePresetDraftTouched();
    createPresetSplitSizeBytes = null;
    createCustomSplitUnit = unit;
    createOptionsValidationAttempted = false;
    normalizeUnsupportedCreateCompletion();
  }

  function updateCreateSfxEnabled(enabled: boolean) {
    if (enabled && !sfxCreateCapabilityReady) {
      showNotice(tr("gui.create.sfx_capability_loading", "Checking self-extracting support"));
      return;
    }
    if (enabled && !sfxCreateCapability.available) {
      showNotice(createSfxUnavailableMessage());
      return;
    }
    markCreatePresetDraftTouched();
    createSfxEnabled = enabled;
    createPresetSfxTarget = "current_platform";
    createOptionsValidationAttempted = false;
    if (enabled) {
      activeCreateFormat = "zip";
      persistCreateFormat("zip");
      createSplitPreset = "none";
      createSplitMode = "generic";
      createPresetSplitSizeBytes = null;
      createEncryptNames = false;
      normalizeUnsupportedCreateCompletion();
      showNotice(tr("gui.create.sfx_enabled_notice", "Self-extracting output enabled · ZIP payload · signing required"));
    } else {
      showNotice(tr("gui.create.sfx_disabled_notice", "Standard archive output restored"));
    }
  }

  function createSfxOutputLabel(): string {
    if (sfxCreateCapability.target === "macos") return tr("gui.create.sfx_macos_output", "macOS app (.app)");
    if (sfxCreateCapability.target === "windows") return tr("gui.create.sfx_windows_output", "Windows app (.exe)");
    return tr("gui.create.sfx_linux_output", "Linux executable (.run)");
  }

  function createSfxTargetLabel(): string {
    if (sfxCreateCapability.target === "macos") return tr("gui.create.sfx_target_macos", "This Mac");
    if (sfxCreateCapability.target === "windows") return tr("gui.create.sfx_target_windows", "This Windows PC");
    return tr("gui.create.sfx_target_linux", "This Linux system");
  }

  function createSfxSummary(): string {
    return createSfxEnabled
      ? tr("gui.create.sfx_summary_enabled", "One ZIP payload plus the Squallz extraction runtime; split volumes are off.")
      : tr("gui.create.sfx_summary_disabled", "Standard archive output; recipients need a compatible archive app.");
  }

  function createSfxSigningWarning(): string {
    if (sfxCreateCapability.target === "macos") {
      return tr("gui.create.sfx_signing_macos", "The result is unsigned. Sign and notarize it before sending it to another Mac.");
    }
    if (sfxCreateCapability.target === "windows") {
      return tr("gui.create.sfx_signing_windows", "The result is unsigned. Sign it before sharing to reduce SmartScreen warnings.");
    }
    return tr("gui.create.sfx_signing_linux", "The result is unsigned. Recipients should verify its source before running it.");
  }

  function createSfxUnavailableMessage(): string {
    if (sfxCreateCapability.status === "invalid") {
      return tr(
        "gui.create.sfx_template_invalid",
        "The self-extractor runtime is damaged or does not match this platform. Reinstall the full desktop package.",
      );
    }
    return tr("gui.create.sfx_template_unavailable", "This installation is missing its self-extractor template. Reinstall the full desktop package.");
  }

  function createSplitSizeBytes(): number | null {
    return resolveSplitSizeBytes(
      createSplitPreset,
      createCustomSplitAmount,
      createCustomSplitUnit,
      createPresetSplitSizeBytes,
    );
  }

  function createPasswordValidationMessage(): string {
    if (!createPasswordDataAvailable()) return "";
    if (createPresetCredentialIntent === "prompt" && createPassword.length === 0) {
      return tr("gui.presets.password_required", "Enter the password this preset should use for this archive");
    }
    if (createPassword.length === 0) return "";
    if (createPasswordConfirmation.length === 0) {
      return tr("gui.create.confirm_password_required", "Confirm the archive password before starting");
    }
    if (createPassword !== createPasswordConfirmation) {
      return tr("gui.create.passwords_do_not_match", "The passwords do not match");
    }
    return "";
  }

  function createSplitValidationMessage(): string {
    const splitSize = createSplitSizeBytes();
    if (createSplitPreset === "custom" && splitSize === null) {
      return tr("gui.create.invalid_part_size", "Enter a part size of at least 0.1 MiB");
    }
    if (
      activeCreateFormat === "zip"
      && createSplitMode === "native"
      && splitSize !== null
      && splitSize > fat32CompatibleSplitSizeBytes
    ) {
      return tr("gui.create.native_zip_part_size_limit", "Native ZIP parts cannot exceed 4 GiB − 1 byte");
    }
    return "";
  }

  function visibleCreatePasswordError(): string {
    const error = createPasswordValidationMessage();
    return createOptionsValidationAttempted || createPasswordConfirmation.length > 0 ? error : "";
  }

  function visibleCreateSplitError(): string {
    const error = createSplitValidationMessage();
    return createOptionsValidationAttempted || createCustomSplitAmount.length > 0 ? error : "";
  }

  function validateCreateOptions(): boolean {
    createOptionsValidationAttempted = true;
    const sfxError = createSfxEnabled && !sfxCreateCapability.available
      ? createSfxUnavailableMessage()
      : createSfxEnabled && (activeCreateFormat !== "zip" || createSplitPreset !== "none")
        ? tr("gui.create.sfx_requires_zip", "Self-extracting output requires a single ZIP payload")
        : "";
    const error = createPasswordValidationMessage() || createSplitValidationMessage() || sfxError;
    if (!error) return true;
    createAdvancedOpen = true;
    showNotice(error);
    return false;
  }

  function toggleCreateAdvancedFromKeyboard(event: KeyboardEvent) {
    if (event.key !== "Enter" && event.key !== " " && event.key !== "Spacebar") return;
    event.preventDefault();
    createAdvancedOpen = !createAdvancedOpen;
  }

  function captureCreateRunDraft(): CreateRunDraft | null {
    normalizeUnsupportedCreatePostSuccess();
    if (!validateCreateOptions()) return null;
    const format = activeCreateFormat;
    const password = createFormats[format].can_encrypt_data && createPassword.length > 0 ? createPassword : null;
    const selectedPreset = selectedCreateArchivePreset();
    const splitSize = createSplitSizeBytes();
    const splitMode = splitSize === null ? "generic" : createSplitMode;
    return {
      format,
      profile: activeCreateProfile,
      level: createCompressionLevel(),
      password,
      encryptNames: Boolean(password) && createEncryptNames && createFormats[format].can_encrypt_names,
      splitSize,
      splitMode,
      contentPolicy: createContentPolicy,
      excludes: createContentPolicy === "custom" ? [...createExcludeRules()] : [],
      sqzInnerFormat: format === "sqz" ? presetSqzInnerFormatValueForJob(createPresetSqzInnerFormat) : null,
      sfxEnabled: createSfxEnabled,
      sfxTarget: createSfxEnabled ? resolvedPresetSfxTarget(createPresetSfxTarget) : null,
      outputExtension: archiveOutputExtension(
        format,
        splitSize,
        splitMode,
        createSfxEnabled,
        sfxCreateCapability.extension,
      ),
      destination: {
        base: createDestinationBase,
        existing_output: createExistingOutputPolicy,
      },
      completion: createCompletion,
      postSuccess: createPostSuccess,
      testAfterCreate: effectiveCreateTestAfterCreate(),
      defaultCreateDir: normalizedDefaultCreateDir(appliedDefaultCreateDir),
      restoreCredentialPrompt: Boolean(selectedPreset && selectedPreset.options.credential.kind !== "none"),
      restoreEncryptNames: selectedPreset?.options.encrypt_names ?? false,
    };
  }

  function updateCustomCreateLevel(value: number, commit = false) {
    markCreatePresetDraftTouched();
    const next = clampCreateLevel(value);
    customCreateLevelError = "";
    customCreateLevel = next;
    persistCustomCreateLevel(next);
    if (activeCreateProfile !== "custom") {
      activeCreateProfile = "custom";
      persistCreateProfile("custom");
    }
    if (commit) {
      recordOperation({
        status: "info",
        title: tr("gui.create.custom_level_updated_operation", "Custom level updated"),
        detail: tr("gui.create.level_detail", "Compression level {level}").replace("{level}", String(next)),
      });
      showNotice(
        tr("gui.create.custom_level_saved_notice", "Compression level {level} saved for this device")
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

  function activeCreateProfileData() {
    return createProfileData(activeCreateProfile);
  }

  function createCompressionLevel(): number {
    return activeCreateProfileData().level;
  }

  function archivePresetById(id: string | null): NamedArchivePreset | null {
    if (!id) return null;
    return presetDocument?.presets.find((preset) => preset.id === id) ?? null;
  }

  function createArchivePresets(): Extract<NamedArchivePreset, { kind: "create" }>[] {
    return (presetDocument?.presets.filter((preset) => preset.kind === "create") ?? []) as Extract<
      NamedArchivePreset,
      { kind: "create" }
    >[];
  }

  function extractArchivePresets(): Extract<NamedArchivePreset, { kind: "extract" }>[] {
    return (presetDocument?.presets.filter((preset) => preset.kind === "extract") ?? []) as Extract<
      NamedArchivePreset,
      { kind: "extract" }
    >[];
  }

  function archivePresetDisplayName(preset: NamedArchivePreset): string {
    if (preset.id === crossPlatformCreatePresetId) {
      return tr("gui.presets.builtin_cross_platform_7z", "Cross-platform 7Z");
    }
    if (preset.id === balancedCreatePresetId) {
      return tr("gui.presets.builtin_balanced_7z", "Balanced 7Z");
    }
    if (preset.id === smartExtractPresetId) {
      return tr("gui.presets.builtin_smart_extract", "Smart extract");
    }
    return preset.label;
  }

  function archivePresetPartSummary(volumes: CreateArchivePresetOptions["volumes"]): string {
    if (volumes.kind === "single") return tr("gui.presets.single_archive", "single archive");
    const value = Number(volumes.size_bytes);
    const size = Number.isSafeInteger(value) ? formatBytes(value) : `${volumes.size_bytes} B`;
    return tr("gui.presets.split_archive", "{size} per part").replace("{size}", size);
  }

  function presetSqzInnerFormatLabel(innerFormat: PresetSqzInnerFormat): string {
    if (innerFormat === "zip") return tr("gui.presets.sqz_inner_zip", "ZIP payload");
    if (innerFormat === "seven_zip") return tr("gui.presets.sqz_inner_7z", "7Z payload");
    return tr("gui.presets.sqz_inner_entry_set", "Native entry set");
  }

  function createContentPolicyLabel(policy: CreateContentPolicy): string {
    if (policy === "cross_platform_clean") {
      return tr("gui.create.content_policy.clean", "Cross-platform clean");
    }
    if (policy === "keep_all_files") {
      return tr("gui.create.content_policy.keep_all", "Keep every file");
    }
    return tr("gui.create.content_policy.custom", "Custom rules");
  }

  function selectPresetSqzInnerFormat(innerFormat: PresetSqzInnerFormat) {
    markCreatePresetDraftTouched();
    createPresetSqzInnerFormat = innerFormat;
  }

  function createArchivePresetSummary(options: CreateArchivePresetOptions): string {
    let format = isCreateFormatId(options.format)
      ? createFormats[options.format].label
      : options.format.toUpperCase();
    if (options.format_options.kind === "sqz") {
      format = tr("gui.presets.sqz_format_summary", "{format} · {inner}")
        .replace("{format}", format)
        .replace("{inner}", presetSqzInnerFormatLabel(options.format_options.inner_format));
    }
    const protection = options.credential.kind === "none"
      ? tr("gui.presets.no_password", "no password")
      : tr("gui.presets.ask_password", "ask for password");
    const summary = tr("gui.presets.create_summary", "{format} · level {level} · {parts} · {protection} · {content}")
      .replace("{format}", format)
      .replace("{level}", String(options.level))
      .replace("{parts}", archivePresetPartSummary(options.volumes))
      .replace("{protection}", protection)
      .replace("{content}", createContentPolicyLabel(options.content_policy));
    return options.test_after_create
      ? `${summary} · ${tr("gui.presets.integrity_test", "integrity tested")}`
      : summary;
  }

  function extractArchivePresetSummary(options: ExtractArchivePresetOptions): string {
    const destination = options.destination.layout === "smart"
      ? tr("gui.presets.smart_destination", "smart folder")
      : options.destination.layout === "archive_folder"
        ? tr("gui.presets.archive_folder_destination", "archive folder")
        : options.destination.base === "ask"
          ? tr("gui.presets.choose_destination", "choose a folder")
          : tr("gui.presets.same_folder_destination", "same folder");
    return tr("gui.presets.extract_summary", "{destination} · conflicts: {conflicts} · links: {links}")
      .replace("{destination}", destination)
      .replace("{conflicts}", extractOverwriteLabel(options.existing_output))
      .replace("{links}", extractSymlinkLabel(options.symlinks));
  }

  function currentCreateArchivePresetOptions(): CreateArchivePresetOptions {
    const splitSize = createSplitSizeBytes();
    const credential: CreateArchivePresetOptions["credential"] =
      createPresetCredentialIntent === "prompt" || createPassword.length > 0
        ? { kind: "prompt" }
        : { kind: "none" };
    return {
      format: activeCreateFormat,
      level: createCompressionLevel(),
      credential,
      encrypt_names: credential.kind !== "none" && createEncryptNames,
      volumes: splitSize === null
        ? { kind: "single" }
        : { kind: "split", size_bytes: String(splitSize) },
      content_policy: createContentPolicy,
      excludes: createContentPolicy === "custom" ? createExcludeRules() : [],
      output: createSfxEnabled
        ? { kind: "self_extracting", target: createPresetSfxTarget }
        : { kind: "archive" },
      format_options: activeCreateFormat === "sqz"
        ? { kind: "sqz", inner_format: createPresetSqzInnerFormat }
        : { kind: "none" },
      destination: {
        base: createDestinationBase,
        existing_output: createExistingOutputPolicy,
      },
      completion: createCompletion,
      post_success: createPostSuccess,
      test_after_create: effectiveCreateTestAfterCreate(),
    };
  }

  function currentExtractArchivePresetOptions(): ExtractArchivePresetOptions {
    const destination: ExtractArchivePresetOptions["destination"] =
      extractDestinationMode === "smart"
        ? { base: "default_directory", layout: "smart" }
        : extractDestinationMode === "archive"
          ? { base: "default_directory", layout: "archive_folder" }
          : extractDestinationMode === "choose"
            ? { base: "ask", layout: "direct" }
            : { base: "archive_parent", layout: "direct" };
    const encoding = extractPresetEncodingLabel ?? archiveEncodingForJob();
    return {
      destination,
      existing_output: extractOverwriteMode,
      symlinks: extractSymlinkMode,
      encoding: encoding ? { kind: "named", label: encoding } : { kind: "auto" },
      credential: { kind: "prompt_when_needed" },
      post_success: "keep_source",
    };
  }

  function createPresetPickerOptions() {
    return createArchivePresets().map((preset) => ({
      id: preset.id,
      name: archivePresetDisplayName(preset),
      summary: createArchivePresetSummary(preset.options),
    }));
  }

  function extractPresetPickerOptions() {
    return extractArchivePresets().map((preset) => ({
      id: preset.id,
      name: archivePresetDisplayName(preset),
      summary: extractArchivePresetSummary(preset.options),
    }));
  }

  function normalizePresetName(value: string): string {
    return Array.from(value.replace(/\s+/gu, " ").trim()).slice(0, 40).join("");
  }

  async function migrateLegacyCreatePresets(document: ArchivePresetDocument): Promise<ArchivePresetDocument> {
    const storage = previewStorage();
    const raw = storage?.getItem("squallz.customCreateProfiles.v1");
    if (!storage || !raw) return document;
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return document;
    }
    if (!Array.isArray(parsed)) return document;
    const next = structuredClone(document);
    let imported = 0;
    for (const [index, item] of parsed.entries()) {
      if (!item || typeof item !== "object" || next.presets.length >= maxArchivePresets) continue;
      const legacy = item as { id?: unknown; name?: unknown; level?: unknown };
      const name = normalizePresetName(typeof legacy.name === "string" ? legacy.name : "");
      const level = Number(legacy.level);
      if (!name || !Number.isInteger(level) || level < 1 || level > 9) continue;
      const rawId = typeof legacy.id === "string" ? legacy.id : String(index + 1);
      const suffix = rawId.toLowerCase().replace(/[^a-z0-9._-]+/gu, "-").replace(/^-+|-+$/gu, "").slice(0, 36)
        || String(index + 1);
      const id = `user.create.legacy-${suffix}`;
      if (next.presets.some((preset) => preset.id === id)) continue;
      let label = name;
      for (let copy = 2; next.presets.some((preset) => preset.kind === "create" && preset.label.toLocaleLowerCase() === label.toLocaleLowerCase()); copy += 1) {
        const marker = ` ${copy}`;
        label = `${Array.from(name).slice(0, 40 - marker.length).join("")}${marker}`;
      }
      next.presets.push({
        kind: "create",
        id,
        label,
        built_in: false,
        options: {
          format: "7z",
          level,
          credential: { kind: "none" },
          encrypt_names: false,
          volumes: { kind: "single" },
          content_policy: "custom",
          excludes: [],
          output: { kind: "archive" },
          format_options: { kind: "none" },
          destination: { base: "ask", existing_output: "ask" },
          completion: "none",
          post_success: "keep_source",
          test_after_create: false,
        },
      });
      imported += 1;
    }
    if (imported === 0) return document;
    const saved = await ipc.saveArchivePresets(document.revision, next);
    storage.removeItem("squallz.customCreateProfiles.v1");
    storage.removeItem("squallz.activeCustomCreateProfile");
    showNotice(
      tr("gui.presets.legacy_imported", "Imported {count} older compression presets")
        .replace("{count}", String(imported)),
    );
    return saved;
  }

  function presetNameExists(kind: "create" | "extract", name: string, exceptId: string | null): boolean {
    const normalized = name.toLocaleLowerCase();
    return (presetDocument?.presets ?? []).some(
      (preset) =>
        preset.kind === kind &&
        preset.id !== exceptId &&
        preset.label.toLocaleLowerCase() === normalized,
    );
  }

  function uniquePresetName(kind: "create" | "extract", requested: string): string {
    if (!presetNameExists(kind, requested, null)) return requested;
    for (let index = 2; index <= 99; index += 1) {
      const suffix = ` ${index}`;
      const base = Array.from(requested).slice(0, 40 - suffix.length).join("");
      const candidate = `${base}${suffix}`;
      if (!presetNameExists(kind, candidate, null)) return candidate;
    }
    return `${Array.from(requested).slice(0, 32).join("")} ${Date.now().toString(36)}`;
  }

  function createPresetSaveValidationMessage(): string {
    if (createPostSuccess === "trash_source" && createTrashSourceDisabledReason()) {
      return createTrashSourceDisabledReason();
    }
    const splitError = createSplitValidationMessage();
    if (splitError) return splitError;
    const rules = createContentPolicy === "custom" ? createExcludeRules() : [];
    if (rules.length > maxArchivePresetExcludeRules) {
      return tr("gui.presets.exclude_limit", "Presets can store up to {count} exclude rules")
        .replace("{count}", String(maxArchivePresetExcludeRules));
    }
    const oversized = rules.find((rule) => new TextEncoder().encode(rule).length > maxArchivePresetExcludeRuleBytes);
    if (oversized) {
      return tr("gui.presets.exclude_rule_too_long", "Shorten the exclude rule beginning with {rule}; each rule can use up to {count} bytes")
        .replace("{rule}", oversized.slice(0, 24))
        .replace("{count}", String(maxArchivePresetExcludeRuleBytes));
    }
    return "";
  }

  function selectedCreateArchivePreset() {
    const preset = archivePresetById(selectedCreatePresetId);
    return preset?.kind === "create" ? preset : null;
  }

  function selectedExtractArchivePreset() {
    const preset = archivePresetById(selectedExtractPresetId);
    return preset?.kind === "extract" ? preset : null;
  }

  function createPresetModified(): boolean {
    const selected = selectedCreateArchivePreset();
    if (!selected) return false;
    return (
      JSON.stringify(currentCreateArchivePresetOptions()) !== JSON.stringify(selected.options) ||
      normalizePresetName(createPresetDraftName) !== selected.label
    );
  }

  function extractPresetModified(): boolean {
    const selected = selectedExtractArchivePreset();
    if (!selected) return false;
    return (
      JSON.stringify(currentExtractArchivePresetOptions()) !== JSON.stringify(selected.options) ||
      normalizePresetName(extractPresetDraftName) !== selected.label
    );
  }

  function createPresetStatus(): "idle" | "applied" | "modified" | "saving" | "error" {
    if (presetLoadState === "error" || createPresetMutationState === "error") return "error";
    if (presetLoadState === "loading" || createPresetMutationState === "saving") return "saving";
    if (!selectedCreatePresetId) return "idle";
    return createPresetModified() ? "modified" : "applied";
  }

  function extractPresetStatus(): "idle" | "applied" | "modified" | "saving" | "error" {
    if (presetLoadState === "error" || extractPresetMutationState === "error") return "error";
    if (presetLoadState === "loading" || extractPresetMutationState === "saving") return "saving";
    if (!selectedExtractPresetId) return "idle";
    return extractPresetModified() ? "modified" : "applied";
  }

  function archivePresetStatusLabel(
    status: "idle" | "applied" | "modified" | "saving" | "error",
  ): string {
    if (presetLoadState === "loading") return tr("gui.presets.loading", "Loading presets");
    if (presetLoadState === "error") return tr("gui.presets.unavailable", "Presets unavailable");
    if (status === "saving") return tr("gui.presets.saving", "Saving");
    if (status === "error") return tr("gui.presets.failed", "Could not save");
    if (status === "modified") return tr("gui.presets.modified", "Modified");
    if (status === "applied") return tr("gui.presets.applied", "Applied");
    return tr("gui.presets.current", "Not saved");
  }

  function archivePresetPickerDisabledReason(kind: "create" | "extract"): string {
    if (presetLoadState === "loading") return tr("gui.presets.loading", "Loading presets");
    if (presetLoadState === "error") {
      return tr("gui.presets.load_failed", "Could not load presets. The preset file was not changed.");
    }
    if (kind === "create" && createPreflightBusy()) {
      return tr("gui.presets.busy", "Wait for the current preflight to finish");
    }
    return "";
  }

  function applyPresetVolumeMode(volumes: CreateArchivePresetOptions["volumes"]) {
    createSplitMode = "generic";
    if (volumes.kind === "single") {
      createSplitPreset = "none";
      createPresetSplitSizeBytes = null;
      return;
    }
    const bytes = Number(volumes.size_bytes);
    createPresetSplitSizeBytes = volumes.size_bytes;
    if (bytes === 25 * bytesPerMiB) createSplitPreset = "25-mib";
    else if (bytes === 100 * bytesPerMiB) createSplitPreset = "100-mib";
    else if (bytes === 700 * bytesPerMiB) createSplitPreset = "700-mib";
    else if (bytes === fat32CompatibleSplitSizeBytes) createSplitPreset = "4-gib";
    else {
      createSplitPreset = "custom";
      createCustomSplitUnit = bytes >= bytesPerGiB ? "gib" : "mib";
      const divisor = createCustomSplitUnit === "gib" ? bytesPerGiB : bytesPerMiB;
      createCustomSplitAmount = String(Number((bytes / divisor).toPrecision(9)));
    }
  }

  function applyDefaultCreatePresetWhenReady() {
    if (!sfxCreateCapabilityReady || createPresetDraftTouched || !presetDocument) return;
    const presetId = presetDocument.bindings.app_default_create;
    if (presetId) applyCreatePreset(presetId, false);
  }

  function resolvedPresetSfxTarget(target: PresetSfxTarget): PlatformKind {
    return target === "current_platform" ? sfxCreateCapability.target : target;
  }

  function presetSqzInnerFormatValueForJob(innerFormat: PresetSqzInnerFormat): "sqz" | "zip" | "7z" {
    if (innerFormat === "zip") return "zip";
    if (innerFormat === "seven_zip") return "7z";
    return "sqz";
  }

  function applyCreatePreset(id: string | null, announce = true) {
    if (createPreflightBusy()) {
      if (announce) showNotice(tr("gui.presets.busy", "Wait for the current preflight to finish"));
      return;
    }
    if (announce) markCreatePresetDraftTouched();
    if (!id) {
      selectedCreatePresetId = null;
      createPresetDraftName = "";
      createPresetMutationState = "idle";
      return;
    }
    const preset = archivePresetById(id);
    if (!preset || preset.kind !== "create") return;
    if (!isCreateFormatId(preset.options.format)) {
      showNotice(tr("gui.presets.format_unavailable", "This preset uses a format that is not available in the create screen"));
      return;
    }
    if (preset.options.output.kind === "self_extracting") {
      if (!sfxCreateCapabilityReady) {
        showNotice(tr("gui.create.sfx_capability_loading", "Checking self-extracting support"));
        return;
      }
      if (!sfxCreateCapability.available) {
        showNotice(createSfxUnavailableMessage());
        return;
      }
      const requestedTarget = resolvedPresetSfxTarget(preset.options.output.target);
      if (requestedTarget !== sfxCreateCapability.target) {
        showNotice(
          tr("gui.presets.sfx_target_unavailable", "This device cannot build the preset's {target} self-extractor")
            .replace("{target}", requestedTarget),
        );
        return;
      }
    }
    invalidateCreatePreflightResult();
    selectedCreatePresetId = preset.id;
    createPresetDraftName = preset.label;
    activeCreateFormat = preset.options.format;
    activeCreateProfile = "custom";
    customCreateLevel = preset.options.level;
    customCreateLevelError = "";
    createPresetCredentialIntent = preset.options.credential.kind === "none" ? "none" : "prompt";
    createPassword = "";
    createPasswordConfirmation = "";
    createPasswordVisible = false;
    createEncryptNames = preset.options.encrypt_names;
    applyPresetVolumeMode(preset.options.volumes);
    createContentPolicy = preset.options.content_policy;
    createExcludeText = preset.options.excludes.join("\n");
    createSfxEnabled = preset.options.output.kind === "self_extracting";
    createPresetSfxTarget = preset.options.output.kind === "self_extracting"
      ? preset.options.output.target
      : "current_platform";
    createPresetSqzInnerFormat = preset.options.format_options.kind === "sqz"
      ? preset.options.format_options.inner_format
      : "entry_set";
    createDestinationBase = preset.options.destination.base;
    createExistingOutputPolicy = preset.options.destination.base === "ask" ? "ask" : "rename";
    createCompletion = preset.options.completion;
    createPostSuccess = preset.options.post_success;
    createTestAfterCreate = preset.options.test_after_create;
    const postSuccessNormalized = normalizeUnsupportedCreatePostSuccess(false);
    normalizeUnsupportedCreateCompletion(announce);
    createOptionsValidationAttempted = false;
    createPresetMutationState = "idle";
    if (announce) {
      showNotice(
        postSuccessNormalized
          ? tr(
            "gui.create.output.source.preset_changed_to_keep",
            "{name} applied · originals will stay in place because this preset excludes some content",
          ).replace("{name}", archivePresetDisplayName(preset))
          : tr("gui.presets.applied_notice", "{name} applied")
            .replace("{name}", archivePresetDisplayName(preset)),
      );
    }
  }

  function applyExtractPreset(id: string | null, announce = true) {
    if (!id) {
      selectedExtractPresetId = null;
      extractPresetDraftName = "";
      extractPresetMutationState = "idle";
      return;
    }
    const preset = archivePresetById(id);
    if (!preset || preset.kind !== "extract") return;
    selectedExtractPresetId = preset.id;
    extractPresetDraftName = preset.label;
    if (preset.options.destination.layout === "smart") extractDestinationMode = "smart";
    else if (preset.options.destination.layout === "archive_folder") extractDestinationMode = "archive";
    else if (preset.options.destination.base === "ask") extractDestinationMode = "choose";
    else extractDestinationMode = "same";
    if (extractDestinationMode === "choose") extractCustomDest = "";
    extractOverwriteMode = preset.options.existing_output;
    extractSymlinkMode = preset.options.symlinks;
    extractPresetEncodingLabel = preset.options.encoding.kind === "named"
      ? preset.options.encoding.label
      : null;
    extractPresetMutationState = "idle";
    if (announce) {
      showNotice(
        tr("gui.presets.applied_notice", "{name} applied")
          .replace("{name}", archivePresetDisplayName(preset)),
      );
    }
  }

  function clonePresetDocument(): ArchivePresetDocument | null {
    return presetDocument ? structuredClone(presetDocument) : null;
  }

  function reconcilePresetSelections(document: ArchivePresetDocument) {
    if (
      selectedCreatePresetId &&
      !document.presets.some((preset) => preset.kind === "create" && preset.id === selectedCreatePresetId)
    ) {
      selectedCreatePresetId = null;
    }
    if (
      selectedExtractPresetId &&
      !document.presets.some((preset) => preset.kind === "extract" && preset.id === selectedExtractPresetId)
    ) {
      selectedExtractPresetId = null;
    }
  }

  function setPresetMutationState(kind: "create" | "extract", state: ArchivePresetMutationState) {
    if (kind === "create") createPresetMutationState = state;
    else extractPresetMutationState = state;
  }

  async function persistPresetDocument(
    kind: "create" | "extract",
    next: ArchivePresetDocument,
    successKey: string,
    successFallback: string,
    detail: string,
  ): Promise<boolean> {
    if (
      (kind === "create" && createPresetMutationState === "saving") ||
      (kind === "extract" && extractPresetMutationState === "saving")
    ) {
      return false;
    }
    setPresetMutationState(kind, "saving");
    try {
      const saved = await ipc.saveArchivePresets(next.revision, next);
      presetDocument = saved;
      presetLoadState = "ready";
      setPresetMutationState(kind, "idle");
      showNotice(tr(successKey, successFallback));
      recordOperation({ status: "done", title: tr(successKey, successFallback), detail });
      return true;
    } catch (error) {
      setPresetMutationState(kind, "error");
      if (isErrorDto(error) && error.key === "error.presets_conflict") {
        void ipc.getArchivePresets().then((latest) => {
          presetDocument = latest;
          presetLoadState = "ready";
          reconcilePresetSelections(latest);
        }).catch(() => undefined);
      }
      showNotice(
        isErrorDto(error)
          ? tr(error.key, tr("gui.presets.failed", "Could not save"))
          : tr("gui.presets.failed", "Could not save"),
      );
      return false;
    }
  }

  function newArchivePresetId(kind: "create" | "extract"): string {
    const random = globalThis.crypto?.randomUUID?.().replaceAll("-", "")
      ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
    return `user.${kind}.${random}`.slice(0, 64);
  }

  async function saveCurrentCreatePresetAsNew() {
    const next = clonePresetDocument();
    if (!next) {
      showNotice(tr("gui.presets.load_failed", "Could not load presets. The preset file was not changed."));
      return;
    }
    const validationError = createPresetSaveValidationMessage();
    if (validationError) {
      showNotice(validationError);
      return;
    }
    const requested = normalizePresetName(createPresetDraftName);
    if (!requested) {
      showNotice(tr("gui.presets.name_required", "Enter a preset name"));
      return;
    }
    if (next.presets.length >= maxArchivePresets) {
      showNotice(tr("gui.presets.limit", "Delete a preset before saving another one"));
      return;
    }
    const name = uniquePresetName("create", requested);
    const id = newArchivePresetId("create");
    next.presets.push({
      kind: "create",
      id,
      label: name,
      built_in: false,
      options: currentCreateArchivePresetOptions(),
    });
    if (await persistPresetDocument("create", next, "gui.presets.operation_saved", "Preset saved", name)) {
      selectedCreatePresetId = id;
      createPresetDraftName = name;
    }
  }

  async function saveCurrentExtractPresetAsNew() {
    const next = clonePresetDocument();
    if (!next) {
      showNotice(tr("gui.presets.load_failed", "Could not load presets. The preset file was not changed."));
      return;
    }
    const requested = normalizePresetName(extractPresetDraftName);
    if (!requested) {
      showNotice(tr("gui.presets.name_required", "Enter a preset name"));
      return;
    }
    if (next.presets.length >= maxArchivePresets) {
      showNotice(tr("gui.presets.limit", "Delete a preset before saving another one"));
      return;
    }
    const name = uniquePresetName("extract", requested);
    const id = newArchivePresetId("extract");
    next.presets.push({
      kind: "extract",
      id,
      label: name,
      built_in: false,
      options: currentExtractArchivePresetOptions(),
    });
    if (await persistPresetDocument("extract", next, "gui.presets.operation_saved", "Preset saved", name)) {
      selectedExtractPresetId = id;
      extractPresetDraftName = name;
    }
  }

  function createPresetFinderCompatible(options: CreateArchivePresetOptions): boolean {
    return options.format === "7z" &&
      options.credential.kind === "none" &&
      options.output.kind === "archive" &&
      options.completion !== "open_in_squallz" &&
      options.post_success === "keep_source";
  }

  function createPresetFinderDisabledReason(): string {
    const selected = selectedCreateArchivePreset();
    if (!selected || createPresetFinderCompatible(selected.options)) return "";
    return tr("gui.presets.finder_create_incompatible", "File-manager compression needs a standard 7Z preset that keeps sources, does not prompt for a password, and does not open the result in Squallz");
  }

  function createPresetUpdateDisabledReason(): string {
    const selected = selectedCreateArchivePreset();
    if (!selected) return "";
    if (selected.built_in) return tr("gui.presets.built_in_read_only", "Built-in presets cannot be changed");
    if (createPreflightBusy()) return tr("gui.presets.busy", "Wait for the current preflight to finish");
    if (
      presetDocument?.bindings.file_manager_create === selected.id &&
      !createPresetFinderCompatible(currentCreateArchivePresetOptions())
    ) {
      return tr("gui.presets.unbind_finder_before_update", "Turn off file-manager use before saving incompatible changes");
    }
    return "";
  }

  function presetDeleteDisabledReason(preset: NamedArchivePreset | null): string {
    return preset?.built_in
      ? tr("gui.presets.built_in_read_only", "Built-in presets cannot be changed")
      : "";
  }

  async function updateSelectedArchivePreset(kind: "create" | "extract") {
    const selected = kind === "create" ? selectedCreateArchivePreset() : selectedExtractArchivePreset();
    const next = clonePresetDocument();
    if (!selected || !next || selected.built_in) return;
    if (kind === "create" && createPresetUpdateDisabledReason()) {
      showNotice(createPresetUpdateDisabledReason());
      return;
    }
    if (kind === "create") {
      const validationError = createPresetSaveValidationMessage();
      if (validationError) {
        showNotice(validationError);
        return;
      }
    }
    const draftName = kind === "create" ? createPresetDraftName : extractPresetDraftName;
    const name = normalizePresetName(draftName);
    if (!name) {
      showNotice(tr("gui.presets.name_required", "Enter a preset name"));
      return;
    }
    if (presetNameExists(kind, name, selected.id)) {
      showNotice(tr("gui.presets.name_in_use", "That preset name is already in use"));
      return;
    }
    const index = next.presets.findIndex((preset) => preset.id === selected.id);
    if (index < 0) return;
    next.presets[index] = kind === "create"
      ? {
          kind: "create",
          id: selected.id,
          label: name,
          built_in: false,
          options: currentCreateArchivePresetOptions(),
        }
      : {
          kind: "extract",
          id: selected.id,
          label: name,
          built_in: false,
          options: currentExtractArchivePresetOptions(),
        };
    if (await persistPresetDocument(kind, next, "gui.presets.operation_updated", "Preset updated", name)) {
      if (kind === "create") createPresetDraftName = name;
      else extractPresetDraftName = name;
    }
  }

  async function deleteSelectedArchivePreset(kind: "create" | "extract") {
    const selected = kind === "create" ? selectedCreateArchivePreset() : selectedExtractArchivePreset();
    const next = clonePresetDocument();
    if (!selected || !next || selected.built_in) return;
    next.presets = next.presets.filter((preset) => preset.id !== selected.id);
    if (kind === "create") {
      if (next.bindings.app_default_create === selected.id) next.bindings.app_default_create = balancedCreatePresetId;
      if (next.bindings.file_manager_create === selected.id) next.bindings.file_manager_create = balancedCreatePresetId;
    } else {
      if (next.bindings.app_default_extract === selected.id) next.bindings.app_default_extract = smartExtractPresetId;
      if (next.bindings.file_manager_extract === selected.id) next.bindings.file_manager_extract = smartExtractPresetId;
    }
    if (await persistPresetDocument(kind, next, "gui.presets.operation_deleted", "Preset deleted", selected.label)) {
      if (kind === "create") {
        if (createPreflightBusy()) {
          selectedCreatePresetId = null;
          createPresetDraftName = "";
        } else {
          applyCreatePreset(balancedCreatePresetId, false);
        }
      } else {
        applyExtractPreset(smartExtractPresetId, false);
      }
    }
  }

  async function setArchivePresetBinding(
    kind: "create" | "extract",
    target: "app" | "file_manager",
    enabled: boolean,
  ) {
    const selected = kind === "create" ? selectedCreateArchivePreset() : selectedExtractArchivePreset();
    const next = clonePresetDocument();
    if (!selected || !next) return;
    if (
      kind === "create" &&
      selected.kind === "create" &&
      target === "file_manager" &&
      enabled &&
      !createPresetFinderCompatible(selected.options)
    ) {
      showNotice(createPresetFinderDisabledReason());
      return;
    }
    const value = enabled ? selected.id : null;
    if (kind === "create" && target === "app") next.bindings.app_default_create = value;
    else if (kind === "create") next.bindings.file_manager_create = value;
    else if (target === "app") next.bindings.app_default_extract = value;
    else next.bindings.file_manager_extract = value;
    await persistPresetDocument(
      kind,
      next,
      "gui.presets.operation_binding",
      "Preset default changed",
      archivePresetDisplayName(selected),
    );
  }

  function nativeSplitKind(
    formatId: CreateFormatId,
    sfxEnabled = false,
  ): NativeSplitKind {
    if (sfxEnabled) return null;
    return formatId === "zip" || formatId === "wim" ? formatId : null;
  }

  function archiveOutputExtension(
    formatId: CreateFormatId,
    splitSize: number | null,
    splitMode: CreateSplitMode,
    sfxEnabled = false,
    sfxExtension = "",
  ): string {
    if (sfxEnabled) return sfxExtension;
    if (formatId === "wim" && splitSize !== null && splitMode === "native") return "swm";
    return createFormats[formatId].extension;
  }

  function createArchiveNameForOutput(base: string, outputExtension: string): string {
    return `${base}.${outputExtension}`;
  }

  function createArchivePreviewName(base = "archive"): string {
    return createArchiveNameForOutput(
      base,
      archiveOutputExtension(
        activeCreateFormat,
        createSplitSizeBytes(),
        createSplitMode,
        createSfxEnabled,
        sfxCreateCapability.extension,
      ),
    );
  }

  function joinFolderPath(folder: string, name: string): string {
    return joinDesktopPath(folder, name, platformKind());
  }

  function commonCreateSourceParent(inputs: readonly string[]): string | null {
    const first = inputs[0];
    if (!first) return null;
    const platform = platformKind();
    const parent = desktopDirname(first, platform);
    return inputs.every((input) => sameDesktopPath(desktopDirname(input, platform), parent, platform))
      ? parent
      : null;
  }

  function createOutputPreviewBase(): string {
    return createSourceInputs.length === 1
      ? archiveBaseOrDefault(archiveStemName(desktopBasename(createSourceInputs[0], platformKind())))
      : "archive";
  }

  function createOutputPreview(base = createOutputPreviewBase()): string {
    const name = createArchivePreviewName(base);
    if (createDestinationBase === "ask") {
      return tr("gui.create.output.preview_ask", "Choose location when starting · {name}").replace("{name}", name);
    }
    if (createDestinationBase === "default_directory") {
      const folder = normalizedDefaultCreateDir(appliedDefaultCreateDir);
      return folder
        ? joinFolderPath(folder, name)
        : tr("gui.create.output.preview_default_missing", "Default create folder is not set · Squallz will ask");
    }
    const droppedParent = commonCreateSourceParent(createSourceInputs);
    return droppedParent
      ? joinFolderPath(droppedParent, name)
      : tr("gui.create.output.preview_source_parent", "Next to the selected sources · {name}").replace("{name}", name);
  }

  function createSaveDefaultPathForDraft(input: string, base: string, draft: CreateRunDraft): string {
    return joinFolderPath(
      desktopDirname(input, platformKind()),
      createArchiveNameForOutput(base, draft.outputExtension),
    );
  }

  async function inspectCreateDestinationForCreate(
    path: string,
    split: boolean,
    sfxTarget: string | null,
  ): Promise<CreateDestinationInspectionDto> {
    await ensureCreatePreflightListener();
    const requestId = nextPreflightRequestId();
    createPreflightRequestId = requestId;
    createPreflightRequestKind = "destination";
    createPreflightProcessedBytes = 0;
    createPreflightCancelPending = false;
    createPreflightCurrent = "";
    try {
      const inspection = await ipc.inspectCreateDestination(path, split, requestId, sfxTarget);
      if (createPreflightRequestId === requestId && createPreflightCancelPending) {
        throw new CreateDestinationInspectionError(undefined, true);
      }
      return inspection;
    } catch (error) {
      const cancelled = createPreflightRequestId === requestId && createPreflightCancelPending;
      if (error instanceof CreateDestinationInspectionError) {
        if (!cancelled || error.cancelled) throw error;
        throw new CreateDestinationInspectionError(error.detail ?? undefined, true);
      }
      throw new CreateDestinationInspectionError(error, cancelled);
    } finally {
      if (createPreflightRequestId === requestId) {
        createPreflightRequestId = null;
        createPreflightRequestKind = null;
        createPreflightCancelPending = false;
        createPreflightCurrent = "";
      }
    }
  }

  function createDestinationInspectionCancelled(error: unknown): boolean {
    return error instanceof CreateDestinationInspectionError
      && (error.cancelled || error.detail?.key === "error.cancelled");
  }

  async function cancelCreateDestinationInspection(
    options: { announce?: boolean; keepIntentOnFailure?: boolean } = {},
  ) {
    const announce = options.announce ?? true;
    const keepIntentOnFailure = options.keepIntentOnFailure ?? false;
    const requestId = createPreflightRequestId;
    if (
      !requestId
      || createPreflightRequestKind !== "destination"
      || createPreflightCancelPending
    ) return;
    createPreflightCancelPending = true;
    if (import.meta.env.DEV && requestId === previewDestinationRequestId) {
      await new Promise((resolve) => window.setTimeout(resolve, 180));
      finishCreatePreflightWithIssue(
        "destination",
        tr(
          "gui.create.destination_check_cancelled",
          "Output check cancelled · no archive was created",
        ),
        "cancelled",
      );
      focusCreatePrimaryAction();
      return;
    }
    try {
      await ipc.cancelCreateDestinationInspection(requestId);
      if (
        announce
        && createPreflightRequestId === requestId
        && createPreflightCancelPending
      ) {
        showNotice(tr(
          "gui.create.destination_check_cancel_requested",
          "Stopping the output check...",
        ));
      }
    } catch {
      if (createPreflightRequestId !== requestId) return;
      if (!keepIntentOnFailure) createPreflightCancelPending = false;
      if (announce) {
        showNotice(tr(
          "gui.create.destination_check_cancel_failed",
          "Could not stop the output check. It will continue.",
        ));
      }
    }
  }

  async function askCreateDestination(
    inputs: readonly string[],
    base: string,
    draft: CreateRunDraft,
    source: "dialog" | "drop",
  ): Promise<ResolvedCreateDestination | null> {
    const { confirm, save } = await getDialogModule();
    const selected = await saveNativeDialog("create.save-archive", save, {
      title: source === "drop"
        ? tr("gui.create.save_dropped_items_as_archive", "Save dropped items as archive")
        : tr("gui.create.save_archive_as", "Save archive as"),
      defaultPath: createSaveDefaultPathForDraft(inputs[0], base, draft),
      filters: createSaveFiltersForDraft(draft),
    });
    if (!selected) return null;
    const path = normalizeCreateDestinationForDraft(selected, draft);
    const inspection = await inspectCreateDestinationForCreate(
      path,
      draft.splitSize !== null,
      draft.sfxTarget,
    );
    if (inspection.conflict && inspection.guard === null) {
      throw new CreateDestinationInspectionError();
    }
    if (inspection.conflict) {
      const replaceExisting = await confirm(
        tr(
          "gui.create.replace_existing.body",
          "An output file or split volume set already exists for {path}. Replace the existing output set with the new archive?",
        ).replace("{path}", path),
        {
          title: tr("gui.create.replace_existing.title", "Replace existing output?"),
          kind: "warning",
          okLabel: tr("gui.create.replace_existing.action", "Replace"),
          cancelLabel: tr("gui.create.replace_existing.cancel", "Cancel"),
        },
      );
      if (!replaceExisting) return null;
    }
    return {
      path,
      replaceExisting: inspection.conflict,
      replacementGuard: inspection.guard,
      confirmLateConflict: true,
    };
  }

  async function authorizeArchiveOutput(
    path: string,
    confirm: DialogModule["confirm"],
    split = false,
    inspect: (
      candidate: string,
      splitOutput: boolean,
    ) => Promise<CreateDestinationInspectionDto> = (candidate, splitOutput) =>
      inspectCreateDestinationForCreate(candidate, splitOutput, null),
  ): Promise<AuthorizedArchiveOutput | null> {
    const inspection = await inspect(path, split);
    if (inspection.conflict !== (inspection.guard !== null)) {
      throw new CreateDestinationInspectionError();
    }
    if (inspection.conflict) {
      const replaceExisting = await confirm(
        (split
          ? tr(
              "gui.output.replace_existing_split.body",
              "An archive or numbered volume set already exists for {path}. Replace only this exact output set? If another app changes it before the task finishes, Squallz will keep it.",
            )
          : tr(
              "gui.output.replace_existing.body",
              "An archive already exists at {path}. Replace only this exact version? If another app changes it before the task finishes, Squallz will keep it.",
            )).replace("{path}", path),
        {
          title: split
            ? tr("gui.output.replace_existing_split.title", "Replace existing output set?")
            : tr("gui.output.replace_existing.title", "Replace existing archive?"),
          kind: "warning",
          okLabel: tr("gui.output.replace_existing.action", "Replace"),
          cancelLabel: tr("gui.output.replace_existing.cancel", "Cancel"),
        },
      );
      if (!replaceExisting) return null;
    }
    return {
      replaceExisting: inspection.conflict,
      replacementGuard: inspection.guard,
    };
  }

  async function resolveCreateDestination(
    inputs: readonly string[],
    base: string,
    draft: CreateRunDraft,
    source: "dialog" | "drop",
  ): Promise<ResolvedCreateDestination | null> {
    if (draft.destination.base === "ask") {
      return askCreateDestination(inputs, base, draft, source);
    }

    let folder: string | null;
    if (draft.destination.base === "source_parent") {
      folder = commonCreateSourceParent(inputs);
      if (!folder) {
        showNotice(
          tr(
            "gui.create.output.source_parent_fallback",
            "The selected sources are in different folders. Choose where to save this archive.",
          ),
        );
        return askCreateDestination(inputs, base, draft, source);
      }
    } else {
      folder = draft.defaultCreateDir;
      if (!folder) {
        showNotice(
          tr(
            "gui.create.output.default_folder_fallback",
            "The default create folder is not set. Choose where to save this archive.",
          ),
        );
        return askCreateDestination(inputs, base, draft, source);
      }
    }

    const proposed = joinFolderPath(
      folder,
      createArchiveNameForOutput(base, draft.outputExtension),
    );
    const status = tr(
      "gui.create.finding_available_destination",
      "Finding an available output name...",
    );
    createPreflightCurrent = status;
    try {
      return {
        path: await ipc.uniqueCreateDestination(proposed, draft.splitSize !== null),
        replaceExisting: false,
        replacementGuard: null,
        confirmLateConflict: false,
      };
    } catch {
      showNotice(
        draft.destination.base === "default_directory"
          ? tr("gui.create.output.default_folder_unavailable", "The default create folder is unavailable. Choose another location.")
          : tr("gui.create.output.source_folder_unavailable", "The source folder cannot be used for output. Choose another location."),
      );
      return askCreateDestination(inputs, base, draft, source);
    } finally {
      if (createPreflightCurrent === status) createPreflightCurrent = "";
    }
  }

  function normalizeCreateDestinationForDraft(path: string, draft: CreateRunDraft): string {
    const extensions = draft.outputExtension === "swm" || draft.sfxEnabled
      ? [draft.outputExtension]
      : createFormats[draft.format].extensions;
    const lowerPath = path.toLowerCase();
    if (extensions.some((extension) => lowerPath.endsWith(`.${extension.toLowerCase()}`))) return path;
    return `${path}.${draft.outputExtension}`;
  }

  function createSaveFiltersForDraft(draft: CreateRunDraft) {
    if (draft.sfxEnabled || draft.outputExtension === "swm") {
      return [{
        name: draft.sfxEnabled
          ? tr("gui.create.sfx_filter", "Self-extracting output")
          : tr("gui.create.split_wim_filter", "Split WIM first part"),
        extensions: [draft.outputExtension],
      }];
    }
    const activeId = draft.format;
    return [{
      name: createFormatFilterName(activeId),
      extensions: createFormats[activeId].extensions,
    }];
  }

  function createFormatFilterName(formatId: CreateFormatId): string {
    return tr(`gui.create.format.${formatId}.filter`, createFormats[formatId].filterName);
  }

  function createFormatDisabledReason(formatId: CreateFormatId): string {
    const lockedReason = createOptionsLockedReason();
    if (lockedReason) return lockedReason;
    return createSfxEnabled && formatId !== "zip"
      ? tr("gui.create.sfx_zip_only", "Self-extracting output uses a ZIP payload")
      : "";
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
    return createSfxEnabled
      ? tr("gui.create.sfx_no_split_capability", "Self-extracting output uses one complete ZIP payload")
      : createFormatSplit();
  }

  function createRecoveryCapability(): string {
    return createFormatRecovery();
  }

  function createFormatNote(): string {
    return createFormatNoteFor();
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

  function markSettingsDraft(section: PersistedSettingsSection) {
    settingsDraftGenerations[section] += 1;
    settingsSaveOutcomes[section] = "idle";
  }

  function setWorkspaceAccentContrastGuard(enabled: boolean) {
    accentContrastGuard = enabled;
    markSettingsDraft("colors");
  }

  function setWorkspaceGeneralLanguageChoice(value: string) {
    generalLanguageChoice = value;
    markSettingsDraft("general");
  }

  function setWorkspaceGeneralDefaultCreateDir(value: string) {
    generalDefaultCreateDir = value;
    markSettingsDraft("general");
  }

  function setWorkspaceGeneralDefaultExtractDir(value: string) {
    generalDefaultExtractDir = value;
    markSettingsDraft("general");
  }

  function setWorkspaceGeneralRevealAfterExtract(enabled: boolean) {
    generalRevealAfterExtract = enabled;
    markSettingsDraft("general");
  }

  function setWorkspaceGeneralAutomaticUpdateChecks(enabled: boolean) {
    generalAutomaticUpdateChecks = enabled;
    markSettingsDraft("general");
  }

  function setWorkspaceSafetyMaxEntries(value: NumericSetting) {
    safetyMaxEntries = value;
    markSettingsDraft("security");
  }

  function setWorkspaceSafetyMaxOutputGiB(value: NumericSetting) {
    safetyMaxOutputGiB = value;
    markSettingsDraft("security");
  }

  function setWorkspaceSafetyMaxCompressionRatio(value: NumericSetting) {
    safetyMaxCompressionRatio = value;
    markSettingsDraft("security");
  }

  function setWorkspacePerformanceThreads(value: NumericSetting) {
    performanceThreads = value;
    markSettingsDraft("performance");
  }

  function setWorkspacePerformanceParallelJobs(value: NumericSetting) {
    performanceParallelJobs = value;
    markSettingsDraft("performance");
  }

  function setWorkspacePerformanceMemoryKiB(value: NumericSetting) {
    performanceMemoryKiB = value;
    markSettingsDraft("performance");
  }

  function isSettingsWorkspaceScreen(value: Screen): value is SettingsScreen {
    return value === "appearance"
      || value === "colors"
      || value === "settingsGeneral"
      || value === "settingsSecurity"
      || value === "settingsPerformance"
      || value === "passwordBook"
      || value === "integration";
  }

  function settingsWorkspaceProps(settingsScreen: SettingsScreen): SettingsWorkspaceProps {
    const selectedMode = uiModeChoice();
    return {
      screen: settingsScreen,
      tr,
      settingsSaveTarget,
      appearanceSaveState,
      modernModeSelected: selectedMode === "modern" || (selectedMode === null && mode === "modern"),
      classicModeSelected: selectedMode === "classic" || (selectedMode === null && mode === "classic"),
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
      setAccentContrastGuard: setWorkspaceAccentContrastGuard,
      generalSaveState,
      generalSettingsDirty,
      generalSettingsValidationError,
      saveGeneralSettings,
      availableLanguages,
      generalLanguageChoice,
      setGeneralLanguageChoice: setWorkspaceGeneralLanguageChoice,
      generalDefaultCreateDir,
      setGeneralDefaultCreateDir: setWorkspaceGeneralDefaultCreateDir,
      defaultCreateFolderError,
      chooseDefaultCreateFolder,
      clearDefaultCreateFolder,
      generalDefaultExtractDir,
      setGeneralDefaultExtractDir: setWorkspaceGeneralDefaultExtractDir,
      defaultExtractFolderError,
      chooseDefaultExtractFolder,
      clearDefaultExtractFolder,
      generalRevealAfterExtract,
      setGeneralRevealAfterExtract: setWorkspaceGeneralRevealAfterExtract,
      generalAutomaticUpdateChecks,
      setGeneralAutomaticUpdateChecks: setWorkspaceGeneralAutomaticUpdateChecks,
      fileManagerLabel,
      openWithLabel,
      updateCheckPreview,
      securitySaveState,
      safetySettingsDirty,
      safetyValidationError,
      saveSafetySettings,
      safetyMaxEntries,
      setSafetyMaxEntries: setWorkspaceSafetyMaxEntries,
      safetyMaxEntriesError,
      safetyMaxOutputGiB,
      setSafetyMaxOutputGiB: setWorkspaceSafetyMaxOutputGiB,
      safetyMaxOutputError,
      safetyMaxCompressionRatio,
      setSafetyMaxCompressionRatio: setWorkspaceSafetyMaxCompressionRatio,
      safetyMaxCompressionRatioError,
      resetSafetySettings,
      settingsSnapshotLabel,
      performanceSaveState,
      performanceSettingsDirty,
      performanceValidationError,
      savePerformanceSettings,
      performanceParallelJobs,
      setPerformanceParallelJobs: setWorkspacePerformanceParallelJobs,
      performanceParallelJobsError,
      choosePerformanceParallelJobs,
      performanceThreads,
      setPerformanceThreads: setWorkspacePerformanceThreads,
      performanceThreadsError,
      choosePerformanceThreads,
      performanceMemoryKiB,
      setPerformanceMemoryKiB: setWorkspacePerformanceMemoryKiB,
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
      passwordBookStatusState: passwordBookStatus.state,
      refreshPasswordBookPanel,
      currentArchiveName,
      formatRegistry: registryFormats(),
      formatRegistryLoaded: allFormats().length > 0,
      initialIntegrationStatus: runtimePreviews.integrationStatus,
      initialIntegrationDiagnostics: runtimePreviews.integrationDiagnostics,
      showMacosIntegrationDiagnostics: platformKind() === "macos",
      onNotice: showNotice,
    };
  }

  function classicArchiveBrowserSurface(): ClassicArchiveBrowserSurfaceProps {
    const workbenchVisible = Boolean(currentArchive && hasArchiveSelection());
    const rows = browseEntries(CLASSIC_ROW_HEIGHT).map((entry) => ({
      ...entry,
      selected: isEntrySelected(entry),
      previewing: isEntryPreviewActive(entry),
      previewBusy: isEntryPreviewBusy(entry),
      selectionLabel: entrySelectionLabel(entry),
      previewActionLabel: entry.source
        ? previewEntryActionLabel(entry)
        : "",
      previewActionIcon: entry.source
        ? previewActionIcon(entry.source.path, entry.source.entry_type)
        : "eye",
    }));
    const previewLabel = previewActionLabel();
    const previewDisabledReason = previewSelectedDisabledReason();
    return {
      view: {
        archiveTitle: archiveTitle(),
        archiveFormatSummary: currentArchive
          ? `${archiveFormat()} · ${archiveEntryCountLabel(currentArchive.entry_count)}`
          : openArchiveFirstLabel(),
        archiveOpen: Boolean(currentArchive),
        archiveReadOnly: Boolean(currentArchive?.read_only),
        openArchiveFirst: openArchiveFirstLabel(),
        selection: archiveSelectionControl(),
        preview: {
          policyKind: activePreviewPolicyKind(),
          policyCode: activePreviewPolicyCode(),
          nestedTitle: nestedPreview ? nestedPreviewTitle() : null,
          nestedSubtitle: nestedPreview ? nestedPreviewSubtitle() : "",
          nestedRows: nestedPreview ? nestedPreviewRows().map((item) => item.display) : [],
          title: entryPreviewTitle(),
          subtitle: entryPreviewSubtitle(),
          busy: previewBusy(),
          entry: entryPreview,
          failed: Boolean(entryPreviewFailure),
          canPreview: canPreviewEntrySelection(),
          actionLabel: previewLabel,
          actionIcon: previewActionIcon(),
          disabledReason: previewDisabledReason,
          ariaLabel: labelWithDisabledReason(previewLabel, previewDisabledReason),
        },
        rename: {
          visible: canRenameSelection(),
          value: renameTargetName,
          status: renameTargetStatus(),
        },
        move: {
          visible: hasArchiveSelection(),
          value: moveTargetDir,
          status: moveTargetStatus(),
          disabledReason: archiveMutationDisabledReason(),
        },
        newFolder: {
          value: newFolderName,
          status: workbenchVisible ? newFolderStatus() : "",
        },
        workbenchVisible,
        selectedSummary: selectedSummary(),
        conflict: moveConflictReview
          ? {
              count: moveConflictCount(),
              readyCount: moveReadyCount(),
              targetDir: moveConflictReview.targetDir,
              items: visibleMoveConflictItems(),
            }
          : null,
        structureWarning: archiveStructureWarningText(),
        totalRows: currentArchive ? totalRows() : 0,
        rows,
        paddingTop: browsePaddingTop(CLASSIC_ROW_HEIGHT),
        paddingBottom: browsePaddingBottom(CLASSIC_ROW_HEIGHT),
        emptyName: currentArchive ? noEntriesLabel() : openArchiveFirstLabel(),
        emptyStatus: currentArchive ? archiveFilterStatus() : noEntriesLabel(),
      },
      tr,
      onOpenRoot: () => void openArchiveBreadcrumb(-1),
      onOpenRecovery: openCurrentArchiveRecoveryConfiguration,
      onOpenNestedPreview: () => void openNestedPreviewArchive(),
      onExtractNestedPreview: () => void extractNestedPreviewArchive(),
      onClearPreview: (restoreEntryFocus) => clearEntryPreviewState(restoreEntryFocus),
      onRetryPreview: retryEntryPreview,
      onExtractPreviewFailure: () => void extractEntryPreviewFailure(),
      onOpenPreview: () => void openEntryPreview().then((opened) => {
        if (opened) clearEntryPreviewState();
      }),
      onRevealPreview: () => void revealEntryPreview(),
      onPreviewSelection: () => void submitPreviewEntry(),
      onRenameTargetChange: (value) => {
        renameTargetName = value;
      },
      onCommitRenameTarget: () => commitRenameTargetName(),
      onMoveTargetChange: (value) => {
        moveTargetDir = value;
      },
      onCommitMoveTarget: () => commitMoveTargetDir(),
      onNewFolderChange: (value) => {
        newFolderName = value;
      },
      onCommitNewFolder: () => commitNewFolderName(),
      onCancelMoveConflict: () => {
        moveConflictReview = null;
      },
      onSubmitMoveReadyOnly: () => void submitMoveReadyOnly(),
      onSubmitMoveKeepBoth: () => void submitMoveKeepBoth(),
      onBrowseScroll: onBrowseVirtualScroll,
      onSelectEntry: (entry, event) => selectEntry(entry, event),
      onActivateEntry: (entry) => void activateEntry(entry),
      onEntryKeydown: (event, entry) => onEntryKeydown(event, entry),
      onOpenEntryContext: (event, entry) => openEntryContext(event, entry),
      onToggleEntrySelection: (entry) => toggleEntrySelection(entry),
      onToggleAllEntries: toggleLoadedArchiveEntries,
      onPreviewEntry: (entry) => previewDisplayEntry(entry),
    };
  }

  function modernArchiveBrowserSurface(): ModernArchiveBrowserSurfaceProps {
    const archive = currentArchive;
    if (!archive) {
      throw new Error("Modern archive browser requires an open archive");
    }
    const rows = browseEntries(MODERN_ROW_HEIGHT).map((entry) => ({
      ...entry,
      selected: isEntrySelected(entry),
      previewing: isEntryPreviewActive(entry),
      previewBusy: isEntryPreviewBusy(entry),
      selectionLabel: entrySelectionLabel(entry),
      previewActionLabel: entry.source
        ? previewEntryActionLabel(entry)
        : "",
      previewActionIcon: entry.source
        ? previewActionIcon(entry.source.path, entry.source.entry_type)
        : "eye",
    }));
    const mutationDisabledReason = archiveMutationDisabledReason();
    const hasSelection = hasArchiveSelection();
    const previewLabel = previewActionLabel();
    const previewDisabledReason = previewSelectedDisabledReason();
    return {
      view: {
        archive: {
          title: archiveTitle(),
          format: archiveFormat(),
          summary: archiveSummary(),
          dirs: archiveDirs,
          readOnly: archive.read_only,
          canGoUp: canGoUpArchive(),
        },
        actions: {
          mutationDisabledReason,
          renameDisabledReason: renameSelectedDisabledReason(),
          deleteDisabledReason: deleteSelectedDisabledReason(),
          moveDisabledReason: moveSelectedDisabledReason(),
          canRenameSelection: canRenameSelection(),
          hasSelection,
          canPreviewSelection: canPreviewEntrySelection(),
          previewBusy: previewBusy(),
          previewDisabledReason,
          previewLabel,
          previewIcon: previewActionIcon(),
          extractDestinationHint: extractDestinationHint(),
          extractAllLabel: extractAllToLabel(),
          extractSelectedLabel: extractSelectedToLabel(),
          nestedPreview: Boolean(nestedPreview),
        },
        workbench: {
          renameTarget: renameTargetName,
          renameStatus: renameTargetStatus(),
          moveTarget: moveTargetDir,
          normalizedMoveTarget: normalizeMoveTargetDir(moveTargetDir),
          moveTargetPresets,
          moveStatus: moveTargetStatus(),
          newFolderName,
          newFolderStatus: hasSelection ? newFolderStatus() : "",
        },
        conflict: moveConflictReview
          ? {
              count: moveConflictCount(),
              readyCount: moveReadyCount(),
              targetDir: moveConflictReview.targetDir,
              items: visibleMoveConflictItems(),
            }
          : null,
        structureWarning: archiveStructureWarningText(),
        encodingWarning: hasEncodingWarning() ? archiveWarningText() : null,
        totalRows: totalRows(),
        filterText: filterText(),
        filterPending: filterPending(),
        filterStatus: archiveFilterStatus(),
        selection: archiveSelectionControl(),
        rows,
        paddingTop: browsePaddingTop(MODERN_ROW_HEIGHT),
        paddingBottom: browsePaddingBottom(MODERN_ROW_HEIGHT),
        emptyLabel: noEntriesLabel(),
      },
      tr,
      onOpenBreadcrumb: (index) => void openArchiveBreadcrumb(index),
      onGoUp: () => void goArchiveUp(),
      onOpenRoot: () => void openArchiveBreadcrumb(-1),
      onExtractAll: () => openExtractWorkspace("all"),
      onExtractSelection: () => openExtractWorkspace("selection"),
      onAddFiles: () => void submitAddToArchiveJob(),
      onOpenRecovery: openCurrentArchiveRecoveryConfiguration,
      onConvert: () => setScreen("convert"),
      onOpenInfo: () => setScreen("archiveInfo"),
      onRenameSelection: () => void submitRenameSelectedJob(),
      onDeleteSelection: () => void submitDeleteSelectedJob(),
      onMoveSelection: () => void submitMoveSelectedJob(),
      onCreateFolder: () => void submitNewFolderJob(),
      onPreviewSelection: () => void submitPreviewEntry(),
      onOpenNestedPreview: () => void openNestedPreviewArchive(),
      onExtractNestedPreview: () => void extractNestedPreviewArchive(),
      onRenameTargetChange: (value) => {
        renameTargetName = value;
      },
      onCommitRenameTarget: () => commitRenameTargetName(),
      onMoveTargetChange: (value) => {
        moveTargetDir = value;
      },
      onCommitMoveTarget: (target) => commitMoveTargetDir(target),
      onNewFolderChange: (value) => {
        newFolderName = value;
      },
      onCommitNewFolder: () => commitNewFolderName(),
      onCancelMoveConflict: () => {
        moveConflictReview = null;
      },
      onSubmitMoveReadyOnly: () => void submitMoveReadyOnly(),
      onSubmitMoveKeepBoth: () => void submitMoveKeepBoth(),
      onRepairEncoding: () => void repairFilenameEncoding("gbk"),
      onSearchInputMount: (input) => {
        archiveSearchInput = input;
      },
      onSearchInput: updateArchiveFilter,
      onSearchKeydown: onArchiveFilterKeydown,
      onClearSearch: clearArchiveFilter,
      onBrowseScroll: onBrowseVirtualScroll,
      onSelectEntry: (entry, event) => selectEntry(entry, event),
      onActivateEntry: (entry) => void activateEntry(entry),
      onEntryKeydown: (event, entry) => onEntryKeydown(event, entry),
      onOpenEntryContext: (event, entry) => openEntryContext(event, entry),
      onToggleEntrySelection: (entry) => toggleEntrySelection(entry),
      onToggleAllEntries: toggleLoadedArchiveEntries,
      onPreviewEntry: (entry) => previewDisplayEntry(entry),
    };
  }

  function modernInspectorSurface(): ModernInspectorSurfaceProps {
    let view: ModernInspectorSurfaceProps["view"];
    if (screen === "batch") {
      view = {
        kind: "batch",
        ready: batchReadyCount(),
        archives: batchReviewArchives().length,
        percent: batchReadyPercent(),
      };
    } else if (screen === "password") {
      view = {
        kind: "password",
        secretStore: secretStoreLabel(),
      };
    } else if (screen === "conflict") {
      view = { kind: "conflict" };
    } else if (screen === "recovery") {
      view = {
        kind: "recovery",
        tone: recoveryResultTone(),
        title: recoveryResultTitle(),
        detail: recoveryResultDetail(),
        metricsAvailable: recoveryMetricsAvailable(),
        explanation: recoveryResultExplanation(),
      };
    } else {
      view = {
        kind: "archive",
        preview: {
          policyKind: activePreviewPolicyKind(),
          policyCode: activePreviewPolicyCode(),
          nested: nestedPreview
            ? {
                title: nestedPreviewTitle(),
                subtitle: nestedPreviewSubtitle(),
                rows: nestedPreviewRows(),
              }
            : null,
          title: entryPreviewTitle(),
          subtitle: entryPreviewSubtitle(),
          busy: previewBusy(),
          failed: Boolean(entryPreviewFailure),
          entry: entryPreview,
          canPreview: canPreviewEntrySelection(),
          actionLabel: previewActionLabel(),
          actionIcon: previewActionIcon(),
          disabledReason: previewSelectedDisabledReason(),
        },
        canRename: canRenameSelection(),
        renameTarget: renameTargetName,
        renameStatus: renameTargetStatus(),
        canMove: hasArchiveSelection(),
        moveTarget: moveTargetDir,
        normalizedMoveTarget: normalizeMoveTargetDir(moveTargetDir),
        moveTargetPresets,
        moveStatus: moveTargetStatus(),
        archive: currentArchive
          ? {
              format: archiveFormat(),
              entries: currentArchive.entry_count,
              encoding: extractEncodingLabel(),
              volumes: currentArchive.volumes?.length
                ? archiveVolumeCountLabel(currentArchive.volumes.length)
                : tr("gui.archive.single", "Single"),
            }
          : null,
        openArchiveFirst: openArchiveFirstLabel(),
        archiveActionDisabledReason: archiveActionTitle(hasArchiveOpen()),
        selectionSummary: selectedSummary(),
        copyOutDisabledReason: copyOutSelectedDisabledReason(),
      };
    }
    return {
      view,
      tr,
      onOpenNestedPreview: () => void openNestedPreviewArchive(),
      onExtractNestedPreview: () => void extractNestedPreviewArchive(),
      onClearPreview: (restoreEntryFocus) => clearEntryPreviewState(restoreEntryFocus),
      onRetryPreview: retryEntryPreview,
      onExtractPreviewFailure: () => void extractEntryPreviewFailure(),
      onOpenPreview: () => void openEntryPreview().then((opened) => {
        if (opened) clearEntryPreviewState();
      }),
      onRevealPreview: () => void revealEntryPreview(),
      onPreviewSelection: () => void submitPreviewEntry(),
      onRenameTargetChange: (value) => (renameTargetName = value),
      onCommitRenameTarget: commitRenameTargetName,
      onMoveTargetChange: (value) => (moveTargetDir = value),
      onCommitMoveTarget: (target) => commitMoveTargetDir(target),
      onOpenRecovery: openRecoveryConfiguration,
      onTestArchive: () => void submitTestJob(),
      onCopyOutSelection: () => void submitCopyOutSelectedJob(),
    };
  }

  function beginSettingsSave(section: PersistedSettingsSection): number | null {
    if (settingsSaveTarget !== null) return null;
    settingsSaveTarget = section;
    settingsSaveOutcomes[section] = "idle";
    return settingsDraftGenerations[section];
  }

  function finishSettingsSave(
    section: PersistedSettingsSection,
    generation: number,
    outcome: SettingsSaveOutcome,
  ) {
    if (settingsSaveTarget === section) settingsSaveTarget = null;
    settingsSaveOutcomes[section] =
      settingsDraftGenerations[section] === generation || outcome !== "saved"
        ? outcome
        : "idle";
  }

  function settingsSaveState(
    section: PersistedSettingsSection,
    dirty: boolean,
  ): SettingsSaveState {
    if (settingsSaveTarget === section) return "saving";
    const outcome = settingsSaveOutcomes[section];
    if (dirty && outcome === "error") return "error";
    if (dirty && outcome === "session") return "session";
    return dirty ? "dirty" : "saved";
  }

  function applyPaletteSettingsSnapshot(settings: SettingsDto, preserveDraft = false) {
    const palette = isPaletteId(settings.accent_palette) ? settings.accent_palette : "aqua";
    const accent = normalizeHexColor(settings.custom_accent) ?? defaultCustomAccent;
    const contrastGuard = settings.accent_contrast_guard !== false;
    savedAccentPalette = palette;
    savedCustomAccent = accent;
    savedAccentContrastGuard = contrastGuard;
    if (preserveDraft) return;
    if (!hasPaletteOverride) activePalette = palette;
    customAccent = accent;
    customAccentInput = accent;
    customAccentSaveError = false;
    accentContrastGuard = contrastGuard;
    settingsSaveOutcomes.colors = "saved";
  }

  function applyAppearanceSettingsSnapshot(settings: SettingsDto, preserveDensity = false) {
    savedModeChoice =
      settings.ui_mode === "modern" || settings.ui_mode === "classic"
        ? settings.ui_mode
        : null;
    savedThemeChoice = isThemeChoice(settings.theme) ? settings.theme : "system";
    savedDensityChoice = isDensityChoice(settings.ui_density)
      ? settings.ui_density
      : "standard";
    if (!preserveDensity && !hasDensityOverride && isDensityChoice(settings.ui_density)) {
      activeDensityChoice = settings.ui_density;
    }
  }

  function applyGeneralSettingsSnapshot(settings: SettingsDto, preserveDraft = false) {
    const language = settings.language ?? "";
    const defaultCreateDir = normalizedDefaultCreateDir(settings.default_create_dir ?? "") ?? "";
    const defaultExtractDir = normalizedDefaultExtractDir(settings.default_extract_dir ?? "") ?? "";
    const revealAfterExtract = settings.reveal_after_extract === true;
    const automaticUpdateChecks = settings.check_updates_automatically !== false;
    savedGeneralLanguageChoice = language;
    savedGeneralDefaultCreateDir = defaultCreateDir;
    savedGeneralDefaultExtractDir = defaultExtractDir;
    savedGeneralRevealAfterExtract = revealAfterExtract;
    savedGeneralAutomaticUpdateChecks = automaticUpdateChecks;
    appliedGeneralLanguageChoice = language;
    appliedDefaultCreateDir = defaultCreateDir;
    appliedDefaultExtractDir = defaultExtractDir;
    appliedGeneralRevealAfterExtract = revealAfterExtract;
    appliedGeneralAutomaticUpdateChecks = automaticUpdateChecks;
    setRevealAfterExtractPreference(revealAfterExtract);
    if (preserveDraft) return;
    generalLanguageChoice = language;
    generalDefaultCreateDir = defaultCreateDir;
    generalDefaultExtractDir = defaultExtractDir;
    generalRevealAfterExtract = revealAfterExtract;
    generalAutomaticUpdateChecks = automaticUpdateChecks;
    settingsSaveOutcomes.general = "saved";
  }

  async function runAutomaticUpdateCheck(generation: number): Promise<void> {
    try {
      const { checkForSoftwareUpdates } = await import("./lib/app-update.svelte");
      const result = await checkForSoftwareUpdates("automatic");
      if (
        !automaticUpdateChecksMounted
        || generation !== automaticUpdateCheckGeneration
        || result?.status !== "update_available"
      ) return;
      pushToast({
        key: "software-update-available",
        kind: "info",
        title: tr("gui.update.automatic_available_title", "Squallz {version} is available")
          .replace("{version}", `v${result.latestVersion}`),
        body: tr(
          "gui.update.automatic_available_body",
          "Review the package signature, checksum, and release details before downloading.",
        ),
        action: {
          label: tr("gui.update.review", "Review update"),
          run: () => setScreen("settingsGeneral"),
        },
        persistent: true,
      });
    } catch {
      // The manual check in Settings remains available if this background load fails.
    }
  }

  const automaticUpdateCheckDelayMs = 5_000;
  let automaticUpdateCheckTimer: ReturnType<typeof setTimeout> | null = null;
  let automaticUpdateCheckGeneration = 0;
  let automaticUpdateChecksMounted = false;

  function startAutomaticUpdateCheck(enabled: boolean): void {
    const generation = ++automaticUpdateCheckGeneration;
    if (automaticUpdateCheckTimer !== null) {
      clearTimeout(automaticUpdateCheckTimer);
      automaticUpdateCheckTimer = null;
    }
    if (
      !automaticUpdateChecksMounted
      || !enabled
      || taskWindowMode
      || updateCheckPreview !== null
    ) return;
    automaticUpdateCheckTimer = setTimeout(() => {
      automaticUpdateCheckTimer = null;
      void runAutomaticUpdateCheck(generation);
    }, automaticUpdateCheckDelayMs);
  }

  onMount(() => {
    automaticUpdateChecksMounted = true;
    return () => {
      automaticUpdateChecksMounted = false;
      automaticUpdateCheckGeneration += 1;
      if (automaticUpdateCheckTimer !== null) clearTimeout(automaticUpdateCheckTimer);
    };
  });

  function safetyValuesFromSettings(settings: SettingsDto) {
    return {
      maxOutputGiB:
        settings.safety_max_output_bytes && settings.safety_max_output_bytes > 0
          ? Math.max(1, Math.round(settings.safety_max_output_bytes / bytesPerGiB))
          : defaultSafety.maxOutputGiB,
      maxEntries:
        settings.safety_max_entries && settings.safety_max_entries > 0
          ? settings.safety_max_entries
          : defaultSafety.maxEntries,
      maxCompressionRatio:
        settings.safety_max_compression_ratio && settings.safety_max_compression_ratio > 0
          ? settings.safety_max_compression_ratio
          : defaultSafety.maxCompressionRatio,
    };
  }

  function applySafetySettingsSnapshot(settings: SettingsDto, preserveDraft = false) {
    const values = safetyValuesFromSettings(settings);
    savedSafetyMaxOutputGiB = values.maxOutputGiB;
    savedSafetyMaxEntries = values.maxEntries;
    savedSafetyMaxCompressionRatio = values.maxCompressionRatio;
    savedSafetyCustom = Boolean(
      settings.safety_max_output_bytes ||
        settings.safety_max_entries ||
        settings.safety_max_compression_ratio,
    );
    if (preserveDraft) return;
    safetyMaxOutputGiB = values.maxOutputGiB;
    safetyMaxEntries = values.maxEntries;
    safetyMaxCompressionRatio = values.maxCompressionRatio;
    settingsSaveOutcomes.security = "saved";
  }

  function performanceValuesFromSettings(settings: SettingsDto) {
    return {
      parallelJobs:
        settings.performance_parallel_jobs && settings.performance_parallel_jobs > 0
          ? Math.min(settings.performance_parallel_jobs, 8)
          : null,
      threads:
        settings.performance_threads && settings.performance_threads > 0
          ? Math.min(settings.performance_threads, 64)
          : null,
      memoryKiB:
        settings.performance_memory_limit_bytes && settings.performance_memory_limit_bytes > 0
          ? wholeSetting(
              Math.round(settings.performance_memory_limit_bytes / bytesPerKiB),
              64,
              8,
              64,
            )
          : null,
    };
  }

  function applyPerformanceSettingsSnapshot(settings: SettingsDto, preserveDraft = false) {
    const values = performanceValuesFromSettings(settings);
    savedPerformanceParallelJobs = values.parallelJobs;
    savedPerformanceThreads = values.threads;
    savedPerformanceMemoryKiB = values.memoryKiB;
    if (preserveDraft) return;
    performanceParallelJobs = values.parallelJobs;
    performanceThreads = values.threads;
    performanceMemoryKiB = values.memoryKiB;
    settingsSaveOutcomes.performance = "saved";
  }

  function updateSettingsSnapshotLabel() {
    const parallelLabel =
      savedPerformanceParallelJobs === null
        ? tr("gui.settings.snapshot.parallel_auto", "parallel auto")
        : tr("gui.settings.snapshot.parallel_count", "{count} parallel")
            .replace("{count}", String(savedPerformanceParallelJobs));
    const workerLabel =
      savedPerformanceThreads === null
        ? tr("gui.settings.snapshot.workers_auto", "encoder threads auto")
        : tr("gui.settings.snapshot.workers_count", "{count} encoder threads")
            .replace("{count}", String(savedPerformanceThreads));
    const memoryLabel =
      savedPerformanceMemoryKiB === null
        ? tr("gui.settings.snapshot.buffer_auto", "buffer auto")
        : tr("gui.settings.snapshot.buffer_kib", "{count} KiB buffer")
            .replace("{count}", formattedNumber(savedPerformanceMemoryKiB, 64));
    settingsSnapshotLabel = tr(
      "gui.settings.snapshot.summary",
      "Saved settings · {safety} · {parallel} · {workers} · {buffer}",
    )
      .replace(
        "{safety}",
        savedSafetyCustom
          ? tr("gui.settings.snapshot.custom_safety", "Custom safety")
          : tr("gui.settings.snapshot.default_safety", "Default safety"),
      )
      .replace("{parallel}", parallelLabel)
      .replace("{workers}", workerLabel)
      .replace("{buffer}", memoryLabel);
  }

  function applySettingsSnapshot(
    settings: SettingsDto,
    requestedGenerations: Readonly<Record<PersistedSettingsSection, number>>,
    preserveAppearance: boolean,
  ) {
    applyPaletteSettingsSnapshot(
      settings,
      settingsDraftGenerations.colors !== requestedGenerations.colors,
    );
    applyAppearanceSettingsSnapshot(settings, preserveAppearance);
    applyGeneralSettingsSnapshot(
      settings,
      settingsDraftGenerations.general !== requestedGenerations.general,
    );
    applySafetySettingsSnapshot(
      settings,
      settingsDraftGenerations.security !== requestedGenerations.security,
    );
    applyPerformanceSettingsSnapshot(
      settings,
      settingsDraftGenerations.performance !== requestedGenerations.performance,
    );
    updateSettingsSnapshotLabel();
  }

  function wholeSetting(value: NumericSetting, fallback: number, min: number, max: number): number {
    const numberValue = typeof value === "number" && Number.isFinite(value) ? value : fallback;
    return Math.min(max, Math.max(min, Math.round(numberValue)));
  }

  function numericRangeMessage(label: string, min: number, max: number): string {
    return tr("gui.settings.number.invalid_range", "{label} must be a whole number from {min} to {max}")
      .replace("{label}", label)
      .replace("{min}", numberFormatter.format(min))
      .replace("{max}", numberFormatter.format(max));
  }

  function requiredWholeSettingError(
    value: NumericSetting,
    min: number,
    max: number,
    label: string,
  ): string {
    return typeof value !== "number" ||
      !Number.isFinite(value) ||
      !Number.isInteger(value) ||
      value < min ||
      value > max
      ? numericRangeMessage(label, min, max)
      : "";
  }

  function optionalWholeSettingError(
    value: NumericSetting,
    min: number,
    max: number,
    label: string,
  ): string {
    return value === null ? "" : requiredWholeSettingError(value, min, max, label);
  }

  function showNumericRangeNotice(label: string, min: number, max: number) {
    showNotice(numericRangeMessage(label, min, max));
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
    const generation = beginSettingsSave("security");
    if (generation === null) return;
    const useDefaults =
      maxOutputGiB === defaultSafety.maxOutputGiB &&
      maxEntries === defaultSafety.maxEntries &&
      maxCompressionRatio === defaultSafety.maxCompressionRatio;

    try {
      const settings = await ipc.setSafetyLimits(
        useDefaults ? null : maxOutputGiB * bytesPerGiB,
        useDefaults ? null : maxEntries,
        useDefaults ? null : maxCompressionRatio,
      );
      applySafetySettingsSnapshot(
        settings,
        settingsDraftGenerations.security !== generation,
      );
      updateSettingsSnapshotLabel();
      finishSettingsSave("security", generation, "saved");
      showNotice(tr("gui.settings.security.saved", "Security settings saved"));
    } catch (error) {
      if (isSettingsPersistenceFailure(error)) {
        finishSettingsSave("security", generation, "error");
        showNotice(settingsPersistenceFailureLabel());
      } else {
        finishSettingsSave("security", generation, "error");
        showNotice(
          tr(
            "gui.settings.apply_failed",
            "Could not apply these settings. Try again from the desktop app.",
          ),
        );
      }
    }
  }

  function resetSafetySettings() {
    safetyMaxOutputGiB = defaultSafety.maxOutputGiB;
    safetyMaxEntries = defaultSafety.maxEntries;
    safetyMaxCompressionRatio = defaultSafety.maxCompressionRatio;
    markSettingsDraft("security");
  }

  function choosePerformanceThreads(next: NumericSetting) {
    performanceThreads = next;
    markSettingsDraft("performance");
  }

  function choosePerformanceParallelJobs(next: NumericSetting) {
    performanceParallelJobs = next;
    markSettingsDraft("performance");
  }

  function choosePerformanceMemory(next: NumericSetting) {
    performanceMemoryKiB = next;
    markSettingsDraft("performance");
  }

  async function savePerformanceSettings() {
    const parallelJobs = validateOptionalWholeSetting(
      performanceParallelJobs,
      1,
      8,
      tr("gui.settings.performance.custom_parallel_jobs", "Custom parallel tasks"),
    );
    if (parallelJobs === undefined) return;
    const threads = validateOptionalWholeSetting(
      performanceThreads,
      1,
      64,
      tr("gui.settings.performance.custom_threads", "Custom threads"),
    );
    if (threads === undefined) return;
    const memoryKiB = validateOptionalWholeSetting(
      performanceMemoryKiB,
      8,
      64,
      tr("gui.settings.performance.custom_buffer_kib", "Custom buffer KiB"),
    );
    if (memoryKiB === undefined) return;
    performanceParallelJobs = parallelJobs;
    performanceThreads = threads;
    performanceMemoryKiB = memoryKiB;
    const generation = beginSettingsSave("performance");
    if (generation === null) return;

    try {
      const settings = await ipc.setPerformanceOptions(
        threads,
        memoryKiB === null ? null : memoryKiB * bytesPerKiB,
        parallelJobs,
      );
      applyPerformanceSettingsSnapshot(
        settings,
        settingsDraftGenerations.performance !== generation,
      );
      updateSettingsSnapshotLabel();
      finishSettingsSave("performance", generation, "saved");
      showNotice(tr("gui.settings.performance.saved", "Performance settings saved"));
    } catch (error) {
      if (isSettingsPersistenceFailure(error)) {
        finishSettingsSave("performance", generation, "error");
        showNotice(settingsPersistenceFailureLabel());
      } else {
        finishSettingsSave("performance", generation, "error");
        showNotice(
          tr(
            "gui.settings.apply_failed",
            "Could not apply these settings. Try again from the desktop app.",
          ),
        );
      }
    }
  }

  function resetPerformanceSettings() {
    performanceParallelJobs = null;
    performanceThreads = null;
    performanceMemoryKiB = null;
    markSettingsDraft("performance");
  }

  async function savePaletteSettings() {
    const payload = palettePayloadForSave();
    if (!payload) {
      showNotice(tr("gui.colors.invalid_hex", "Enter a valid #RRGGBB color"));
      return;
    }
    const generation = beginSettingsSave("colors");
    if (generation === null) return;
    try {
      const settings = await ipc.setAccentPalette(payload.palette, payload.customAccent, payload.contrastGuard);
      applyPaletteSettingsSnapshot(
        settings,
        settingsDraftGenerations.colors !== generation,
      );
      finishSettingsSave("colors", generation, "saved");
      syncUrl();
      showNotice(tr("gui.colors.saved", "Theme colors saved"));
    } catch (error) {
      finishSettingsSave(
        "colors",
        generation,
        isSettingsPersistenceFailure(error) ? "error" : "session",
      );
      showNotice(
        isSettingsPersistenceFailure(error)
          ? settingsPersistenceFailureLabel()
          : tr("gui.colors.saved_preview", "Theme colors apply to this session but were not saved"),
      );
    }
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

  function showSourceCleanupRecovery(notice: SourceCleanupRecoveryNotice) {
    const trash = platformTrashName();
    if (notice.status === "restored") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.restored",
          "Squallz restored an original after an interrupted cleanup; review it before continuing",
        ),
        body: notice.path
          ? tr("gui.toast.source_recovery.restored_path", "Restored to {path}").replace(
              "{path}",
              notice.path,
            )
          : undefined,
      });
      return;
    }
    if (notice.status === "cleared") {
      pushToast({
        kind: "info",
        title: tr(
          "gui.toast.source_recovery.cleared",
          "An interrupted source cleanup was safely reset; the original stayed in place",
        ),
      });
      return;
    }
    if (notice.status === "preserved") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.preserved",
          "Squallz preserved an original after an interrupted cleanup; review it before continuing",
        ),
        body: notice.path
          ? tr("gui.toast.source_recovery.path", "Preserved at {path}").replace("{path}", notice.path)
          : undefined,
      });
      return;
    }
    if (notice.status === "changed") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.changed",
          "An interrupted cleanup found a different item at a recovery path; it was not moved to Trash",
        ),
        body: notice.path
          ? tr(
              "gui.toast.source_recovery.changed_path",
              "Review {path}, the original location, and Trash before continuing",
            ).replace("{path}", notice.path)
          : undefined,
      });
      return;
    }
    if (notice.status === "completed_unknown") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.completed_unknown",
          "An interrupted cleanup may have reached {trash}; check the original location and {trash}",
        ).replaceAll("{trash}", trash),
        body: notice.path
          ? tr(
              "gui.toast.source_recovery.original_path",
              "Original location: {path}",
            ).replace("{path}", notice.path)
          : undefined,
      });
      return;
    }
    if (notice.status === "busy") {
      pushToast({
        key: sourceCleanupBusyToastKey,
        kind: "warning",
        persistent: true,
        title: tr(
          "gui.toast.source_recovery.busy",
          "Another Squallz window is finishing source cleanup; moving originals is temporarily unavailable",
        ),
      });
      return;
    }
    pushToast({
      kind: "danger",
      title: tr(
        "gui.toast.source_recovery.needs_attention",
        "Source cleanup is paused; review the recovery record before moving any originals to {trash}",
      ).replace("{trash}", trash),
      body: sourceCleanupRecoveryReason(notice.reason),
      action: {
        label: tr(
          "gui.toast.source_recovery.show_record",
          "Show recovery record",
        ),
        run: () => showSourceCleanupRecoveryRecord(notice.journal_path),
      },
    });
  }

  function sourceCleanupRecoveryReason(
    reason: SourceCleanupRecoveryNotice["reason"],
  ): string {
    if (reason === "journal_invalid") {
      return tr(
        "gui.toast.source_recovery.reason_invalid",
        "The recovery record is damaged or no longer matches its files. Do not delete or move the related items manually until you review it.",
      );
    }
    if (reason === "journal_permission_denied") {
      return tr(
        "gui.toast.source_recovery.reason_permission",
        "Squallz cannot read the recovery record. Check its permissions before retrying source cleanup.",
      );
    }
    if (reason === "journal_unavailable") {
      return tr(
        "gui.toast.source_recovery.reason_unavailable",
        "Squallz cannot access its recovery storage. Keep the original and any preserved copy until the storage is available.",
      );
    }
    return tr(
      "gui.toast.source_recovery.reason_failed",
      "Squallz could not safely determine where an original was left. Review the recovery record, the original location, and Trash before continuing.",
    );
  }

  async function showSourceCleanupRecoveryRecord(path: string | null): Promise<boolean> {
    if (!path) {
      showSourceCleanupRecoveryRecordFailure(null);
      return false;
    }
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(path);
      return true;
    } catch {
      showSourceCleanupRecoveryRecordFailure(path);
      return false;
    }
  }

  function showSourceCleanupRecoveryRecordFailure(path: string | null): void {
    pushToast({
      kind: "warning",
      title: tr(
        "gui.toast.source_recovery.show_record_failed",
        "Could not show the recovery record",
      ),
      body: path
        ? tr(
            "gui.toast.source_recovery.show_record_failed_path",
            "Recovery record: {path}. Copy this path and open it manually before moving or deleting the related originals.",
          ).replace("{path}", path)
        : tr(
            "gui.toast.source_recovery.show_record_failed_detail",
            "Open Squallz's configuration folder and inspect source-cleanup.json before moving or deleting the related originals.",
          ),
    });
  }

  async function reportStartupSourceCleanupRecovery() {
    if (sourceCleanupRecoveryRequestInFlight) {
      sourceCleanupRecoveryRefreshPending = true;
      return;
    }
    sourceCleanupRecoveryRequestInFlight = true;
    try {
      do {
        sourceCleanupRecoveryRefreshPending = false;
        let retryBusy = false;
        try {
          const notice = await ipc.getSourceCleanupRecovery();
          if (notice?.status !== "busy") {
            removeToastByKey(sourceCleanupBusyToastKey);
          }
          if (
            notice &&
            isNewSourceCleanupRecoveryGeneration(
              sourceCleanupRecoveryLastGeneration,
              notice.generation,
            )
          ) {
            sourceCleanupRecoveryLastGeneration = notice.generation;
            showSourceCleanupRecovery(notice);
          }
          retryBusy = notice?.status === "busy";
        } catch {
          // Browser previews have no native source-cleanup service.
        }
        if (sourceCleanupRecoveryRetry !== null) clearTimeout(sourceCleanupRecoveryRetry);
        sourceCleanupRecoveryRetry = retryBusy
          ? setTimeout(() => {
              sourceCleanupRecoveryRetry = null;
              void reportStartupSourceCleanupRecovery();
            }, 1500)
          : null;
      } while (sourceCleanupRecoveryRefreshPending);
    } finally {
      sourceCleanupRecoveryRequestInFlight = false;
    }
  }

  function isSettingsPersistenceFailure(error: unknown): boolean {
    return isErrorDto(error) && error.key === "error.settings_write";
  }

  function settingsPersistenceFailureLabel(): string {
    return tr(
      "gui.settings.save_failed",
      "Could not save settings. Check disk access and try again.",
    );
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

  function trashNameLabel(): string {
    return platformTrashName(platformKind());
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

  function classicArchiveStartVisible(): boolean {
    return screen === "browse" && !currentArchive;
  }

  function openArchiveFirstLabel(): string {
    return tr("gui.empty.open_archive_first", "Open archive");
  }

  function noEntriesLabel(): string {
    if (filterPending()) return archiveFilterStatus();
    if (archiveBrowseError()) {
      return filterText().trim()
        ? tr("gui.empty.search_failed", "Search stopped before results could load")
        : tr("gui.empty.browse_failed", "Folder contents could not be loaded");
    }
    if (filterText().trim()) {
      return tr("gui.empty.no_search_matches", "No entries match this search");
    }
    return tr("gui.empty.no_entries", "No entries");
  }

  function archiveFilterStatus(): string {
    if (archiveBrowseError()) {
      return filterText().trim()
        ? tr("gui.list.search_failed", "Search could not be completed")
        : tr("gui.list.browse_failed", "Folder contents could not be loaded");
    }
    if (filterPending()) {
      return filterText().trim()
        ? tr("gui.list.searching", "Searching the entire archive…")
        : tr("gui.list.loading_folder", "Loading folder contents…");
    }
    const count = totalRows().toLocaleString();
    if (filterText().trim()) {
      return tr("gui.list.search_result_count", "{count} matches across the archive").replace("{count}", count);
    }
    return tr("gui.list.current_folder_count", "{count} items in this folder").replace("{count}", count);
  }

  function updateArchiveFilter(value: string): void {
    browseScrollTop = 0;
    clearEntryPreviewState();
    setFilter(value);
  }

  function clearArchiveFilter(): void {
    updateArchiveFilter("");
    queueMicrotask(() => archiveSearchInput?.focus());
  }

  function onArchiveFilterKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || !filterText()) return;
    event.preventDefault();
    event.stopPropagation();
    clearArchiveFilter();
  }

  function normalizedFolderSetting(value: string): string | null {
    return normalizeDesktopFolder(value, platformKind());
  }

  function folderSettingValidationError(value: string, label: string): string {
    if (!value.trim() || normalizedFolderSetting(value)) return "";
    return tr(
      "gui.settings.folder.absolute_required",
      "{name} must be an absolute folder path. Choose a folder or clear the field.",
    ).replace("{name}", label);
  }

  function normalizedDefaultCreateDir(value: string = generalDefaultCreateDir): string | null {
    return normalizedFolderSetting(value);
  }

  function normalizedDefaultExtractDir(value: string = generalDefaultExtractDir): string | null {
    return normalizedFolderSetting(value);
  }

  function defaultCreateFolderLabel(value: string = generalDefaultCreateDir): string {
    return normalizedDefaultCreateDir(value) ?? tr("gui.settings.folder.ask_when_creating", "Ask when creating");
  }

  function defaultExtractFolderLabel(value: string = generalDefaultExtractDir): string {
    return normalizedDefaultExtractDir(value) ?? tr("gui.settings.folder.next_to_archive", "Next to archive");
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
        markSettingsDraft("general");
      }
    } catch {
      showNotice(tr("gui.settings.folder.picker_requires_desktop_service", "Folder picker requires the desktop service"));
    }
  }

  async function chooseDefaultCreateFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("settings.default-create-folder", open, {
        title: tr("gui.settings.folder.choose_create_title", "Choose default create folder"),
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        generalDefaultCreateDir = selected;
        markSettingsDraft("general");
      }
    } catch {
      showNotice(tr("gui.settings.folder.picker_requires_desktop_service", "Folder picker requires the desktop service"));
    }
  }

  function clearDefaultCreateFolder() {
    generalDefaultCreateDir = "";
    markSettingsDraft("general");
  }

  function clearDefaultExtractFolder() {
    generalDefaultExtractDir = "";
    markSettingsDraft("general");
  }

  async function saveGeneralSettings() {
    if (generalSettingsValidationError) {
      showNotice(generalSettingsValidationError);
      return;
    }
    const nextLanguage = generalLanguageChoice.trim() || null;
    const defaultCreateDir = normalizedDefaultCreateDir();
    const defaultExtractDir = normalizedDefaultExtractDir();
    const revealAfterExtract = generalRevealAfterExtract;
    const automaticUpdateChecks = generalAutomaticUpdateChecks;
    const previousAppliedLanguage = appliedGeneralLanguageChoice;
    const requiresPersistence =
      (nextLanguage ?? "") !== savedGeneralLanguageChoice ||
      (defaultCreateDir ?? "") !== savedGeneralDefaultCreateDir ||
      (defaultExtractDir ?? "") !== savedGeneralDefaultExtractDir ||
      revealAfterExtract !== savedGeneralRevealAfterExtract ||
      automaticUpdateChecks !== savedGeneralAutomaticUpdateChecks;
    const generation = beginSettingsSave("general");
    if (generation === null) return;
    try {
      const settings = await ipc.setGeneralOptions(
        nextLanguage,
        defaultCreateDir,
        defaultExtractDir,
        revealAfterExtract,
        automaticUpdateChecks,
      );
      storePreviewLanguage(settings.language);
      applyGeneralSettingsSnapshot(
        settings,
        settingsDraftGenerations.general !== generation,
      );
      await loadLocale(settings.language).catch(() => undefined);
      updateSettingsSnapshotLabel();
      finishSettingsSave("general", generation, "saved");
      recordOperation({
        status: "done",
        title: tr("gui.settings.general.saved", "General settings saved"),
        detail: tr(
          "gui.settings.general.saved_detail",
          "Language: {language} · Create folder: {createFolder} · Extract folder: {folder} · Reveal after extract {reveal} · Automatic update checks {updates}",
        )
          .replace("{language}", languageLabel(settings.language))
          .replace("{createFolder}", defaultCreateFolderLabel(settings.default_create_dir ?? ""))
          .replace("{folder}", defaultExtractFolderLabel(settings.default_extract_dir ?? ""))
          .replace("{reveal}", settings.reveal_after_extract ? tr("common.on", "on") : tr("common.off", "off"))
          .replace(
            "{updates}",
            settings.check_updates_automatically !== false
              ? tr("common.on", "on")
              : tr("common.off", "off"),
          ),
      });
      showNotice(tr("gui.settings.general.saved", "General settings saved"));
      startAutomaticUpdateCheck(settings.check_updates_automatically !== false);
    } catch (error) {
      if (isSettingsPersistenceFailure(error)) {
        finishSettingsSave("general", generation, "error");
        showNotice(settingsPersistenceFailureLabel());
      } else {
        let sessionApplied = false;
        if (settingsDraftGenerations.general === generation) {
          await loadLocale(nextLanguage).catch(() => undefined);
          if (settingsDraftGenerations.general === generation) {
            appliedGeneralLanguageChoice = nextLanguage ?? "";
            appliedDefaultCreateDir = defaultCreateDir ?? "";
            appliedDefaultExtractDir = defaultExtractDir ?? "";
            appliedGeneralRevealAfterExtract = revealAfterExtract;
            appliedGeneralAutomaticUpdateChecks = automaticUpdateChecks;
            storePreviewLanguage(nextLanguage);
            setRevealAfterExtractPreference(revealAfterExtract);
            updateSettingsSnapshotLabel();
            sessionApplied = true;
            startAutomaticUpdateCheck(automaticUpdateChecks);
          } else {
            await loadLocale(previousAppliedLanguage || null).catch(() => undefined);
            updateSettingsSnapshotLabel();
          }
        }
        finishSettingsSave(
          "general",
          generation,
          sessionApplied ? (requiresPersistence ? "session" : "saved") : "error",
        );
        showNotice(
          sessionApplied
            ? requiresPersistence
              ? tr(
                  "gui.settings.general.saved_preview",
                  "General changes apply to this session but were not saved",
                )
              : tr(
                  "gui.settings.general.matches_saved",
                  "General settings now match the saved values",
                )
            : tr(
                "gui.settings.previous_apply_failed",
                "Earlier changes were not applied. Review the current draft and save again.",
              ),
        );
      }
    }
  }

  function openRecoveryConfiguration(source: "preserve" | "current" = "preserve") {
    if (preventCreateSubmissionNavigation("recovery")) return;
    const route = recoveryRouteForOpen(source, Boolean(currentArchive), {
      sourceMode: recoverySourceMode,
      sourceOverride: recoverySourceOverride,
      par2Override: recoveryPar2Override,
    });
    recoverySourceMode = route.sourceMode;
    recoverySourceOverride = route.sourceOverride;
    recoveryPar2Override = route.par2Override;
    setScreen("recovery");
    showNotice(
      recoverySourcePath()
        ? tr("gui.recovery.route_for_archive", "Recovery opened for {name}.")
          .replace("{name}", recoverySourceName() ?? tr("gui.archive.generic", "Archive"))
        : tr("gui.recovery.choose_archive_to_begin", "Choose a damaged archive or a PAR2 file to begin."),
    );
  }

  function openCurrentArchiveRecoveryConfiguration() {
    openRecoveryConfiguration("current");
  }

  async function openArchiveFromDialog() {
    if (archiveOpenStatus === "opening") return;
    if (preventCreateSubmissionNavigation("browse")) return;
    const requestGeneration = ++archiveOpenGeneration;
    archiveOpenStatus = "opening";
    showNotice(tr("gui.archive.opening_picker", "Opening file picker..."));
    try {
      const { open } = await getDialogModule();
      // An unfiltered macOS dialog keeps arbitrary numbered volumes selectable.
      const filters: OpenDialogOptions["filters"] = platformKind() === "macos"
        ? undefined
        : [
            {
              name: tr("gui.archive.filter_all_files", "All files"),
              extensions: ["*"],
            },
            {
              name: tr("gui.archive.filter_archives", "Archives"),
              extensions: [
                ...registryFormatExtensions(),
                ...legacyRarVolumeExtensions,
                ...nativeSplitZipVolumeExtensions,
                "001",
              ],
            },
            { name: tr("gui.recovery.par2_files", "PAR2 recovery files"), extensions: ["par2"] },
          ];
      const selected = await openNativeDialog("archive.open", open, {
        title: tr("gui.archive.open_dialog_title", "Open archive"),
        multiple: false,
        directory: false,
        filters,
      });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (typeof path === "string") {
        await openArchivePath(path, "dialog");
      } else {
        showNotice(tr("gui.archive.open_cancelled", "Open archive cancelled."));
      }
    } catch {
      showNotice(tr("gui.archive.open_requires_desktop_dialog", "Open archive requires the desktop file dialog"));
    } finally {
      if (requestGeneration === archiveOpenGeneration) archiveOpenStatus = "idle";
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
    if (preventCreateSubmissionNavigation("browse")) return;
    if (openRecoverySetFromPaths(payload.paths, "open-file")) return;
    await openFirstArchivePath(payload.paths, "open-file");
  }

  async function submitExternalTaskWindow(action: ExternalOpenAction, paths: string[], output: string | null) {
    function applyTaskWindowSubmitTransition(transition: TaskWindowSubmitTransition) {
      taskWindowLaunchState = transition.state;
      if (transition.notice) showNotice(transition.notice);
    }

    applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "starting", tr));
    let resolvedSpec: JobSpec | null;
    try {
      resolvedSpec = await ipc.resolveExternalTaskJob(
        action,
        paths,
        output,
        checksumAlgorithm,
        checksumExcludeRules(),
      );
    } catch (error) {
      if (isErrorDto(error)) {
        applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "preset-error", tr));
        showNotice(tr(error.key, tr("gui.presets.load_failed", "Could not load presets. The preset file was not changed.")));
        return;
      }
      if (!import.meta.env.DEV) {
        applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "requires-desktop-service", tr));
        return;
      }
      resolvedSpec = buildExternalTaskJobSpec(action, {
        paths,
        output,
        checksumAlgorithm,
        checksumExcludes: checksumExcludeRules(),
        archiveStemName,
      });
    }
    const plan = taskWindowSubmitPlan(action, resolvedSpec, tr);
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

  async function openArchivePath(
    path: string,
    source: "dialog" | "open-file",
  ): Promise<boolean> {
    if (preventCreateSubmissionNavigation("browse")) return false;
    if (isPar2Path(path)) {
      openRecoverySet(path, null, source);
      return true;
    }
    const requestGeneration = ++archiveOpenGeneration;
    archiveOpenStatus = "opening";
    const ok = await openArchiveStore(path);
    if (requestGeneration !== archiveOpenGeneration) return true;
    archiveOpenStatus = "idle";
    if (ok) {
      finishOpenedArchive(path, source);
      return true;
    }
    const passwordPrompt = openPasswordPrompt();
    if (passwordPrompt?.path === path) {
      setScreenRespectingJobQuestion("password");
      showNotice(tr("gui.archive.password_needed", "Enter the archive password to continue."));
      return true;
    }
    if (archiveOpenError(path)?.key === "error.corrupt_archive") {
      recoverySourceMode = "selected";
      recoverySourceOverride = path;
      recoveryPar2Override = null;
      setScreenRespectingJobQuestion("recovery");
      showNotice(
        tr("gui.recovery.open_failed_routed", "{name} could not be opened. It is ready for recovery checks.")
          .replace("{name}", pathBaseName(path)),
      );
      return true;
    }
    if (!archiveOpenError(path)) return false;
    showNotice(archiveOpenFailureNotice(path));
    return false;
  }

  function finishOpenedArchive(path: string, source: "dialog" | "open-file" | "password") {
    rememberRecent(path);
    recordOperation({
      status: "info",
      title: tr("gui.archive.opened_operation", "Opened archive"),
      detail: pathBaseName(path),
    });
    recoverySourceMode = "current";
    recoverySourceOverride = null;
    recoveryPar2Override = null;
    clearEntryPreviewState();
    setScreenRespectingJobQuestion("browse");
    showNotice(
      source === "password"
        ? tr("gui.archive.unlocked", "Archive unlocked")
        : source === "open-file"
          ? tr("gui.archive.open_file_loaded", "Open-file archive loaded")
          : tr("gui.archive.open_loaded", "Open archive loaded"),
    );
    recordValidationRenderReady(`archive-open:${source}`);
  }

  function archiveOpenFailureNotice(path: string): string {
    const error = archiveOpenError(path);
    if (error?.key === "gui.error.corrupt.volume_missing") {
      return tError(error);
    }
    if (error?.key === "error.unsupported_split_wim") {
      return tError(error);
    }
    if (error?.key === "error.unsupported") {
      return tr("gui.archive.open_unsupported", "{name} uses a format or compression method that is not supported.")
        .replace("{name}", pathBaseName(path));
    }
    if (error?.key === "error.io") {
      return tr("gui.archive.open_io", "Could not access {name}. Check its location and file permissions.")
        .replace("{name}", pathBaseName(path));
    }
    if (error?.key === "error.resource_limit") {
      return tr("gui.archive.open_resource_limit", "{name} exceeds the current safety limits. Review Security settings before trying again.")
        .replace("{name}", pathBaseName(path));
    }
    return tr("gui.archive.open_failed", "Could not open {name}. Check the format, file access, and integrity.")
      .replace("{name}", pathBaseName(path));
  }

  function isPar2Path(path: string): boolean {
    return pathBaseName(path).toLowerCase().endsWith(".par2");
  }

  function preferredRecoverySidecar(paths: string[]): string | null {
    const sidecars = paths.filter(isPar2Path);
    return sidecars.find((path) => !/\.vol\d+\+\d+\.par2$/i.test(pathBaseName(path))) ?? sidecars[0] ?? null;
  }

  function recoverySidecarSetKey(path: string): string {
    const key = path.replace(/\.vol\d+\+\d+\.par2$/i, ".par2");
    return activePlatform === "windows" ? key.toLocaleLowerCase() : key;
  }

  function openRecoverySetFromPaths(paths: string[], source: "dialog" | "open-file"): boolean {
    const sidecar = preferredRecoverySidecar(paths);
    if (!sidecar) return false;
    const sidecars = paths.filter(isPar2Path);
    const sidecarCount = sidecars.length;
    const sidecarSetCount = new Set(sidecars.map(recoverySidecarSetKey)).size;
    const archivePaths = paths.filter((path) => !isPar2Path(path) && archiveLikePath(path));
    const archiveGroups = new Set(
      archiveVolumeFamilyKeys(archivePaths).map((key) =>
        activePlatform === "windows" ? key.toLocaleLowerCase() : key,
      ),
    );
    const archivePath = sidecarSetCount === 1 && archiveGroups.size <= 1
      ? archivePaths.find((path) => /\.001$/i.test(pathBaseName(path))) ?? archivePaths[0] ?? null
      : null;
    openRecoverySet(sidecar, archivePath, source, sidecarCount, sidecarSetCount);
    return true;
  }

  function openRecoverySet(
    sidecar: string,
    archivePath: string | null,
    source: "dialog" | "open-file",
    sidecarCount = 1,
    sidecarSetCount = 1,
  ) {
    archiveOpenGeneration += 1;
    archiveOpenStatus = "idle";
    cancelArchivePasswordPrompt();
    recoveryPar2Override = sidecar;
    recoverySourceOverride = archivePath;
    recoverySourceMode = archivePath ? "selected" : "none";
    setScreenRespectingJobQuestion("recovery");
    let notice: string;
    if (sidecarSetCount > 1) {
      notice = tr(
        "gui.recovery.multiple_sidecar_sets_choose",
        "{name} selected, but {count} PAR2 sets were provided. Choose the matching archive and PAR2 file.",
      )
        .replace("{name}", pathBaseName(sidecar))
        .replace("{count}", sidecarSetCount.toLocaleString());
    } else if (sidecarCount > 1) {
      const key = archivePath
        ? "gui.recovery.sidecar_set_loaded"
        : "gui.recovery.sidecar_set_loaded_choose_archive";
      const fallback = archivePath
        ? "{name} selected from {count} PAR2 files. Other recovery volumes will be found beside it."
        : "{name} selected from {count} PAR2 files. Choose the matching archive.";
      notice = tr(key, fallback)
        .replace("{name}", pathBaseName(sidecar))
        .replace("{count}", sidecarCount.toLocaleString());
    } else if (archivePath) {
      notice = tr("gui.recovery.set_loaded", "Recovery set loaded for {name}")
        .replace("{name}", pathBaseName(archivePath));
    } else {
      notice = tr("gui.recovery.par2_loaded_choose_archive", "{name} selected. Choose its archive or use the current archive.")
        .replace("{name}", pathBaseName(sidecar));
    }
    showNotice(notice);
    recordValidationRenderReady(`recovery-open:${source}`);
  }

  async function chooseRecoveryArchive() {
    if (recoveryPickerStatus !== "idle") return;
    recoveryPickerStatus = "archive";
    showNotice(tr("gui.recovery.opening_archive_picker", "Choose the archive to inspect or repair."));
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("recovery.choose-archive", open, {
        title: tr("gui.recovery.choose_archive", "Choose archive"),
        multiple: false,
        directory: false,
      });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (typeof path !== "string") {
        showNotice(tr("gui.recovery.archive_picker_cancelled", "Archive selection cancelled."));
        return;
      }
      recoverySourceMode = "selected";
      recoverySourceOverride = path;
      showNotice(
        tr("gui.recovery.archive_selected", "Recovery target: {name}").replace("{name}", pathBaseName(path)),
      );
    } catch {
      showNotice(tr("gui.recovery.archive_picker_unavailable", "Choosing a recovery target requires the desktop file dialog."));
    } finally {
      recoveryPickerStatus = "idle";
    }
  }

  async function chooseRecoveryPar2() {
    if (recoveryPickerStatus !== "idle") return;
    recoveryPickerStatus = "par2";
    showNotice(tr("gui.recovery.opening_par2_picker", "Choose a PAR2 recovery file."));
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("recovery.choose-par2", open, {
        title: tr("gui.recovery.choose_par2", "Choose PAR2 file"),
        multiple: false,
        directory: false,
        filters: [{ name: tr("gui.recovery.par2_files", "PAR2 recovery files"), extensions: ["par2"] }],
      });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (typeof path !== "string") {
        showNotice(tr("gui.recovery.par2_picker_cancelled", "PAR2 selection cancelled."));
        return;
      }
      recoveryPar2Override = path;
      showNotice(
        tr("gui.recovery.par2_selected", "PAR2 file: {name}").replace("{name}", pathBaseName(path)),
      );
    } catch {
      showNotice(tr("gui.recovery.par2_picker_unavailable", "Choosing PAR2 data requires the desktop file dialog."));
    } finally {
      recoveryPickerStatus = "idle";
    }
  }

  function useCurrentArchiveForRecovery() {
    if (!currentArchive) {
      showNotice(tr("gui.recovery.no_current_archive", "No archive is currently open."));
      return;
    }
    recoverySourceMode = "current";
    recoverySourceOverride = null;
    showNotice(
      tr("gui.recovery.using_current_archive", "Using {name} for recovery.").replace("{name}", currentArchive.name),
    );
  }

  function useDefaultPar2ForRecovery() {
    const source = recoverySourcePath();
    if (!source) {
      showNotice(tr("gui.recovery.choose_archive_before_default_par2", "Choose an archive before using its default PAR2 file."));
      return;
    }
    recoveryPar2Override = null;
    showNotice(
      tr("gui.recovery.using_default_par2", "Verify and Repair will look for {name} beside the archive.")
        .replace("{name}", pathBaseName(`${source}.par2`)),
    );
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
    const normalizedPath = row.path.replaceAll("\\", "/").replace(/\/+$/g, "");
    const parentBoundary = normalizedPath.lastIndexOf("/");
    const location = filterText().trim()
      ? parentBoundary < 0
        ? tr("gui.list.archive_root", "Archive root")
        : normalizedPath.slice(0, parentBoundary)
      : "";
    return {
      name: row.display,
      location,
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
      ? tr("gui.archive.names_review", "{count} names need review")
          .replace("{count}", currentArchive.garbled_count.toLocaleString())
      : currentArchive.legacy_encoding_count
        ? tr("gui.archive.legacy_names", "{count} legacy encoded names")
            .replace("{count}", currentArchive.legacy_encoding_count.toLocaleString())
        : tr("gui.archive.names_clean", "Names decoded cleanly");
    return tr("gui.archive.summary", "{count} entries · {format} · {diagnostics}")
      .replace("{count}", currentArchive.entry_count.toLocaleString())
      .replace("{format}", archiveFormat())
      .replace("{diagnostics}", diagnostics);
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
    if (count === 0) {
      return tr("gui.archive.encoding_clean", "No filename encoding issues detected.");
    }
    return tr("gui.archive.encoding_warning", "Encoding review needed for {count} names.")
      .replace("{count}", count.toLocaleString());
  }

  function archiveStructureWarningText(): string | null {
    if (currentArchive?.structure !== "zip_local_headers_recovered") return null;
    return tr(
      "gui.archive.zip_local_headers_recovered",
      "The ZIP index is missing or unreadable. The visible files came from local headers; test the archive and rebuild its index before relying on it.",
    );
  }

  function hasArchiveStructureWarning(): boolean {
    return archiveStructureWarningText() !== null;
  }

  function hasEncodingWarning(): boolean {
    return Boolean(currentArchive && (currentArchive.garbled_count > 0 || currentArchive.legacy_encoding_count > 0));
  }

  function isEntrySelected(entry: DisplayEntry): boolean {
    if (!currentArchive) return false;
    if (entry.source) return selectedPaths().has(entry.source.path);
    if (!import.meta.env.DEV) return false;
    return entry.name === "Launch plan.pdf" || entry.name === "screenshots" || entry.name === "财务报表.xlsx";
  }

  function entrySelectionLabel(entry: DisplayEntry): string {
    const name = entry.name || entry.source?.path || tr("gui.selection.entry", "entry");
    const key = isEntrySelected(entry) ? "gui.selection.deselect_entry" : "gui.selection.select_entry";
    const fallback = isEntrySelected(entry) ? "Deselect {name}" : "Select {name}";
    return tr(key, fallback).replace("{name}", name);
  }

  function entryPreviewForPath(entryPath: string): EntryPreviewDto | null {
    if (
      !currentArchive ||
      (entryPreview?.outer_path !== currentArchive.source && entryPreview?.outer_path !== currentArchive.path)
    ) {
      return null;
    }
    return entryPreview.entry_path === entryPath ? entryPreview : null;
  }

  function isEntryPreviewActive(entry: DisplayEntry): boolean {
    if (
      !entry.source ||
      previewOriginEntryPath !== entry.source.path ||
      (
        previewOriginVirtualIndex !== null &&
        entry.virtualIndex !== previewOriginVirtualIndex
      )
    ) {
      return false;
    }
    return Boolean(
      previewBusy() ||
      nestedPreview ||
      entryPreviewForPath(entry.source.path) ||
      entryPreviewFailure?.entryPath === entry.source.path,
    );
  }

  async function disposeEntryPreview(previewId: string): Promise<void> {
    await ipc.releasePreviewSession(previewId).catch(() => undefined);
  }

  async function prepareEntryPreviewSerially(
    archiveSource: string,
    archivePath: string | null,
    entryPath: string,
    requestGeneration: number,
  ): Promise<EntryPreviewDto | null> {
    const previous = entryPreviewPreparationTail;
    let releaseSlot: () => void = () => undefined;
    entryPreviewPreparationTail = new Promise<void>((resolve) => {
      releaseSlot = resolve;
    });
    await previous;
    try {
      if (requestGeneration !== previewRequestGeneration) return null;
      return (
        (archivePath
          ? previewSampleForEntry(archivePath, entryPath)
          : null) ??
        (await ipc.previewArchiveEntry(
          archiveSource,
          entryPath,
          null,
          archiveEncodingForJob(),
        ))
      );
    } finally {
      releaseSlot();
    }
  }

  function clearEntryPreviewState(restoreEntryFocus = false) {
    const preview = entryPreview;
    const originEntryPath = previewOriginEntryPath;
    const originVirtualIndex = previewOriginVirtualIndex;
    previewRequestGeneration += 1;
    previewActionGeneration += 1;
    nestedPreview = null;
    entryPreview = null;
    entryPreviewFailure = null;
    previewOriginEntryPath = null;
    previewOriginVirtualIndex = null;
    previewPhase = "idle";
    previewTargetName = "";
    if (preview) void disposeEntryPreview(preview.preview_id);
    if (restoreEntryFocus && originVirtualIndex !== null) {
      queueMicrotask(() => void focusArchiveRow(originVirtualIndex, originEntryPath));
    }
  }

  function selectOnlyEntry(entry: DisplayEntry) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    const preservePreview = entryPreviewForPath(entry.source.path) !== null;
    clearSelection();
    toggleSelect(entry.source);
    if (!preservePreview) clearEntryPreviewState();
  }

  function previewDisplayEntry(entry: DisplayEntry): void {
    if (!entry.source) return;
    void submitPreviewEntry(
      entry.source.path,
      entry.source.entry_type,
      entry.virtualIndex ?? null,
    );
  }

  function toggleEntrySelection(entry: DisplayEntry) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    toggleSelect(entry.source);
    if (!entryPreviewForPath(entry.source.path)) clearEntryPreviewState();
    recordValidationEvent("frontend.entry.selection_toggle", {
      path: entry.source.path,
      selected: selectedPaths().has(entry.source.path),
      selected_count: selectedPaths().size,
    });
  }

  function archiveSelectionControl() {
    const selected = selectedPaths();
    const total = totalRows();
    const checked = total > 0 && allCurrentRowsSelected();
    const mixed = selected.size > 0 && !checked;
    const busyLabel = archiveSelectionBusyReason();
    const label = busyLabel
      ? busyLabel
      : checked
      ? tr("gui.selection.clear", "Clear selection")
      : tr("gui.selection.select_all", "Select all entries");
    return {
      checked,
      mixed,
      disabled: total === 0 || filterPending() || archiveSelectAllProgress !== null,
      label,
      busy: Boolean(busyLabel),
      busyLabel,
    };
  }

  function selectAllProgressLabel(loaded: number, total: number): string {
    return tr("gui.selection.selecting", "Selecting {loaded} of {total}…")
      .replace("{loaded}", loaded.toLocaleString())
      .replace("{total}", total.toLocaleString());
  }

  function archiveSelectionBusyReason(): string {
    return archiveSelectAllProgress
      ? selectAllProgressLabel(archiveSelectAllProgress.loaded, archiveSelectAllProgress.total)
      : "";
  }

  function blockSelectionScopedAction(): boolean {
    const reason = archiveSelectionBusyReason();
    if (!reason) return false;
    showNotice(reason);
    return true;
  }

  async function selectAllArchiveEntries() {
    const control = archiveSelectionControl();
    if (control.disabled || control.checked) return;
    archiveSelectAllProgress = { loaded: 0, total: totalRows() };
    const updateProgress = (loaded: number, total: number) => {
      archiveSelectAllProgress = { loaded, total };
      showNotice(selectAllProgressLabel(loaded, total));
    };
    updateProgress(0, totalRows());
    const result = await selectAllRows(updateProgress);
    archiveSelectAllProgress = null;
    if (result === "failed") {
      showNotice(tr("gui.selection.select_all_failed", "Could not select every entry. Try again."));
      return;
    }
    if (result === "stale") {
      showNotice(tr("gui.selection.select_all_stale", "Selection changed before every entry could be selected."));
      return;
    }
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.selection_all", {
      selected_count: selectedPaths().size,
      total_count: totalRows(),
    });
    showNotice(tr("gui.selection.all_selected", "All entries selected"));
  }

  function toggleLoadedArchiveEntries() {
    const control = archiveSelectionControl();
    if (control.disabled) return;
    if (control.checked) {
      clearSelection();
      clearEntryPreviewState();
      recordValidationEvent("frontend.entry.selection_clear", {
        selected_count: 0,
      });
      showNotice(tr("gui.selection.cleared", "Selection cleared"));
      return;
    }
    void selectAllArchiveEntries();
  }

  function selectEntry(entry: DisplayEntry, event?: MouseEvent | KeyboardEvent) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    if (event?.metaKey || event?.ctrlKey) {
      toggleSelect(entry.source);
    } else {
      clearSelection();
      toggleSelect(entry.source);
    }
    if (!entryPreviewForPath(entry.source.path)) clearEntryPreviewState();
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
    if (entry.source.entry_type === "dir") {
      await openArchiveDirectoryEntry(entry.source.path);
      return;
    }
    if (archiveLikePath(entry.source.path) && currentArchive) {
      await openNestedArchiveEntry(
        currentArchive.source,
        entry.source.path,
        entry.virtualIndex ?? null,
      );
      return;
    }
    await submitPreviewEntry(entry.source.path, entry.source.entry_type, entry.virtualIndex ?? null);
  }

  function canGoUpArchive(): boolean {
    return Boolean(currentArchive && archiveDirs.length > 0);
  }

  async function goArchiveUp() {
    if (!canGoUpArchive()) return;
    const targetDirectory = archiveDirs.slice(0, -1).join("/");
    clearEntryPreviewState();
    clearSelection();
    await goUp();
    if (archiveBrowseError() || archiveDirs.join("/") !== targetDirectory) return;
    browseScrollTop = 0;
    recordValidationEvent("frontend.entry.go_up", {
      path: archiveDirs.join("/"),
    });
    showNotice(tr("gui.nav.opened_parent_folder", "Opened parent folder"));
  }

  async function openArchiveBreadcrumb(level: number) {
    if (!currentArchive) return;
    const targetDirectory = archiveDirs.slice(0, level + 1).join("/");
    clearEntryPreviewState();
    clearSelection();
    await gotoBreadcrumb(level);
    if (archiveBrowseError() || archiveDirs.join("/") !== targetDirectory) return;
    browseScrollTop = 0;
    recordValidationEvent("frontend.entry.breadcrumb", {
      level,
      path: archiveDirs.slice(0, level + 1).join("/"),
    });
  }

  function showEntryContextAt(x: number, y: number, entry: DisplayEntry) {
    if (
      !archiveSelectionBusyReason()
      && entry.source
      && !selectedPaths().has(entry.source.path)
    ) {
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
      openExtractWorkspace("selection");
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
    if (event.target !== event.currentTarget) return;
    if (
      archiveSelectionBusyReason()
      && ["Delete", "Backspace", "e", "E", "m", "M"].includes(event.key)
    ) {
      event.preventDefault();
      showNotice(archiveSelectionBusyReason());
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      void activateEntry(entry);
    } else if (event.key === " ") {
      event.preventDefault();
      if (!event.repeat) {
        previewDisplayEntry(entry);
      }
    } else if (event.key === "Backspace" && selectedPaths().size === 0) {
      event.preventDefault();
      void goArchiveUp();
    } else if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      void submitDeleteSelectedJob();
    } else if ((event.metaKey || event.altKey) && event.key === "ArrowUp") {
      event.preventDefault();
      void goArchiveUp();
    } else if (event.key === "e" || event.key === "E") {
      event.preventDefault();
      if (entry.source && !selectedPaths().has(entry.source.path)) {
        selectOnlyEntry(entry);
      }
      openExtractWorkspace("selection");
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
    const count = selectedPaths().size;
    if (!currentArchive || count === 0) {
      return tr("gui.selection.none", "No entries selected");
    }
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
    return archiveNameWithoutVolumeSuffix(name);
  }

  function archiveExtensionMatch(name: string): string | null {
    if (isNativeSplitZipVolumeName(name)) return "zip";
    const lower = archivePathWithoutSplitSuffix(name).toLowerCase().trimEnd();
    if (isLegacyRarVolumeName(lower)) return "rar";
    return registryFormatExtensions().find((extension) => lower.endsWith(`.${extension}`)) ?? null;
  }

  function archiveStemName(name: string = currentArchive?.name ?? "archive"): string {
    if (isNativeSplitZipVolumeName(name)) {
      return archivePathWithoutSplitSuffix(name);
    }
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
        entries: isCurrent
          ? currentArchive.entry_count.toLocaleString()
          : tr("gui.batch.state.pending", "Pending"),
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

  function appendCreateSources(
    paths: readonly string[],
    kind: CreateSourceKind,
  ): { added: number; total: number } {
    const previousCount = createSources.length;
    createSources = mergeCreateSources(
      createSources,
      paths.map((path) => ({ path, kind })),
      platformKind(),
    );
    return {
      added: createSources.length - previousCount,
      total: createSources.length,
    };
  }

  function showCreateSourcesAdded(result: { added: number; total: number }): void {
    showNotice(
      result.added > 0
        ? tr("gui.create.sources.added", "{added} items added · {total} total")
          .replace("{added}", result.added.toLocaleString())
          .replace("{total}", result.total.toLocaleString())
        : tr("gui.create.sources.already_added", "Those items are already in the source list"),
    );
  }

  function createSourceSelected(path: string): boolean {
    return includesCreateSourcePath(selectedCreateSourcePaths, path, platformKind());
  }

  function createSourceSelectedCount(): number {
    return createSources.filter((source) => createSourceSelected(source.path)).length;
  }

  function createSourceAllSelected(): boolean {
    return createSources.length > 0 && createSourceSelectedCount() === createSources.length;
  }

  function setAllCreateSourcesSelected(selected: boolean): void {
    if (createSourcesLocked()) return;
    selectedCreateSourcePaths = selected ? [...createSourceInputs] : [];
  }

  function toggleCreateSourceSelection(path: string): void {
    if (createSourcesLocked()) return;
    selectedCreateSourcePaths = toggleCreateSourcePath(
      selectedCreateSourcePaths,
      path,
      platformKind(),
    );
  }

  function clearCreateSourceSelection(): void {
    if (createSourcesLocked()) return;
    selectedCreateSourcePaths = [];
  }

  function removeCreateSourcePaths(paths: readonly string[]): number {
    if (createSourcesLocked() || paths.length === 0) return 0;
    const previousCount = createSources.length;
    createSources = removeCreateSourcesByPaths(createSources, paths, platformKind());
    selectedCreateSourcePaths = selectedCreateSourcePaths.filter(
      (selected) => !includesCreateSourcePath(paths, selected, platformKind()),
    );
    return previousCount - createSources.length;
  }

  function removeCreateSource(path: string): void {
    if (removeCreateSourcePaths([path]) === 0) return;
    showNotice(
      tr("gui.create.sources.removed_one", "{name} removed from the source list")
        .replace("{name}", desktopBasename(path, platformKind())),
    );
  }

  function removeSelectedCreateSources(): void {
    const removed = removeCreateSourcePaths(selectedCreateSourcePaths);
    if (removed === 0) return;
    showNotice(
      tr("gui.create.sources.removed_many", "{count} items removed from the source list")
        .replace("{count}", removed.toLocaleString()),
    );
  }

  function clearCreateSources(): void {
    createSources = [];
    selectedCreateSourcePaths = [];
  }

  function createSourceKindLabel(kind: CreateSourceKind): string {
    if (kind === "file") return tr("gui.create.sources.kind_file", "File");
    if (kind === "folder") return tr("gui.create.sources.kind_folder", "Folder");
    return tr("gui.create.sources.kind_item", "File or folder");
  }

  function createSourceCountLabel(): string {
    return tr("gui.create.sources.count", "{count} items")
      .replace("{count}", createSources.length.toLocaleString());
  }

  function createSourceSelectionLabel(): string {
    const count = createSourceSelectedCount();
    return count === 0
      ? tr("gui.create.sources.none_selected", "None selected")
      : tr("gui.create.sources.selected_count", "{count} selected")
        .replace("{count}", count.toLocaleString());
  }

  async function submitCreateSourceList(): Promise<void> {
    if (createSourceInputs.length === 0) {
      showNotice(tr("gui.create.no_source_items", "No source items selected"));
      return;
    }
    await submitCreateInputs([...createSourceInputs], "dialog");
  }

  function dropStatusLabel(): string {
    if (dragActive) return tr("gui.drop.active", "Drop archives to open, or files and folders to create an archive");
    if (lastDropKind === "archives") {
      return tr("gui.drop.archives_ready", "{count} dropped archives ready").replace("{count}", String(batchArchivePaths.length));
    }
    if (lastDropKind === "create") {
      return tr("gui.drop.create_ready", "{count} dropped items ready to archive").replace("{count}", String(createSources.length));
    }
    if (lastDropKind === "recovery") {
      return tr("gui.drop.recovery_ready", "Recovery files ready");
    }
    return "";
  }

  function uniqueNonEmptyPaths(paths: string[]): string[] {
    return createSourcePaths(mergeCreateSources(
      [],
      paths.map((path) => ({ path, kind: "unknown" })),
      platformKind(),
    ));
  }

  function pathsFromDomDrop(event: DragEvent): string[] {
    const transfer = event.dataTransfer;
    if (!transfer) return [];
    const uriList = transfer.getData("text/uri-list");
    const textList = transfer.getData("text/plain");
    const fromText = (uriList || textList)
      .split(/\r?\n/)
      .filter((line) => line.length > 0 && !line.startsWith("#"))
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
    const deadline = performance.now() + 5_000;
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
    const waitForContent = () => {
      if (
        document.querySelector('.deferred-workspace-state[aria-busy="true"]') &&
        performance.now() < deadline
      ) {
        setTimeout(waitForContent, 16);
        return;
      }
      emit();
    };
    void tick().then(() => setTimeout(waitForContent, 0));
  }

  async function handleDroppedPaths(paths: string[], source: "native" | "dom" | "preview" | "validation") {
    const dropped = uniqueNonEmptyPaths(paths);
    if (dropped.length === 0) return;
    if (modeSelectionBlocked) {
      dragActive = false;
      if (firstRunRequired) {
        firstRunDropFeedback = tr("gui.first_run.choose_before_drop", "Choose an interface mode before opening or creating archives");
      } else {
        showNotice(tr("gui.first_run.wait_before_drop", "Wait while Squallz checks saved settings before opening or creating archives"));
      }
      return;
    }
    if (createSourcePickerBusy) {
      showNotice(tr("gui.create.sources.finish_picker_before_drop", "Close the source picker before dropping other items"));
      return;
    }
    if (createPreflightBusy()) {
      showNotice(tr("gui.create.finish_preflight_before_drop", "Wait for create preflight to finish before dropping other items"));
      return;
    }
    if (pendingCreateSubmission) {
      showNotice(tr("gui.create.finish_review_before_drop", "Confirm or cancel the current create plan before dropping other items"));
      return;
    }
    if (screen === "create") {
      const result = appendCreateSources(dropped, "unknown");
      lastDropKind = "create";
      recordValidationEvent("frontend.drop", {
        source,
        route: "create",
        paths: dropped,
        archive_count: dropped.filter(archiveLikePath).length,
        create_count: dropped.length,
      });
      if (source !== "preview" && source !== "validation") {
        showCreateSourcesAdded(result);
      }
      return;
    }
    if (preferredRecoverySidecar(dropped)) {
      recordValidationEvent("frontend.drop", {
        source,
        route: "recovery",
        paths: dropped,
        archive_count: dropped.filter((path) => !isPar2Path(path) && archiveLikePath(path)).length,
        recovery_count: dropped.filter(isPar2Path).length,
      });
      lastDropKind = "recovery";
      openRecoverySetFromPaths(dropped, "open-file");
      return;
    }
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
    const result = appendCreateSources(dropped, "unknown");
    recordValidationEvent("frontend.drop", {
      source,
      route: "create",
      paths: dropped,
      archive_count: archivePaths.length,
      create_count: createInputs.length,
    });
    setScreen("create");
    if (source === "native" && createPostSuccess === "trash_source") {
      showNotice(
        tr(
          "gui.create.output.trash_drop_confirmation",
          "Review the output settings, then confirm creation before originals can move to {trash}.",
        ).replace("{trash}", trashNameLabel()),
      );
    } else if (source !== "preview" && source !== "validation") {
      showCreateSourcesAdded(result);
    }
  }

  function extractDestInDefaultFolder(fallbackParent: string, archiveName: string): string {
    const parent = normalizedDefaultExtractDir(appliedDefaultExtractDir) ?? fallbackParent;
    const name = archiveStemName(archiveName);
    if (parent === "/") return `/${name}`;
    return `${parent}/${name}`;
  }

  function defaultExtractDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return extractDestInDefaultFolder(pathDir(currentArchive.path), currentArchive.name);
  }

  function extractJobDestination(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (extractDestinationMode !== "smart") return effectiveExtractDest();
    return normalizedDefaultExtractDir(appliedDefaultExtractDir) ?? pathDir(currentArchive.path);
  }

  function sameFolderExtractDest(): string {
    return currentArchive ? pathDir(currentArchive.path) : openArchiveFirstLabel();
  }

  function chosenExtractDest(): string {
    return extractCustomDest.trim() || tr("gui.extract.pick_another_folder", "Pick another folder");
  }

  function previewEntryMatchesSelection(entry: EntryDto, selection: string[] | null): boolean {
    if (!selection) return true;
    const entryPath = entry.path.replaceAll("\\", "/");
    return selection.some((selected) => {
      const normalized = selected.replaceAll("\\", "/");
      const prefix = normalized.endsWith("/") ? normalized : `${normalized}/`;
      return entryPath === normalized || entryPath.startsWith(prefix);
    });
  }

  function previewExtractWraps(entries: EntryDto[]): boolean {
    let root: string | null = null;
    let rootIsDirectory = false;
    for (const entry of entries) {
      const components = entry.path
        .replaceAll("\\", "/")
        .split("/")
        .filter((component) => component && component !== ".");
      const first = components[0];
      if (!first) continue;
      if (root !== null && root !== first) return true;
      root = first;
      if (components.length === 1) {
        if (entry.entry_type !== "dir") return true;
        rootIsDirectory = true;
      } else {
        rootIsDirectory = true;
      }
    }
    return root !== null && !rootIsDirectory;
  }

  function previewExtractPlan(
    dest: string,
    selection: string[] | null,
    smart: boolean,
  ): ExtractPlanPreflightDto | null {
    const preview = runtimePreviews.archive;
    if (!preview || !currentArchive) return null;
    const allEntries = preview.previewRows ?? preview.rows;
    const selectedEntries = allEntries.filter((entry) => previewEntryMatchesSelection(entry, selection));
    const layout = smart && previewExtractWraps(allEntries) ? "wrap_in_folder" : "direct";
    const destination = layout === "wrap_in_folder"
      ? joinDesktopPath(dest, archiveStemName(currentArchive.name), initialPlatform)
      : dest;
    const count = (type: EntryDto["entry_type"]) => selectedEntries.filter((entry) => entry.entry_type === type).length;
    const files = count("file");
    const totalBytes = selectedEntries.reduce(
      (total, entry) => total + (entry.entry_type === "file" ? entry.size : 0),
      0,
    );
    const requiredFreeBytes = Math.min(
      Number.MAX_SAFE_INTEGER,
      totalBytes + selectedEntries.length * 4096,
    );
    return {
      requested_destination: dest,
      destination,
      layout,
      entries: selectedEntries.length,
      files,
      directories: count("dir"),
      symlinks: count("symlink"),
      hardlinks: count("hardlink"),
      other: count("other"),
      total_bytes: totalBytes,
      estimated_conflicts: 0,
      input_guard: "",
      required_free_bytes: requiredFreeBytes,
      available_bytes: runtimePreviews.extractAvailableBytes,
      space_ok: runtimePreviews.extractAvailableBytes >= requiredFreeBytes,
    };
  }

  function clearExtractPlanDebounce(): void {
    if (extractPlanDebounceTimer === null) return;
    clearTimeout(extractPlanDebounceTimer);
    extractPlanDebounceTimer = null;
  }

  function discardQueuedExtractPlan(): void {
    clearExtractPlanDebounce();
    const queued = extractPlanQueued;
    extractPlanQueued = null;
    queued?.resolve();
  }

  function cancelActiveExtractPlan(): void {
    const active = extractPlanActive;
    if (!active || active.control.cancelRequested) return;
    active.control.cancelRequested = true;
    void ipc.cancelExtractPlan(active.requestId).catch(() => {
      // A stale read-only plan may already have completed.
    });
  }

  function resetExtractPlanRequestState(): void {
    cancelActiveExtractPlan();
    extractPlanGeneration += 1;
    extractPlanRequestKey = "";
    discardQueuedExtractPlan();
    extractPlan = null;
    extractPlanPhase = "idle";
    extractPlanErrorKey = "";
  }

  async function refreshExtractPlan(request: ExtractPlanRequest): Promise<void> {
    try {
      const preview = previewExtractPlan(request.dest, request.selection, request.smart);
      const plan = preview ?? await ipc.planExtract(
        request.path,
        request.displayPath,
        request.dest,
        request.selection,
        request.smart,
        request.encoding,
        request.requestId,
      );
      if (
        request.generation !== extractPlanGeneration ||
        request.key !== extractPlanRequestKey
      ) return;
      extractPlan = plan;
      extractPlanPhase = plan.space_ok ? "ready" : "blocked";
    } catch {
      if (
        request.generation !== extractPlanGeneration ||
        request.key !== extractPlanRequestKey
      ) return;
      extractPlan = null;
      extractPlanPhase = "error";
      extractPlanErrorKey = "gui.extract.plan_unavailable_body";
    }
  }

  function startQueuedExtractPlan(): void {
    if (extractPlanActive || extractPlanDebounceTimer !== null || !extractPlanQueued) return;
    const request = extractPlanQueued;
    extractPlanQueued = null;
    extractPlanActive = request;
    void refreshExtractPlan(request).finally(() => {
      request.resolve();
      if (extractPlanActive !== request) return;
      extractPlanActive = null;
      startQueuedExtractPlan();
    });
  }

  function queueExtractPlanRequest(request: ExtractPlanRequest, debounce: boolean): void {
    discardQueuedExtractPlan();
    extractPlanQueued = request;
    if (!debounce) {
      startQueuedExtractPlan();
      return;
    }
    extractPlanDebounceTimer = setTimeout(() => {
      extractPlanDebounceTimer = null;
      startQueuedExtractPlan();
    }, extractPlanDebounceMs);
  }

  function requestExtractPlan(
    archiveId: number,
    path: string,
    displayPath: string,
    dest: string,
    selection: string[] | null,
    smart: boolean,
    encoding: string | null,
    debounce = false,
  ): Promise<void> {
    const key = extractPlanKey(
      archiveId,
      path,
      displayPath,
      dest,
      selection,
      smart,
      encoding,
    );
    if (
      key === extractPlanRequestKey &&
      (extractPlanPhase === "ready" || extractPlanPhase === "blocked") &&
      extractPlan
    ) {
      return Promise.resolve();
    }
    if (
      key === extractPlanRequestKey &&
      extractPlanActive?.key === key &&
      extractPlanActive.generation === extractPlanGeneration
    ) return extractPlanActive.promise;
    const queued = extractPlanQueued;
    if (
      key === extractPlanRequestKey &&
      queued?.key === key &&
      queued.generation === extractPlanGeneration
    ) {
      if (!debounce && extractPlanDebounceTimer !== null) {
        clearExtractPlanDebounce();
        startQueuedExtractPlan();
      }
      return queued.promise;
    }

    cancelActiveExtractPlan();
    const generation = ++extractPlanGeneration;
    extractPlanRequestKey = key;
    extractPlan = null;
    extractPlanPhase = "loading";
    extractPlanErrorKey = "";
    let resolveRequest: () => void = () => undefined;
    const promise = new Promise<void>((resolve) => {
      resolveRequest = resolve;
    });
    const request: ExtractPlanRequest = {
      key,
      generation,
      requestId: nextPreflightRequestId(),
      path,
      displayPath,
      dest,
      selection,
      smart,
      encoding,
      promise,
      resolve: resolveRequest,
      control: { cancelRequested: false },
    };
    queueExtractPlanRequest(request, debounce);
    return promise;
  }

  function extractPlanKey(
    archiveId: number,
    path: string,
    displayPath: string,
    dest: string,
    selection: string[] | null,
    smart: boolean,
    encoding: string | null,
  ): string {
    return JSON.stringify([
      archiveId,
      path,
      displayPath,
      dest,
      selection,
      smart,
      encoding,
    ]);
  }

  function extractPlanStatusLabel(): string {
    if (extractPlanPhase === "ready") return tr("gui.extract.plan_ready", "Ready");
    if (extractPlanPhase === "blocked") return tr("gui.error.disk_full.title", "Not Enough Disk Space");
    if (extractPlanPhase === "error") return tr("gui.extract.plan_unavailable", "Preview unavailable");
    if (extractPlanPhase === "loading") return tr("gui.extract.plan_checking", "Checking");
    return tr("gui.extract.plan_waiting", "Waiting");
  }

  function extractPlanDescription(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (extractPlanPhase === "blocked") return extractSpaceFailureLabel();
    return extractPlanPhase === "ready"
      ? tr(
          "gui.extract.plan_ready_body",
          "The selected scope, smart layout, and destination below come from the same core plan the task will rebuild before extraction.",
        )
      : tr(
          "gui.extract.plan_checking_body",
          "Reading the archive layout and checking the current destination without writing files.",
        );
  }

  function extractPlanErrorLabel(): string {
    if (!extractPlanErrorKey) return "";
    return tr(
      extractPlanErrorKey,
      "Squallz could not refresh this preview. Check the archive and destination, then try again.",
    );
  }

  function extractSpaceFailureLabel(): string {
    const plan = extractPlan;
    if (!plan) return tr("error.disk_full", "Disk is full");
    return tr(
      "gui.error.disk_full.body",
      "{required} needed, {available} available. Free up space and retry.",
    )
      .replace("{required}", formatBytes(plan.required_free_bytes))
      .replace("{available}", formatBytes(plan.available_bytes));
  }

  function retryExtractPlan(): Promise<void> {
    const current = currentArchive;
    if (!current || screen !== "extract") return Promise.resolve();
    extractPlanRequestKey = "";
    return requestExtractPlan(
      current.id,
      current.source,
      current.path,
      extractJobDestination(),
      extractJobPaths(),
      extractDestinationMode === "smart",
      extractEncodingForJob(),
    );
  }

  function extractPlanLayoutLabel(): string {
    if (!extractPlan) return tr("gui.extract.layout_direct", "Direct");
    return extractPlan.layout === "wrap_in_folder"
      ? tr("gui.extract.layout_wrapped", "Containing folder")
      : tr("gui.extract.layout_direct", "Direct");
  }

  function extractPlanMetrics() {
    const plan = extractPlan;
    if (!plan) return [];
    const linkCount = plan.symlinks + plan.hardlinks;
    const metrics: ExtractWorkspaceSurface["plan"]["metrics"] = [
      {
        id: "scope",
        label: tr("gui.extract.plan_scope", "Scope"),
        value: tr("gui.extract.plan_scope_value", "{entries} entries · {files} files · {folders} folders · {links} links · {other} other")
          .replace("{entries}", plan.entries.toLocaleString())
          .replace("{files}", plan.files.toLocaleString())
          .replace("{folders}", plan.directories.toLocaleString())
          .replace("{links}", linkCount.toLocaleString())
          .replace("{other}", plan.other.toLocaleString()),
      },
      {
        id: "size",
        label: tr("gui.extract.plan_size", "Selected file data"),
        value: formatBytes(plan.total_bytes),
      },
    ];
    if (plan.entries > 0) {
      metrics.push(
        {
          id: "required-space",
          label: tr("common.required", "Required"),
          value: formatBytes(plan.required_free_bytes),
        },
        {
          id: "available-space",
          label: tr("common.available", "Available"),
          value: formatBytes(plan.available_bytes),
          tone: plan.space_ok ? "default" as const : "warning" as const,
        },
      );
    }
    metrics.push(
      {
        id: "layout",
        label: tr("gui.extract.plan_layout", "Layout"),
        value: extractPlanLayoutLabel(),
      },
      {
        id: "conflicts",
        label: tr("gui.extract.plan_conflicts", "Estimated conflicts"),
        value: plan.estimated_conflicts.toLocaleString(),
        tone: plan.estimated_conflicts > 0 ? "warning" as const : "default" as const,
      },
    );
    return metrics;
  }

  function effectiveExtractDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (extractDestinationMode === "smart") {
      if (
        (extractPlanPhase === "ready" || extractPlanPhase === "blocked") &&
        extractPlan
      ) return extractPlan.destination;
      return tr("gui.extract.smart_destination_value", "{base} · final folder chosen from archive contents")
        .replace("{base}", extractJobDestination());
    }
    if (extractDestinationMode === "same") return sameFolderExtractDest();
    if (extractDestinationMode === "choose") return extractCustomDest.trim() || defaultExtractDest();
    return defaultExtractDest();
  }

  function extractDestinationFieldLabel(): string {
    return extractDestinationMode === "smart"
      ? tr("gui.extract.smart_base", "Smart base")
      : tr("gui.extract.final_destination", "Final destination");
  }

  function extractDestinationTitle(mode: ExtractDestinationMode): string {
    if (mode === "archive") return tr("gui.extract.archive_folder", "Archive folder");
    if (mode === "same") return tr("gui.extract.same_folder", "Same folder");
    if (mode === "choose") return tr("gui.extract.choose", "Choose");
    return tr("gui.extract.smart_folder", "Smart folder");
  }

  function extractDestinationDetail(mode: ExtractDestinationMode): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (mode === "same") return sameFolderExtractDest();
    if (mode === "choose") return chosenExtractDest();
    if (mode === "smart") {
      const base = normalizedDefaultExtractDir(appliedDefaultExtractDir) ?? pathDir(currentArchive.path);
      return tr("gui.extract.smart_destination_value", "{base} · final folder chosen from archive contents")
        .replace("{base}", base);
    }
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
    markExtractPresetDraftTouched();
    extractDestinationMode = mode;
  }

  function extractWorkspaceSurface(variant: ExtractWorkspaceVariant): ExtractWorkspaceSurface {
    const archiveRequiredReason = extractArchiveRequiredReason();
    const startBlockedReason = extractStartBlockedReason();
    const startLabel = variant === "modern"
      ? extractActionLabel()
      : tr("gui.extract.start", "Extract");
    const batchLabel = variant === "modern"
      ? tr("gui.extract.review_batch", "Review batch extract")
      : tr("gui.extract.batch_review", "Batch review");
    const selectedPreset = selectedExtractArchivePreset();

    return {
      tr,
      title: variant === "modern"
        ? extractSafeTitle()
        : classicCommandLabel("Extract To"),
      start: {
        label: startLabel,
        disabled: Boolean(startBlockedReason),
        title: startBlockedReason || extractDestinationHint(),
        ariaLabel: labelWithDisabledReason(startLabel, startBlockedReason),
        onSelect: () => void submitExtractJob(),
      },
      batch: {
        label: batchLabel,
        disabled: Boolean(archiveRequiredReason),
        title: archiveRequiredReason,
        ariaLabel: labelWithDisabledReason(batchLabel, archiveRequiredReason),
        onSelect: () => setScreen("batch"),
      },
      destination: {
        label: extractDestinationFieldLabel(),
        path: effectiveExtractDest(),
        choices: extractDestinationModes.map((mode) => ({
          id: mode,
          label: extractDestinationTitle(mode),
          detail: extractDestinationDetail(mode),
          selected: extractDestinationMode === mode,
          disabled: Boolean(archiveRequiredReason),
          title: archiveRequiredReason,
          ariaLabel: labelWithDisabledReason(extractDestinationTitle(mode), archiveRequiredReason),
          onSelect: () => void selectExtractDestination(mode),
        })),
      },
      archive: {
        title: archiveTitle(),
        line: archiveLine(),
        selection: extractSelectionLabel(),
        password: extractPasswordLabel(),
      },
      plan: {
        variant,
        phase: extractPlanPhase,
        ariaLabel: tr("gui.extract.plan_aria", "Extraction write plan"),
        eyebrow: tr("gui.extract.plan_eyebrow", "Before extraction"),
        heading: tr("gui.extract.plan_heading", "Know what will be written"),
        statusLabel: extractPlanStatusLabel(),
        description: extractPlanDescription(),
        destinationLabel: tr("gui.extract.plan_destination", "Planned destination"),
        destination: extractPlan?.destination ?? extractJobDestination(),
        metrics: extractPlanMetrics(),
        note: tr("gui.extract.plan_snapshot_note", "Required space includes selected data and filesystem allocation allowance. Space and conflicts are checked again immediately before writing."),
        error: extractPlanErrorLabel(),
        retryLabel: extractPlanPhase === "blocked"
          ? tr("gui.extract.plan_recheck_space", "Check space again")
          : tr("gui.extract.plan_retry", "Retry preview"),
        onRetry: () => void retryExtractPlan(),
      },
      preset: {
        instanceId: `${variant}-extract`,
        variant,
        compact: true,
        kind: "extract",
        options: extractPresetPickerOptions(),
        selectedId: selectedExtractPresetId,
        draftName: extractPresetDraftName,
        summary: extractArchivePresetSummary(currentExtractArchivePresetOptions()),
        status: extractPresetStatus(),
        statusLabel: archivePresetStatusLabel(extractPresetStatus()),
        disabledReason: archivePresetPickerDisabledReason("extract"),
        deleteDisabledReason: presetDeleteDisabledReason(selectedPreset),
        isDefault: presetDocument?.bindings.app_default_extract === selectedExtractPresetId,
        isFileManagerDefault: presetDocument?.bindings.file_manager_extract === selectedExtractPresetId,
        tr,
        onSelect: (id) => applyExtractPreset(id),
        onDraftNameInput: (name) => (extractPresetDraftName = name),
        onUpdate: () => void updateSelectedArchivePreset("extract"),
        onSaveAs: () => void saveCurrentExtractPresetAsNew(),
        onDelete: () => void deleteSelectedArchivePreset("extract"),
        onDefaultChange: (enabled) => void setArchivePresetBinding("extract", "app", enabled),
        onFileManagerDefaultChange: (enabled) => void setArchivePresetBinding("extract", "file_manager", enabled),
      },
      overwrite: {
        label: currentExtractOverwriteLabel,
        choices: extractOverwriteModes.map((mode) => ({
          id: mode,
          label: extractOverwriteLabel(mode),
          detail: "",
          selected: extractOverwriteMode === mode,
          disabled: false,
          title: "",
          ariaLabel: extractOverwriteLabel(mode),
          onSelect: () => selectExtractOverwrite(mode),
        })),
      },
      symlink: {
        label: currentExtractSymlinkLabel,
        choices: extractSymlinkModes.map((mode) => ({
          id: mode,
          label: extractSymlinkLabel(mode),
          detail: "",
          selected: extractSymlinkMode === mode,
          disabled: false,
          title: "",
          ariaLabel: extractSymlinkLabel(mode),
          onSelect: () => selectExtractSymlink(mode),
        })),
      },
      encoding: {
        label: extractEncodingLabel(),
        detail: currentArchive ? archiveWarningText() : openArchiveFirstLabel(),
      },
      test: {
        disabled: Boolean(archiveRequiredReason),
        title: archiveRequiredReason,
        ariaLabel: labelWithDisabledReason(
          tr("gui.extract.test_first", "Test first"),
          archiveRequiredReason,
        ),
        onSelect: () => void submitTestJob(),
      },
    };
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
        markExtractPresetDraftTouched();
        extractCustomDest = selected;
        extractDestinationMode = "choose";
        showNotice(tr("gui.extract.destination_selected", "Extract destination selected"));
      }
    } catch {
      showNotice(tr("gui.extract.destination_picker_requires_desktop_service", "Destination picker requires the desktop service"));
    }
  }

  function extractOverwriteLabel(mode: ExtractOverwriteMode): string {
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
    markExtractPresetDraftTouched();
    extractOverwriteMode = mode;
  }

  function extractSymlinkLabel(mode: ExtractSymlinkMode): string {
    if (mode === "skip") return tr("gui.extract.symlink.skip", "Skip links");
    if (mode === "follow") return tr("gui.extract.symlink.follow", "Follow safe links");
    return tr("gui.extract.symlink.preserve", "Preserve links");
  }

  function selectExtractSymlink(mode: ExtractSymlinkMode) {
    if (!currentArchive) {
      showNotice(openArchiveFirstLabel());
      return;
    }
    markExtractPresetDraftTouched();
    extractSymlinkMode = mode;
  }

  function extractDestForPath(path: string): string {
    return extractDestInDefaultFolder(pathDir(path), pathBaseName(path));
  }

  function nestedExtractDest(outerDisplayPath: string, entryPath: string): string {
    return extractDestInDefaultFolder(pathDir(outerDisplayPath), pathBaseName(entryPath));
  }

  function defaultSqzExportDest(): string {
    const source = recoverySourcePath();
    if (!source) return openArchiveFirstLabel();
    return `${pathDir(source)}/${archiveStemName(pathBaseName(source))}.zip`;
  }

  function defaultSqzRepairDest(): string {
    const source = recoverySourcePath();
    if (!source) return openArchiveFirstLabel();
    return `${pathDir(source)}/${archiveStemName(pathBaseName(source))}.repaired.sqz`;
  }

  function defaultZipRepairDest(): string {
    const source = recoverySourcePath();
    if (!source) return openArchiveFirstLabel();
    return `${pathDir(source)}/${archiveStemName(pathBaseName(source))}.rebuilt.zip`;
  }

  function recoverySourcePath(): string | null {
    if (recoverySourceMode === "current") return currentArchive?.path ?? null;
    if (recoverySourceMode === "selected") return recoverySourceOverride;
    return null;
  }

  function recoverySourceForJob(): string | null {
    const source = recoverySourcePath();
    return recoverySourceMatchesCurrentArchive() ? currentArchive?.source ?? null : source;
  }

  function recoverySourceName(): string | null {
    const source = recoverySourcePath();
    return source ? pathBaseName(source) : null;
  }

  function defaultRecoveryPath(): string | null {
    const source = recoverySourcePath();
    return source ? `${source}.par2` : null;
  }

  function recoveryPar2Path(): string | null {
    return recoveryPar2Override ?? defaultRecoveryPath();
  }

  function recoverySourceMatchesCurrentArchive(): boolean {
    const source = recoverySourcePath();
    return Boolean(
      recoverySourceMode === "current" &&
      source &&
      currentArchive &&
      sameFilePath(source, currentArchive.path),
    );
  }

  function recoverySourceIsSplit(): boolean {
    const source = recoverySourcePath();
    return Boolean(source && /\.\d{3,}$/i.test(pathBaseName(source)));
  }

  function recoveryRepairUsesDirectory(): boolean {
    const reportedCount = recoveryReportNumber("source_file_count");
    if (reportedCount !== null) return reportedCount > 1;
    return recoverySourceIsSplit()
      || (recoverySourceMatchesCurrentArchive() && (currentArchive?.volumes?.length ?? 0) > 1);
  }

  function recoverySourceFormatId(): string | null {
    const source = recoverySourcePath();
    if (!source) return null;
    if (recoverySourceMatchesCurrentArchive()) return currentArchive?.format.toLowerCase() ?? null;
    return archiveExtensionMatch(pathBaseName(source));
  }

  function isRecoverySourceZipFamily(): boolean {
    const format = recoverySourceFormatId();
    return Boolean(format && ["zip", "jar", "apk", "cbz", "ipa"].includes(format));
  }

  function isRecoverySourceSqz(): boolean {
    return recoverySourceFormatId() === "sqz";
  }

  function defaultPar2RepairDest(): string {
    const source = recoverySourcePath();
    if (!source) return openArchiveFirstLabel();
    const sourceName = pathBaseName(source);
    const extension = archiveExtensionMatch(sourceName);
    const suffix = extension ? `.${extension}` : "";
    return `${pathDir(source)}/${archiveStemName(sourceName)}.repaired${suffix}`;
  }

  function defaultPar2RepairDirectoryName(): string {
    const source = recoverySourcePath();
    if (!source) return tr("gui.recovery.repaired_set_folder_name", "repaired-set");
    return `${archiveStemName(pathBaseName(source))}.repaired`;
  }

  function sameFilePath(left: string, right: string): boolean {
    return sameDesktopPath(left, right, platformKind());
  }

  function labelWithDisabledReason(label: string, reason: string): string {
    return reason ? `${label} · ${reason}` : label;
  }

  function recoveryZipDisabledReason(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.choose_archive_before_zip_rebuild", "Choose a ZIP-family archive before rebuilding its index.");
    if (recoverySourceIsSplit()) return tr("gui.recovery.zip_rebuild_no_split", "ZIP index rebuild cannot write a safe copy from a split archive.");
    return isRecoverySourceZipFamily() ? "" : tr("gui.recovery.zip_rebuild_zip_family_only", "ZIP index rebuild is available for ZIP-family archives");
  }

  function recoverySqzExportDisabledReason(): string {
    if (!isRecoverySourceSqz()) return tr("gui.recovery.choose_sqz_before_export", "Choose an SQZ archive before exporting.");
    return recoverySourceMatchesCurrentArchive()
      ? ""
      : tr("gui.recovery.open_selected_sqz_before_export", "Open this SQZ archive successfully before exporting it.");
  }

  function recoverySqzRepairDisabledReason(): string {
    return isRecoverySourceSqz()
      ? ""
      : tr("gui.recovery.choose_sqz_before_repair", "Choose an SQZ archive or SQZ volume before repairing.");
  }

  function recoveryProtectSourceDisabledReason(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.choose_archive_before_protect", "Choose an archive before creating PAR2 recovery data.");
    if (!recoverySourceMatchesCurrentArchive()) return tr("gui.recovery.open_selected_before_protect", "Open this archive successfully before creating new recovery data.");
    if (currentArchive?.read_only) return archiveMutationDisabledReason();
    return "";
  }

  function recoveryRedundancyValue(): number | null {
    const value = recoveryRedundancyDraft.trim();
    if (!/^\d+$/.test(value)) return null;
    const percent = Number(value);
    return Number.isSafeInteger(percent) && percent >= 1 && percent <= 100
      ? percent
      : null;
  }

  function recoveryRedundancyError(): string {
    return recoveryRedundancyValue() === null
      ? tr("gui.recovery.redundancy_invalid", "Enter a whole percentage from 1% to 100%.")
      : "";
  }

  function setRecoveryRedundancy(value: string): void {
    recoveryRedundancyDraft = value;
  }

  function recoveryProtectDisabledReason(): string {
    return recoveryProtectSourceDisabledReason() || recoveryRedundancyError();
  }

  function recoveryVerifyDisabledReason(): string {
    if (recoverySourceMatchesCurrentArchive() && currentArchive?.read_only) return archiveMutationDisabledReason();
    return recoverySourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_verify", "Choose the archive described by this PAR2 file.");
  }

  function recoveryRepairPar2DisabledReason(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.choose_archive_before_par2_repair", "Choose an archive before repairing with PAR2 data.");
    if (recoverySourceMatchesCurrentArchive() && currentArchive?.read_only) return archiveMutationDisabledReason();
    const gate = recoveryRepairGate(recoveryReport());
    if (gate === "verify_first") {
      return tr("gui.recovery.verify_before_repair", "Verify this archive and PAR2 set before creating a repaired copy.");
    }
    if (gate === "no_damage") {
      return tr("gui.recovery.no_damage_to_repair", "Verification found no damaged blocks to repair.");
    }
    if (gate === "over_capacity") {
      return tr("gui.recovery.insufficient_par2_data", "Verification found more damage than the available PAR2 data can repair");
    }
    return "";
  }

  function recoveryTestDisabledReason(): string {
    return recoverySourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_test", "Choose an archive before testing");
  }

  function recoveryBestEffortDisabledReason(): string {
    return recoverySourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_best_effort", "Choose an archive before extracting readable files.");
  }

  function recoveryPickerBusyReason(): string {
    return recoveryPickerStatus === "idle"
      ? ""
      : tr("gui.recovery.file_picker_open", "A file picker is already open.");
  }

  function archiveEncodingForJob(): string | null {
    return currentArchive?.encoding_override ?? null;
  }

  function extractEncodingForJob(): string | null {
    return extractPresetEncodingLabel ?? archiveEncodingForJob();
  }

  function recoveryEncodingForJob(): string | null {
    return recoverySourceMatchesCurrentArchive() ? archiveEncodingForJob() : null;
  }

  function recoverySelectionForJob(): string[] | null {
    return recoverySourceMatchesCurrentArchive() ? selectedJobPaths() : null;
  }

  function openExtractWorkspace(scope: ExtractScope) {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_extract", "Open an archive before extracting"));
      return;
    }
    if (scope === "selection" && archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    const selection = selectedJobPaths();
    if (scope === "selection" && !selection) {
      showNotice(tr("gui.precondition.select_before_extract", "Select one or more entries before extracting them"));
      return;
    }
    extractScope = scope;
    extractSelectionSnapshot = scope === "selection" ? [...(selection ?? [])] : [];
    setScreen("extract");
  }

  function extractJobPaths(): string[] | null {
    return extractScope === "selection" && extractSelectionSnapshot.length > 0
      ? [...extractSelectionSnapshot]
      : null;
  }

  function selectedJobPaths(): string[] | null {
    if (archiveSelectAllProgress) return null;
    const selected = [...selectedPaths()];
    return selected.length > 0 ? selected : null;
  }

  function hasArchiveOpen(): boolean {
    return currentArchive !== null;
  }

  function archiveMutationDisabledReason(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return currentArchive.read_only
      ? tr("gui.archive.nested_read_only", "Nested archives are read-only. Extract or convert to save changes.")
      : "";
  }

  function hasArchiveSelection(): boolean {
    return hasArchiveOpen() && !archiveSelectAllProgress && selectedPaths().size > 0;
  }

  function canRenameSelection(): boolean {
    return hasArchiveSelection() && !currentArchive?.read_only && selectedRenameSource() !== null;
  }

  function entryExtension(entryPath: string): string {
    const name = pathBaseName(entryPath);
    const index = name.lastIndexOf(".");
    return index > 0 ? name.slice(index + 1).toLowerCase() : "";
  }

  function previewEntryForPath(entryPath: string): EntryDto | null {
    return findLoadedRow(entryPath);
  }

  function previewEntryDisplayName(entryPath: string): string {
    return previewEntryForPath(entryPath)?.display || pathBaseName(entryPath);
  }

  function previewSystemCode(entryPath: string): PreviewPolicyCode {
    const ext = entryExtension(entryPath);
    return ext ? "system_type" : "system_unknown";
  }

  function previewPolicyFor(entryPath: string | null, entryType: EntryDto["entry_type"] | null = null): PreviewPolicy {
    if (!currentArchive) {
      return {
        kind: "none",
        label: tr("gui.preview.no_file", "Open or preview"),
        code: "no_archive",
        disabledReason: tr("gui.preview.open_archive_first", "Open an archive before opening or previewing entries"),
      };
    }
    if (!entryPath) {
      return {
        kind: "none",
        label: tr("gui.preview.no_file", "Open or preview"),
        code: "select_one",
        disabledReason: tr("gui.preview.select_one", "Select one entry to open or preview"),
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

    return {
      kind: "system-file",
      label: tr("gui.action.open_preview", "Open"),
      code: previewSystemCode(entryPath),
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
    return !archiveSelectAllProgress && selectedPreviewPolicy().kind !== "none" && !previewBusy();
  }

  function renameSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
    return canRenameSelection()
      ? ""
      : tr("gui.precondition.select_one_before_rename", "Select exactly one file entry before renaming");
  }

  function deleteSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_delete", "Select entries before deleting");
  }

  function moveSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
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
    if (previewBusy()) return tr("gui.preview.loading", "Preparing item");
    return selectedPreviewPolicy().disabledReason;
  }

  function previewActionLabel(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
  ): string {
    if (previewBusy()) return tr("gui.preview.loading", "Preparing item");
    return previewPolicyFor(entryPath, entryType).label;
  }

  function previewActionIcon(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
  ): "external-link" | "folder-open" | "eye" {
    const kind = previewPolicyFor(entryPath, entryType).kind;
    if (kind === "folder") return "folder-open";
    if (kind === "system-file") return "external-link";
    return "eye";
  }

  function isEntryPreviewBusy(entry: DisplayEntry): boolean {
    return Boolean(
      previewBusy() &&
      entry.source &&
      previewOriginEntryPath === entry.source.path &&
      (
        previewOriginVirtualIndex === null ||
        previewOriginVirtualIndex === entry.virtualIndex
      ),
    );
  }

  function previewEntryActionLabel(entry: DisplayEntry): string {
    if (!entry.source) return "";
    return isEntryPreviewBusy(entry)
      ? tr("gui.preview.loading", "Preparing item")
      : previewPolicyFor(entry.source.path, entry.source.entry_type).label;
  }

  function archiveActionTitle(enabled: boolean): string {
    return enabled ? "" : openArchiveFirstLabel();
  }

  function createExcludeRules(): string[] {
    return parseDelimitedRules(createExcludeText);
  }

  function createDraftExcludeCount(draft: CreateRunDraft): number {
    return draft.contentPolicy === "cross_platform_clean" ? 3 : draft.excludes.length;
  }

  function updateCreateExcludeText(value: string) {
    markCreatePresetDraftTouched();
    createExcludeText = value;
    normalizeUnsupportedCreatePostSuccess();
  }

  function createExcludeCountLabel(): string {
    const count = createExcludeRules().length;
    return tr("gui.create.rule_count", "{count} rules").replace("{count}", count.toLocaleString());
  }

  function createPreflightBusy(): boolean {
    return ["selecting", "measuring", "checkingTemp", "choosingDest", "checkingDest", "submitting"].includes(createPreflightPhase);
  }

  function createConfigurationPending(): boolean {
    return presetLoadState === "loading" || !sfxCreateCapabilityReady;
  }

  function createSourcesLocked(): boolean {
    return createSourcePickerBusy !== null
      || createPreflightBusy()
      || pendingCreateSubmission !== null;
  }

  function createSourcesLockedReason(): string {
    if (createSourcePickerBusy) {
      return tr("gui.create.sources.picker_open", "Finish choosing sources before changing the list");
    }
    if (createPreflightBusy()) {
      return tr("gui.create.sources.locked_preflight", "The source list is locked while Squallz checks it");
    }
    if (pendingCreateSubmission) {
      return tr("gui.create.sources.locked_review", "Confirm or cancel the current plan before changing the source list");
    }
    return "";
  }

  function createStartDisabled(): boolean {
    return createSources.length === 0
      || createSourcesLocked()
      || createConfigurationPending();
  }

  function createStartLabel(readyLabel: string): string {
    if (createConfigurationPending()) return tr("gui.create.preparing_settings", "Preparing settings");
    if (createSourcePickerBusy) return tr("gui.create.sources.adding", "Adding sources");
    if (createPreflightBusy()) return tr("gui.create.checking", "Checking");
    if (pendingCreateSubmission) return tr("gui.create.review_plan_below", "Review plan below");
    return readyLabel;
  }

  function createSourcePickerLabel(sourceKind: "files" | "folder"): string {
    if (createSourcePickerBusy === sourceKind) {
      return sourceKind === "files"
        ? tr("gui.create.opening_file_picker", "Opening file picker...")
        : tr("gui.create.opening_folder_picker", "Opening folder picker...");
    }
    return sourceKind === "files"
      ? tr("gui.action.add_files", "Add files")
      : tr("gui.create.sources.add_folders", "Add folders");
  }

  function createConfigurationPendingMessage(): string {
    return tr(
      "gui.create.wait_for_initial_settings",
      "Loading your default creation settings. Try again in a moment.",
    );
  }

  function createOptionsLockedReason(): string {
    return createPreflightBusy()
      ? tr("gui.create.options_locked_preflight", "Create settings are locked until this preflight ends")
      : "";
  }

  function createPreflightPhaseLabel(): string {
    switch (createPreflightPhase) {
      case "selecting":
        return tr("gui.create.waiting_source_picker", "Waiting for source picker");
      case "measuring":
        return tr("gui.create.measuring_source_bytes", "Measuring source bytes and exclude rules");
      case "checkingTemp":
        return tr("gui.create.checking_temp_workspace", "Checking workspace");
      case "choosingDest":
        if (createPreflightRequestKind === "destination") {
          return createPreflightCancelPending
            ? tr("gui.create.cancelling_destination_check", "Stopping the output check")
            : tr("gui.create.checking_existing_destination", "Checking the current output before replacement");
        }
        return createPreflightCurrent
          || tr("gui.create.waiting_destination", "Waiting for destination");
      case "checkingDest":
        return tr("gui.create.checking_destination_disk_short", "Checking destination disk");
      case "reviewing":
        return tr("gui.create.ready_for_review", "Ready for review");
      case "submitting":
        if (createPreflightRequestKind === "destination") {
          return createPreflightCancelPending
            ? tr("gui.create.cancelling_destination_check", "Stopping the output check")
            : tr("gui.create.rechecking_existing_destination", "Rechecking the current output");
        }
        if (createPreflightCurrent) return createPreflightCurrent;
        return createPreflightCreatingSfx
          ? tr("gui.create.submitting_sfx_job", "Starting self-extractor task")
          : tr("gui.create.submitting_archive_job", "Submitting archive job");
      case "ready":
        return tr("gui.create.preflight_ready", "Preflight ready");
      case "cancelled":
        return tr("gui.create.preflight_cancelled", "Preflight cancelled");
      case "blocked":
        return tr("gui.create.preflight_blocked", "Preflight blocked");
      case "idle":
        return tr("gui.create.preflight_pending", "Preflight pending");
    }
  }

  function createPreflightStepState(stage: Exclude<CreatePreflightStage, "submit">): CreatePreflightStepState {
    if (createPreflightIssueStage === stage) {
      return createPreflightPhase === "cancelled" ? "cancelled" : "blocked";
    }
    if (stage === "source") {
      if (createPreflightPhase === "selecting" || createPreflightPhase === "measuring") return "active";
      if (lastCreatePlan && lastCreatePlan.entries > 0) return "ready";
      return "pending";
    }
    if (stage === "temp") {
      if (createPreflightPhase === "checkingTemp") return "active";
      if (lastTempDiskSpace) {
        return lastTempDiskSpace.ok && (lastSystemTempDiskSpace?.ok ?? true) ? "ready" : "blocked";
      }
      return "pending";
    }
    if (
      createPreflightPhase === "choosingDest"
      || createPreflightPhase === "checkingDest"
      || (createPreflightPhase === "submitting" && createPreflightRequestKind === "destination")
    ) return "active";
    if (lastDiskSpace) return lastDiskSpace.ok ? "ready" : "blocked";
    return "pending";
  }

  function createPreflightStepStateLabel(state: CreatePreflightStepState): string {
    if (state === "active") return tr("gui.create.preflight_stage_active", "In progress");
    if (state === "ready") return tr("gui.create.preflight_stage_ready", "Checked");
    if (state === "blocked") return tr("gui.create.preflight_stage_blocked", "Blocked");
    if (state === "cancelled") return tr("gui.create.preflight_stage_cancelled", "Cancelled");
    return tr("gui.create.preflight_stage_pending", "Pending");
  }

  function createPreflightCurrentDetail(): string {
    if (createPreflightPhase !== "measuring" || !createPreflightCurrent) return "";
    return tr("gui.create.scanning_current_item", "Current · {path}").replace("{path}", createPreflightCurrent);
  }

  function createDestinationPreflightDetail(): string {
    if (createPreflightRequestKind === "destination" && createPreflightCurrent) {
      return tr("gui.create.destination_check_current", "Current · {path}")
        .replace("{path}", createPreflightCurrent);
    }
    return lastCreateDest
      ? tr("gui.create.destination_path_checked", "Destination · {path}").replace("{path}", lastCreateDest)
      : "";
  }

  function createDestinationInspectionCancellable(): boolean {
    return createPreflightRequestKind === "destination"
      && createPreflightRequestId !== null
      && (createPreflightPhase === "choosingDest" || createPreflightPhase === "submitting");
  }

  function createDestinationInspectionCancelLabel(): string {
    return createPreflightCancelPending
      ? tr("gui.create.cancelling_destination_check", "Stopping the output check")
      : tr("gui.create.cancel_destination_check", "Cancel output check");
  }

  function createPreflightStageIssueSummary(stage: Exclude<CreatePreflightStage, "submit">): string | null {
    if (createPreflightIssueStage !== stage) return null;
    return createPreflightPhase === "cancelled"
      ? tr("gui.create.preflight_stage_cancelled_summary", "Cancelled before this check completed")
      : tr("gui.create.preflight_stage_blocked_summary", "This check could not finish");
  }

  function createPreflightSteps() {
    const sourceState = createPreflightStepState("source");
    const tempState = createPreflightStepState("temp");
    const destinationState = createPreflightStepState("destination");
    return [
      {
        id: "source",
        label: tr("gui.create.input_preflight", "Input preflight"),
        summary: createEstimateStatusbar(),
        detail: createPreflightCurrentDetail(),
        state: sourceState,
        stateLabel: createPreflightStepStateLabel(sourceState),
      },
      {
        id: "temp",
        label: tr("gui.create.temp_preflight", "Workspace peak"),
        summary: tempPreflightStatusbar(),
        detail: "",
        state: tempState,
        stateLabel: createPreflightStepStateLabel(tempState),
      },
      {
        id: "destination",
        label: tr("gui.create.disk_preflight", "Disk preflight"),
        summary: diskPreflightStatusbar(),
        detail: createDestinationPreflightDetail(),
        state: destinationState,
        stateLabel: createPreflightStepStateLabel(destinationState),
      },
    ];
  }

  function createVolumePreview(): string {
    const splitSize = createSplitSizeBytes();
    if (splitSize === null) {
      return tr("gui.create.single_archive_summary", "Single archive · no numbered parts");
    }
    const size = formatBytes(splitSize);
    if (!lastCreatePlan) {
      const nativeWim = createSplitMode === "native" && activeCreateFormat === "wim";
      const key = nativeWim
        ? "gui.create.native_split_wim_summary_pending"
        : createSplitMode === "native"
          ? "gui.create.native_split_summary_pending"
          : "gui.create.split_summary_pending";
      const fallback = nativeWim
        ? "{size} target per part · native .swm set; one large file may exceed the target"
        : createSplitMode === "native"
          ? "{size} per part · native ZIP set ending in .zip"
          : "{size} per part · final count appears after preflight and write";
      return tr(key, fallback)
        .replace("{size}", size);
    }
    const count = Math.max(1, Math.trunc(lastCreatePlan.split_volume_count_budget ?? 1));
    return tr("gui.create.volume_output_budget_guide", "{size} per part · budget guide up to {count} parts; final count depends on compression")
      .replace("{size}", size)
      .replace("{count}", String(count));
  }

  function createSetupPresetLabel(): string {
    const selected = selectedCreateArchivePreset();
    return selected
      ? archivePresetDisplayName(selected)
      : tr("gui.create.setup.custom", "Custom setup");
  }

  function createCompletionSummaryLabel(): string {
    if (createCompletion === "reveal_output") {
      return tr("gui.create.output.completion.reveal", "Reveal in {fileManager}")
        .replace("{fileManager}", fileManagerLabel());
    }
    if (createCompletion === "open_in_squallz") {
      return tr("gui.create.output.completion.open", "Open in Squallz");
    }
    return tr("gui.create.output.completion.none", "Do nothing");
  }

  function createSourceHandlingSummaryLabel(): string {
    return createPostSuccess === "trash_source"
      ? tr("gui.create.output.source.trash", "Move originals to {trash}").replace("{trash}", trashNameLabel())
      : tr("gui.create.output.source.keep", "Keep originals");
  }

  function createIntegritySummaryLabel(): string {
    return effectiveCreateTestAfterCreate()
      ? tr("gui.create.output.integrity.enabled_summary", "Full integrity test")
      : tr("gui.create.output.integrity.disabled_summary", "No extra integrity test");
  }

  function createProtectionSummaryLabel(): string {
    if (createPassword.length > 0 && createEncryptNames) {
      return tr("gui.create.setup.password_and_names", "Password + encrypted names");
    }
    if (createPassword.length > 0) {
      return tr("gui.create.setup.password_on", "Password on");
    }
    if (createPresetCredentialIntent === "prompt") {
      return tr("gui.create.setup.password_required", "Password required");
    }
    return tr("gui.create.setup.no_password", "No password");
  }

  function createSetupSummaryItems() {
    const outputValue = createSfxEnabled
      ? createSfxOutputLabel()
      : activeCreateFormatData().label;
    const outputDetail = createSfxEnabled ? createSfxSummary() : createMethodLabel();
    return [
      {
        id: "preset",
        icon: "sparkles",
        label: tr("gui.create.setup.preset", "Current preset"),
        value: createSetupPresetLabel(),
        detail: createArchivePresetSummary(currentCreateArchivePresetOptions()),
      },
      {
        id: "artifact",
        icon: "archive",
        label: tr("gui.create.setup.artifact", "Output"),
        value: outputValue,
        detail: outputDetail,
      },
      {
        id: "destination",
        icon: "folder-open",
        label: tr("gui.create.setup.destination", "Save & finish"),
        value: createOutputPreview(),
        detail: tr(
          "gui.create.setup.afterwards_with_integrity",
          "{completion} · {sources} · {integrity}",
        )
          .replace("{completion}", createCompletionSummaryLabel())
          .replace("{sources}", createSourceHandlingSummaryLabel())
          .replace("{integrity}", createIntegritySummaryLabel()),
      },
      {
        id: "protection",
        icon: "lock",
        label: tr("gui.create.setup.protection", "Protection & volumes"),
        value: createProtectionSummaryLabel(),
        detail: tr("gui.create.setup.volume_and_content", "{volumes} · {content}")
          .replace("{volumes}", createVolumePreview())
          .replace("{content}", createContentPolicyLabel(createContentPolicy)),
      },
    ];
  }

  function createPlanLayoutSummary(): string {
    const pending = pendingCreateSubmission;
    const plan = lastCreatePlan;
    if (!pending || !plan) return "";
    if (pending.creatingSfx) return pending.artifactLabel;
    const count = plan.split_volume_count_budget;
    if (pending.splitSize !== null && count !== null) {
      const volumeSummary = tr("gui.create.review.numbered_volumes", "Numbered volumes · up to {count} data parts")
        .replace("{count}", Math.max(1, Math.trunc(count)).toLocaleString());
      return pending.format === "sqz"
        ? tr("gui.create.review.sqz_recovery_separate", "{volumes} · SQZ recovery files are separate")
          .replace("{volumes}", volumeSummary)
        : volumeSummary;
    }
    return tr("gui.create.review.single_file", "One archive file");
  }

  function createPlanReviewItems() {
    const plan = lastCreatePlan;
    if (!plan) return [];
    const workspace = plan.system_temp_budget_bytes > 0
      ? tr("gui.create.review.workspace_split", "{destination} destination + {temporary} system temporary")
        .replace("{destination}", formatBytes(plan.workspace_budget_bytes))
        .replace("{temporary}", formatBytes(plan.system_temp_budget_bytes))
      : tr("gui.create.review.workspace_destination", "{size} on the destination filesystem")
        .replace("{size}", formatBytes(plan.workspace_budget_bytes));
    return [
      {
        id: "inputs",
        label: tr("gui.create.review.inputs", "Measured inputs"),
        value: tr("gui.create.review.input_value", "{size} · {entries} entries")
          .replace("{size}", formatBytes(plan.total_bytes))
          .replace("{entries}", plan.entries.toLocaleString()),
      },
      ...(plan.deduplicated_entries > 0
        ? [{
            id: "overlap",
            label: tr("gui.create.review.overlap", "Overlap handling"),
            value: tr(
              "gui.create.review.overlap_value",
              "{count} repeated entries merged after filters",
            ).replace("{count}", plan.deduplicated_entries.toLocaleString()),
          }]
        : []),
      {
        id: "layout",
        label: tr("gui.create.review.layout", "Output layout"),
        value: createPlanLayoutSummary(),
      },
      {
        id: "budget",
        label: tr("gui.create.review.output_budget", "Final output space upper bound"),
        value: formatBytes(plan.final_output_budget_bytes),
      },
      {
        id: "workspace",
        label: tr("gui.create.review.workspace", "Peak creation workspace"),
        value: workspace,
      },
    ];
  }

  function createWorkspaceSurface(variant: CreateWorkspaceVariant): CreateWorkspaceSurface {
    const preflightBusy = createPreflightBusy();
    const lockedReason = createOptionsLockedReason();
    const sourceLockedReason = createSourcesLockedReason();
    const selectedSourceCount = createSourceSelectedCount();
    const allSourcesSelected = createSourceAllSelected();
    const reviewDisabledReason = createSources.length === 0
      ? tr("gui.create.sources.add_before_review", "Add at least one file or folder before reviewing")
      : createConfigurationPending()
        ? createConfigurationPendingMessage()
        : sourceLockedReason;
    const createPreset = selectedCreateArchivePreset();
    const sqzPayloadLabel = tr("gui.presets.sqz_inner_format", "SQZ payload");
    const review = pendingCreateSubmission && lastCreatePlan
      ? {
          variant,
          ariaLabel: tr("gui.create.review.aria", "Create plan review"),
          eyebrow: tr("gui.create.review.eyebrow", "Checked and ready"),
          heading: tr("gui.create.review.heading", "Review before creating"),
          description: createPreflightIssue || tr("gui.create.review.description", "Squallz scanned the selected sources and checked the required filesystems. The sizes below are conservative safety bounds, not predicted compressed sizes."),
          outputName: lastCreatePlan.primary_output,
          items: createPlanReviewItems(),
          confirmLabel: createPlanConfirmLabel(),
          cancelLabel: tr("gui.create.review.cancel", "Cancel plan"),
          busy: createPreflightPhase === "submitting",
          onConfirm: () => void confirmCreatePlan(),
          onCancel: cancelCreatePlanReview,
        }
      : null;

    return {
      tr,
      sources: {
        ariaLabel: tr("gui.create.sources.aria", "Items to archive"),
        heading: tr("gui.create.sources.heading", "Items to archive"),
        description: tr(
          "gui.create.sources.description",
          "Add files or folders, then review the list before scanning.",
        ),
        countLabel: createSourceCountLabel(),
        selectionLabel: createSourceSelectionLabel(),
        selectAllLabel: tr("gui.create.sources.select_all", "Select all source items"),
        emptyTitle: tr("gui.create.sources.empty_title", "Nothing added yet"),
        emptyBody: tr(
          "gui.create.sources.empty_body",
          "Add files or folders, or drag them into this window.",
        ),
        removeSelectedLabel: tr("gui.create.sources.remove_selected", "Remove selected"),
        keepUntilQueuedLabel: tr(
          "gui.create.sources.keep_until_queued",
          "This list is cleared only after the task is added to the queue.",
        ),
        lockedReason: sourceLockedReason,
        rows: createSources.map((source) => {
          const name = desktopBasename(source.path, platformKind());
          return {
            path: source.path,
            name,
            parent: desktopDirname(source.path, platformKind()),
            kind: source.kind,
            kindLabel: createSourceKindLabel(source.kind),
            selected: createSourceSelected(source.path),
            selectLabel: tr("gui.create.sources.select_item", "Select {name}")
              .replace("{name}", name),
            removeLabel: tr("gui.create.sources.remove_item", "Remove {name}")
              .replace("{name}", name),
          };
        }),
        selectedCount: selectedSourceCount,
        allSelected: allSourcesSelected,
        mixedSelection: selectedSourceCount > 0 && !allSourcesSelected,
        addFiles: {
          label: createSourcePickerLabel("files"),
          disabled: createSourcesLocked(),
          busy: createSourcePickerBusy === "files",
          title: sourceLockedReason,
          onSelect: () => void submitCreateJob("files"),
        },
        addFolders: {
          label: createSourcePickerLabel("folder"),
          disabled: createSourcesLocked(),
          busy: createSourcePickerBusy === "folder",
          title: sourceLockedReason,
          onSelect: () => void submitCreateJob("folder"),
        },
        review: {
          label: createStartLabel(tr("gui.create.sources.review", "Review and create")),
          disabled: createStartDisabled(),
          busy: createPreflightBusy(),
          title: reviewDisabledReason,
          onSelect: () => void submitCreateSourceList(),
        },
        onToggleAll: setAllCreateSourcesSelected,
        onToggleRow: toggleCreateSourceSelection,
        onRemoveRow: removeCreateSource,
        onRemoveSelected: removeSelectedCreateSources,
        onClearSelection: clearCreateSourceSelection,
      },
      profiles: createProfileIds.map((profileId) => ({
        id: profileId,
        label: createProfileLabel(profileId),
        selected: activeCreateProfile === profileId,
        disabled: preflightBusy,
        title: lockedReason,
        ariaLabel: labelWithDisabledReason(createProfileLabel(profileId), lockedReason),
        onSelect: () => chooseCreateProfile(profileId),
      })),
      formats: createFormatIds.map((formatId) => {
        const disabledReason = createFormatDisabledReason(formatId);
        return {
          id: formatId,
          label: createFormats[formatId].label,
          selected: activeCreateFormat === formatId,
          disabled: Boolean(disabledReason),
          title: disabledReason || createFormatNoteFor(formatId),
          ariaLabel: labelWithDisabledReason(createFormats[formatId].label, disabledReason),
          onSelect: () => chooseCreateFormat(formatId),
        };
      }),
      formatNote: createFormatNote(),
      sqzPayload: activeCreateFormat === "sqz"
        ? {
            label: sqzPayloadLabel,
            options: presetSqzInnerFormats.map((innerFormat) => ({
              id: innerFormat,
              label: presetSqzInnerFormatLabel(innerFormat),
              selected: createPresetSqzInnerFormat === innerFormat,
              disabled: preflightBusy,
              title: lockedReason,
              ariaLabel: labelWithDisabledReason(presetSqzInnerFormatLabel(innerFormat), lockedReason),
              onSelect: () => selectPresetSqzInnerFormat(innerFormat),
            })),
          }
        : null,
      compression: {
        level: createCompressionLevel(),
        detail: activeCreateProfileDetail(),
        method: createMethodLabel(),
        custom: activeCreateProfile === "custom"
          ? {
              value: customCreateLevel,
              error: customCreateLevelError,
              disabled: preflightBusy,
              title: lockedReason,
              rangeAriaLabel: labelWithDisabledReason(
                variant === "classic"
                  ? tr("gui.create.classic_custom_compression_level", "Classic custom compression level")
                  : tr("gui.create.custom_compression_level", "Custom compression level"),
                lockedReason,
              ),
              numberAriaLabel: labelWithDisabledReason(
                variant === "classic"
                  ? tr("gui.create.classic_custom_compression_level_number", "Classic custom compression level number")
                  : tr("gui.create.custom_compression_level_number", "Custom compression level number"),
                lockedReason,
              ),
              onInput: (event) => updateCustomCreateLevelFromInput(event),
              onChange: (event) => updateCustomCreateLevelFromInput(event, true),
            }
          : null,
      },
      setupSummary: {
        variant,
        ariaLabel: tr("gui.create.setup.aria", "Current create setup"),
        eyebrow: tr("gui.create.setup.eyebrow", "Before choosing sources"),
        heading: tr("gui.create.setup.heading", "What Squallz will create"),
        items: createSetupSummaryItems(),
      },
      preset: {
        instanceId: `${variant}-create`,
        variant,
        compact: variant === "modern",
        kind: "create",
        options: createPresetPickerOptions(),
        selectedId: selectedCreatePresetId,
        draftName: createPresetDraftName,
        summary: createArchivePresetSummary(currentCreateArchivePresetOptions()),
        status: createPresetStatus(),
        statusLabel: archivePresetStatusLabel(createPresetStatus()),
        disabledReason: archivePresetPickerDisabledReason("create"),
        updateDisabledReason: createPresetUpdateDisabledReason(),
        deleteDisabledReason: presetDeleteDisabledReason(createPreset),
        fileManagerDisabledReason: createPresetFinderDisabledReason(),
        isDefault: presetDocument?.bindings.app_default_create === selectedCreatePresetId,
        isFileManagerDefault: presetDocument?.bindings.file_manager_create === selectedCreatePresetId,
        tr,
        onSelect: (id) => applyCreatePreset(id),
        onDraftNameInput: (name) => (createPresetDraftName = name),
        onUpdate: () => void updateSelectedArchivePreset("create"),
        onSaveAs: () => void saveCurrentCreatePresetAsNew(),
        onDelete: () => void deleteSelectedArchivePreset("create"),
        onDefaultChange: (enabled) => void setArchivePresetBinding("create", "app", enabled),
        onFileManagerDefaultChange: (enabled) => void setArchivePresetBinding("create", "file_manager", enabled),
      },
      advanced: {
        open: createAdvancedOpen,
        onToggle: (open) => (createAdvancedOpen = open),
        onKeydown: toggleCreateAdvancedFromKeyboard,
      },
      output: {
        instanceId: `${variant}-create-output`,
        variant,
        destination: createDestinationBase,
        completion: createCompletion,
        postSuccess: createPostSuccess,
        testAfterCreate: effectiveCreateTestAfterCreate(),
        testAfterCreateRequired: createPostSuccess === "trash_source",
        outputPreview: createOutputPreview(),
        defaultFolder: normalizedDefaultCreateDir(appliedDefaultCreateDir) ?? "",
        fileManager: fileManagerLabel(),
        trashName: trashNameLabel(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        openDisabledReason: createOpenCompletionDisabledReason(),
        trashDisabledReason: createTrashSourceDisabledReason(),
        tr,
        onDestinationChange: updateCreateDestinationBase,
        onCompletionChange: updateCreateCompletion,
        onPostSuccessChange: updateCreatePostSuccess,
        onTestAfterCreateChange: updateCreateTestAfterCreate,
      },
      content: {
        variant,
        classicSectionId: variant === "classic" ? "classic-create-content" : undefined,
        value: createContentPolicy,
        rulesText: createExcludeText,
        rules: createExcludeRules(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        tr,
        onChange: updateCreateContentPolicy,
        onRulesInput: updateCreateExcludeText,
      },
      recovery: {
        capability: createRecoveryCapability(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        onOpen: openRecoveryConfiguration,
      },
      sfx: {
        variant,
        classicSectionId: variant === "classic" ? "classic-create-security" : undefined,
        enabled: createSfxEnabled,
        available: sfxCreateCapability.available,
        targetLabel: createSfxTargetLabel(),
        outputLabel: createSfxOutputLabel(),
        summary: createSfxSummary(),
        signingWarning: createSfxSigningWarning(),
        unavailableMessage: createSfxUnavailableMessage(),
        disabled: preflightBusy || !sfxCreateCapabilityReady,
        disabledReason: lockedReason || (!sfxCreateCapabilityReady
          ? tr("gui.create.sfx_capability_loading", "Checking self-extracting support")
          : ""),
        loading: !sfxCreateCapabilityReady,
        tr,
        onEnabledChange: updateCreateSfxEnabled,
      },
      protection: {
        variant,
        classicSplitSectionId: variant === "classic" ? "classic-create-volumes" : undefined,
        password: createPassword,
        passwordConfirmation: createPasswordConfirmation,
        passwordVisible: createPasswordVisible,
        encryptNames: createEncryptNames,
        canEncryptData: createPasswordDataAvailable(),
        canEncryptNames: createNameEncryptionAvailable(),
        splitDisabled: createSfxEnabled,
        splitPreset: createSplitPreset,
        splitMode: createSplitMode,
        nativeSplitKind: nativeSplitKind(activeCreateFormat, createSfxEnabled),
        customSplitAmount: createCustomSplitAmount,
        customSplitUnit: createCustomSplitUnit,
        passwordCapability: createPasswordCapability(),
        nameEncryptionCapability: createNameEncryptionCapability(),
        splitCapability: createSplitCapability(),
        splitSummary: createVolumePreview(),
        passwordError: visibleCreatePasswordError(),
        splitError: visibleCreateSplitError(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        tr,
        onPasswordInput: updateCreatePassword,
        onPasswordConfirmationInput: updateCreatePasswordConfirmation,
        onPasswordVisibleChange: (visible) => (createPasswordVisible = visible),
        onEncryptNamesChange: updateCreateEncryptNames,
        onSplitPresetChange: updateCreateSplitPreset,
        onSplitModeChange: updateCreateSplitMode,
        onCustomSplitAmountInput: updateCreateCustomSplitAmount,
        onCustomSplitUnitChange: updateCreateCustomSplitUnit,
      },
      showPreflight: createPreflightPhase !== "idle",
      preflight: {
        variant,
        phase: createPreflightPhase,
        ariaLabel: tr("gui.create.preflight_status", "Create preflight status"),
        heading: tr("gui.create.preflight_heading", "Before compression"),
        statusLabel: createPreflightPhaseLabel(),
        lockMessage: lockedReason,
        actionLabel: createDestinationInspectionCancellable()
          ? createDestinationInspectionCancelLabel()
          : "",
        actionPending: createPreflightCancelPending,
        issue: createPreflightIssue,
        steps: createPreflightSteps(),
        onAction: () => void cancelCreateDestinationInspection(),
      },
      review,
      classic: {
        archiveName: createArchivePreviewName(),
        activeSection: classicCreateSection,
        sections: [
          {
            id: "general",
            label: settingsSectionLabel("General"),
            targetId: "classic-create-general",
            onSelect: () => void showClassicCreateSection("general", "classic-create-general"),
          },
          {
            id: "compression",
            label: tr("gui.create.section_compression", "Compression"),
            targetId: "classic-create-compression",
            onSelect: () => void showClassicCreateSection("compression", "classic-create-compression"),
          },
          {
            id: "content",
            label: tr("gui.create.section_content", "Contents"),
            targetId: "classic-create-content",
            onSelect: () => void showClassicCreateSection("content", "classic-create-content"),
          },
          {
            id: "security",
            label: settingsSectionLabel("Security"),
            targetId: "classic-create-security",
            onSelect: () => void showClassicCreateSection("security", "classic-create-security"),
          },
          {
            id: "volumes",
            label: tr("gui.create.section_volumes", "Volumes"),
            targetId: "classic-create-volumes",
            onSelect: () => void showClassicCreateSection("volumes", "classic-create-volumes"),
          },
          {
            id: "recovery",
            label: tr("gui.recovery.title", "Recovery"),
            targetId: "classic-create-recovery",
            onSelect: () => void showClassicCreateSection("recovery", "classic-create-recovery"),
          },
          {
            id: "preflight",
            label: tr("gui.create.input_preflight", "Preflight"),
            targetId: "classic-create-preflight",
            onSelect: () => void showClassicCreateSection("preflight", "classic-create-preflight"),
          },
        ],
        recoveryCapability: createRecoveryCapability(),
        updateMode: tr("gui.create.add_and_replace_files", "Add and replace files"),
        featuredFormats: featuredFormatCards(),
      },
    };
  }

  function createEstimateStatusbar(): string {
    const interrupted = createPreflightStageIssueSummary("source");
    if (interrupted) return interrupted;
    if (createPreflightPhase === "selecting") return tr("gui.create.waiting_source_picker", "Waiting for source picker");
    if (createPreflightPhase === "measuring") {
      return createPreflightScanned > 0
        ? tr("gui.create.scanning_inputs_count", "Scanning inputs · {count} entries").replace("{count}", createPreflightScanned.toLocaleString())
        : tr("gui.create.measuring_input_bytes", "Measuring input bytes...");
    }
    if (createPreflightPhase === "blocked" && lastCreatePlan?.entries === 0) return tr("gui.create.no_entries_after_excludes", "No entries after excludes");
    if (!lastCreatePlan) return tr("gui.create.input_estimate_pending", "Input estimate pending source selection");
    return tr("gui.create.estimate_status", "{size} input · {entries} entries · {excludes}")
      .replace("{size}", formatBytes(lastCreatePlan.total_bytes))
      .replace("{entries}", lastCreatePlan.entries.toLocaleString())
      .replace(
        "{excludes}",
        tr("gui.create.rule_count", "{count} rules").replace("{count}", createPreflightExcludeCount.toLocaleString()),
      );
  }

  function diskPreflightStatusbar(): string {
    const interrupted = createPreflightStageIssueSummary("destination");
    if (interrupted) return interrupted;
    if (createPreflightPhase === "choosingDest") {
      if (createPreflightRequestKind === "destination") {
        return tr("gui.create.destination_check_progress", "Reading current output · {bytes}")
          .replace("{bytes}", formatBytes(createPreflightProcessedBytes));
      }
      return createPreflightCurrent
        || tr("gui.create.waiting_destination_picker", "Waiting for destination picker");
    }
    if (createPreflightPhase === "submitting" && createPreflightRequestKind === "destination") {
      return tr("gui.create.destination_recheck_progress", "Reading current output again · {bytes}")
        .replace("{bytes}", formatBytes(createPreflightProcessedBytes));
    }
    if (createPreflightPhase === "checkingDest") return tr("gui.create.checking_destination_disk", "Checking destination disk...");
    if (!lastDiskSpace) return tr("gui.create.destination_disk_pending", "Destination disk preflight pending");
    return tr("gui.create.disk_status_available", "{status} · {available} available")
      .replace("{status}", lastDiskSpace.ok ? tr("gui.create.disk_ok", "Disk OK") : tr("gui.create.disk_blocked", "Disk blocked"))
      .replace("{available}", formatBytes(lastDiskSpace.available_bytes));
  }

  function tempPreflightStatusbar(): string {
    const interrupted = createPreflightStageIssueSummary("temp");
    if (interrupted) return interrupted;
    if (createPreflightPhase === "checkingTemp") return tr("gui.create.checking_temporary_space", "Checking workspace...");
    if (!lastTempDiskSpace) return tr("gui.create.temp_preflight_pending", "Workspace check pending");
    if (lastSystemTempDiskSpace) {
      const ok = lastTempDiskSpace.ok && lastSystemTempDiskSpace.ok;
      return tr("gui.create.temp_status_destination_and_system", "{status} · destination {destination} · temporary {temporary}")
        .replace("{status}", ok ? tr("gui.create.temp_ok", "Workspace OK") : tr("gui.create.temp_blocked", "Workspace blocked"))
        .replace("{destination}", formatBytes(lastTempDiskSpace.available_bytes))
        .replace("{temporary}", formatBytes(lastSystemTempDiskSpace.available_bytes));
    }
    return tr("gui.create.temp_status_available", "{status} · {available} available")
      .replace("{status}", lastTempDiskSpace.ok ? tr("gui.create.temp_ok", "Workspace OK") : tr("gui.create.temp_blocked", "Workspace blocked"))
      .replace("{available}", formatBytes(lastTempDiskSpace.available_bytes));
  }

  function beginCreatePreflight(draft: CreateRunDraft, phase: "selecting" | "choosingDest") {
    createPresetDraftTouched = true;
    createPreflightPhase = phase;
    createPreflightScanned = 0;
    createPreflightCurrent = "";
    createPreflightRequestId = null;
    createPreflightRequestKind = null;
    createPreflightProcessedBytes = 0;
    createPreflightCancelPending = false;
    createPreflightExcludeCount = createDraftExcludeCount(draft);
    createPreflightIssue = "";
    createPreflightIssueStage = null;
    createPreflightCreatingSfx = draft.sfxEnabled;
    lastCreatePlan = null;
    lastDiskSpace = null;
    lastTempDiskSpace = null;
    lastSystemTempDiskSpace = null;
    lastCreateDest = null;
    pendingCreateSubmission = null;
  }

  function invalidateCreatePreflightResult() {
    if (createPreflightBusy() || createPreflightPhase === "idle") return;
    const pending = pendingCreateSubmission;
    createPreflightPhase = "idle";
    createPreflightScanned = 0;
    createPreflightCurrent = "";
    createPreflightRequestId = null;
    createPreflightRequestKind = null;
    createPreflightProcessedBytes = 0;
    createPreflightCancelPending = false;
    createPreflightIssue = "";
    createPreflightIssueStage = null;
    lastCreatePlan = null;
    lastDiskSpace = null;
    lastTempDiskSpace = null;
    lastSystemTempDiskSpace = null;
    lastCreateDest = null;
    pendingCreateSubmission = null;
    if (pending) resetCreateCredentialsAfterPlan(pending);
  }

  function resetCreateCredentialsAfterPlan(pending: PendingCreateSubmission | null) {
    clearCreatePasswordFields();
    if (pending?.restoreCredentialPrompt) {
      createPresetCredentialIntent = "prompt";
      createEncryptNames = pending.restoreEncryptNames;
    }
  }

  function createPrimaryAction(): HTMLElement | null {
    return document.getElementById("create-primary-source-action");
  }

  function focusCreatePrimaryAction() {
    void tick().then(() => createPrimaryAction()?.focus());
  }

  function discardPendingCreatePlan(restoreFocus = false) {
    invalidateCreatePreflightResult();
    if (restoreFocus) focusCreatePrimaryAction();
  }

  function finishCreatePreflightWithIssue(
    stage: CreatePreflightStage,
    message: string,
    phase: "blocked" | "cancelled" = "blocked",
  ) {
    createPreflightIssueStage = stage;
    createPreflightIssue = message;
    createPreflightCurrent = "";
    createPreflightRequestId = null;
    createPreflightRequestKind = null;
    createPreflightProcessedBytes = 0;
    createPreflightCancelPending = false;
    createPreflightPhase = phase;
    showNotice(message);
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
    if (findLoadedRow(folder) || findLoadedRow(folder.slice(0, -1))) {
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
    return findLoadedRow(entryPath)?.entry_type ?? null;
  }

  function nestedPreviewTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase === "nested") return tr("gui.preview.loading", "Preparing item");
    if (!nestedPreview) return tr("gui.preview.no_nested", "Preview");
    return `${pathBaseName(nestedPreview.entry_path)} · ${nestedPreview.format.toUpperCase()}`;
  }

  function nestedPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase === "nested") {
      return tr("gui.preview.loading_target", "Preparing {name}...")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry", "selected entry"));
    }
    if (!nestedPreview) return tr("gui.preview.select_file", "Select an item, then choose Open or Preview.");
    return tr("gui.preview.nested_entries", "{count} entries{suffix}")
      .replace("{count}", nestedPreview.entry_count.toLocaleString())
      .replace("{suffix}", nestedPreview.truncated ? tr("gui.preview.first_200_shown_suffix", " · first 200 shown") : "");
  }

  function nestedPreviewRows(): EntryDto[] {
    return nestedPreview?.items.slice(0, 5) ?? [];
  }

  function entryPreviewTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") return tr("gui.preview.loading", "Preparing item");
    if (entryPreviewFailure) return tr("gui.preview.failed_title", "Item did not open");
    if (!entryPreview) return tr("gui.preview.no_file", "Open or preview");
    return entryPreview.display_name;
  }

  function entryPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") {
      return tr("gui.preview.loading_target", "Preparing {name}...")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry", "selected entry"));
    }
    if (entryPreviewFailure) {
      return entryPreviewFailure.message;
    }
    if (!entryPreview) {
      return tr("gui.preview.select_file", "Select an item, then choose Open or Preview.");
    }
    return formatBytes(entryPreview.size);
  }

  function selectedPreviewPolicyCode(): PreviewPolicyCode {
    return selectedPreviewPolicy().code;
  }

  function activePreviewPolicyKind(): PreviewPolicyKind | "failed" {
    if (entryPreviewFailure) return "failed";
    if (nestedPreview) return "nested";
    if (entryPreview) return "system-file";
    return selectedPreviewPolicy().kind;
  }

  function entryPreviewPolicyCode(): PreviewPolicyCode {
    if (!entryPreview) return selectedPreviewPolicyCode();
    return "system_ready";
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
    const failure = entryPreviewFailure;
    if (!failure) return;
    if (failure.policyKind === "nested") {
      if (failure.retryAction === "open") {
        void openNestedArchiveEntry(failure.outerSource, failure.entryPath);
      } else {
        void submitPreviewNestedArchive(failure.entryPath, previewOriginVirtualIndex);
      }
      return;
    }
    void submitPreviewEntry(
      failure.entryPath,
      failure.entryType,
      previewOriginVirtualIndex,
    );
  }

  async function extractEntryPreviewFailure() {
    const failure = entryPreviewFailure;
    if (!failure) return;
    if (failure.policyKind === "nested") {
      const queued = await submitNestedExtract(
        failure.outerSource,
        failure.outerDisplayPath,
        failure.entryPath,
      );
      if (queued) clearEntryPreviewState();
      return;
    }
    const selectionBusyReason = archiveSelectionBusyReason();
    if (selectionBusyReason) {
      showNotice(selectionBusyReason);
      return;
    }
    const row = previewEntryForPath(failure.entryPath);
    if (!row) {
      showNotice(tr("gui.preview.extract_unavailable", "Return to the entry and select Extract instead."));
      return;
    }
    clearSelection();
    toggleSelect(row);
    clearEntryPreviewState();
    openExtractWorkspace("selection");
  }

  function previewFailureMessage(
    error: unknown,
    nested: boolean,
    fallbackKey: string,
    fallback: string,
  ): string {
    if (isErrorDto(error)) {
      if (error.key.startsWith("error.preview_")) {
        return tr(error.key, fallback);
      }
      if (error.key === "error.resource_limit") {
        return nested
          ? tr("gui.preview.nested_resource_limit", "Nested preview reached a safety limit. Extract the inner archive instead.")
          : tr("gui.preview.resource_limit", "Opening this item reached a safety limit. Extract it instead.");
      }
    }
    return tr(fallbackKey, fallback);
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
    const count = extractJobPaths()?.length ?? 0;
    return count > 0
      ? tr("gui.selection.count_selected", "{count} selected").replace("{count}", count.toLocaleString())
      : tr("gui.selection.all_entries", "All entries");
  }

  function extractActionLabel(): string {
    if (!currentArchive) return tr("gui.extract.start", "Extract");
    return extractJobPaths()
      ? actionLabel("Extract selected")
      : tr("gui.action.extract_all", "Extract all");
  }

  function extractSafeTitle(): string {
    return extractJobPaths()
      ? tr("gui.extract.safe_title", "Extract selected files safely")
      : tr("gui.extract.safe_title_all", "Extract every file safely");
  }

  function extractAllToLabel(): string {
    return tr("gui.action.extract_all_to", "Extract all to…");
  }

  function extractSelectedToLabel(): string {
    return tr("gui.action.extract_selected_to", "Extract selected to…");
  }

  function extractPasswordLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (passwordBookStatus.state === "ready" && passwordBookStatus.saved) {
      return tr("gui.password.book_can_unlock", "Password Book can unlock if needed");
    }
    return tr("gui.password.ask_only_if_required", "Ask only if the archive requires it");
  }

  function extractPasswordStatusbarLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (passwordBookStatus.state === "ready" && passwordBookStatus.saved) {
      return tr("gui.extract.password_book_ready_short", "Password Book ready");
    }
    return tr("gui.extract.password_if_needed_short", "Password if needed");
  }

  function extractEncodingLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (extractPresetEncodingLabel) {
      return tr("gui.presets.encoding_override", "Preset: {encoding}")
        .replace("{encoding}", extractPresetEncodingLabel.toUpperCase());
    }
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

  function extractStartBlockedReason(): string {
    const archiveReason = extractArchiveRequiredReason();
    if (archiveReason) return archiveReason;
    if (extractPlanPhase === "blocked") return extractSpaceFailureLabel();
    if (extractPlanPhase === "error") return extractPlanErrorLabel();
    if (extractPlanPhase !== "ready") {
      return tr("gui.extract.plan_wait_before_start", "Wait for the extraction preview to finish");
    }
    return "";
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
      [extractDestinationFieldLabel(), effectiveExtractDest()],
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
    if (extractPlanPhase === "loading") {
      showNotice(tr("gui.extract.plan_wait_before_start", "Wait for the extraction preview to finish"));
      return;
    }
    if (extractPlanPhase === "blocked") {
      showNotice(extractSpaceFailureLabel());
      return;
    }
    if (extractDestinationMode === "choose" && !extractCustomDest.trim()) {
      await chooseExtractDestination();
      if (!extractCustomDest.trim()) return;
    }
    const current = currentArchive;
    if (!current || screen !== "extract") return;
    const selection = extractJobPaths();
    const jobDestination = extractJobDestination();
    const smart = extractDestinationMode === "smart";
    const encoding = extractEncodingForJob();
    const overwrite = extractOverwriteMode;
    const symlinks = extractSymlinkMode;
    const expectedPlanKey = extractPlanKey(
      current.id,
      current.source,
      current.path,
      jobDestination,
      selection,
      smart,
      encoding,
    );
    await requestExtractPlan(
      current.id,
      current.source,
      current.path,
      jobDestination,
      selection,
      smart,
      encoding,
    );
    if (!currentArchive || currentArchive.id !== current.id || screen !== "extract") return;
    const currentPlanKey = extractPlanKey(
      currentArchive.id,
      currentArchive.source,
      currentArchive.path,
      extractJobDestination(),
      extractJobPaths(),
      extractDestinationMode === "smart",
      extractEncodingForJob(),
    );
    if (
      extractPlanRequestKey !== expectedPlanKey ||
      currentPlanKey !== expectedPlanKey ||
      extractOverwriteMode !== overwrite ||
      extractSymlinkMode !== symlinks
    ) {
      showNotice(tr(
        "gui.extract.plan_changed_before_start",
        "Extraction settings changed while the write plan was being checked. Review the updated plan and start again.",
      ));
      return;
    }
    if (extractPlanPhase !== "ready" || !extractPlan) {
      showNotice(extractPlanErrorLabel() || tr(
        "gui.extract.plan_wait_before_start",
        "Wait for the extraction preview to finish",
      ));
      return;
    }
    const destination = effectiveExtractDest();
    const action = selection ? tr("gui.extract.selected_queued", "Selected extract queued") : tr("gui.extract.all_queued", "Extract all queued");
    const success = tr("gui.extract.started_to_destination", "{action} · destination: {destination}")
      .replace("{action}", action)
      .replace("{destination}", destination);
    const queued = await submitCurrentArchiveJob(
      {
        kind: "extract",
        path: currentArchive.source,
        dest: jobDestination,
        expected_destination: extractPlan.destination,
        expected_input_guard: extractPlan.input_guard,
        selection,
        overwrite,
        symlinks,
        smart,
        encoding,
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
    if (blockSelectionScopedAction()) return;
    const selection = selectedJobPaths();
    if (!currentArchive || !selection) {
      showNotice(copyOutSelectedDisabledReason());
      return;
    }
    if (extractDestinationMode === "choose" && !extractCustomDest.trim()) {
      await chooseExtractDestination();
      if (!extractCustomDest.trim()) return;
    }
    const destination = effectiveExtractDest();
    const jobDestination = extractJobDestination();
    const action = tr("gui.copy_out.selected_queued", "Selected copy-out queued");
    const success = tr("gui.copy_out.started_to_destination", "{action} · destination: {destination}")
      .replace("{action}", action)
      .replace("{destination}", destination);
    const queued = await submitCurrentArchiveJob(
      {
        kind: "extract",
        path: currentArchive.source,
        dest: jobDestination,
        selection,
        overwrite: extractOverwriteMode,
        symlinks: extractSymlinkMode,
        smart: extractDestinationMode === "smart",
        encoding: extractEncodingForJob(),
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
        ? [currentArchive.source]
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
          dest: currentArchive?.source === path ? defaultExtractDest() : extractDestForPath(path),
          encoding: currentArchive?.source === path ? archiveEncodingForJob() : null,
          password: null,
          best_effort: false,
        })),
        overwrite: "ask",
        symlinks: "preserve",
        smart: true,
      });
      showNotice(tr("gui.batch.extract_job_queued", "{count} archives added as one task").replace("{count}", paths.length.toLocaleString()));
      recordOperation({
        status: "queued",
        title: tr("gui.batch.extract_queued", "Batch extract queued"),
        detail: tr("gui.batch.archive_count", "{count} archives").replace("{count}", paths.length.toLocaleString()),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.batch.requires_desktop_service", "Batch extract requires the desktop service"));
    }
  }

  function batchWorkspaceSurface(variant: ToolsWorkspaceVariant): BatchWorkspaceSurface {
    return {
      kind: "batch",
      variant,
      title: tr("gui.screen.batch", "Batch Extract Review"),
      tr,
      archiveReturn: toolsArchiveReturnSurface(variant),
      rows: batchReviewArchives().map((row) => ({
        name: row.name,
        format: row.format,
        entries: row.entries,
        target: row.target,
        state: batchArchiveStateLabel(row.state),
        warning: row.state === "Needs password",
      })),
      warningLabel: batchWarningLabel(),
      emptyLabel: openArchiveFirstLabel(),
      actions: {
        onStart: () => void startBatchExtract(),
        onBack: () => setScreen("extract"),
        onResolvePassword: () => setScreen("password"),
      },
    };
  }

  function toolsArchiveReturnSurface(variant: ToolsWorkspaceVariant) {
    return {
      visible: variant === "classic" && showArchiveReturnBar(),
      title: archiveTitle(),
      detail: archiveReturnDetail(),
      contextLabel: tr("gui.archive.current_context", "Current archive"),
      actionLabel: tr("gui.archive.back_to_current", "Back to current archive"),
      onReturn: returnToCurrentArchive,
    };
  }

  function checksumWorkspaceSurface(variant: ToolsWorkspaceVariant): ChecksumWorkspaceSurface {
    const checksumRows = checksumItems("checksum").slice(0, 20).map((item) => {
      const path = checksumItemText(item, "path");
      return {
        name: pathBaseName(path) || path,
        size: formatBytes(checksumItemNumber(item, "size")),
        digest: checksumItemText(item, "digest"),
        status: checksumItemStatus(item),
      };
    });
    const verificationRows = checksumItems("checksum_check").slice(0, 20).map((item) => {
      const path = checksumItemText(item, "path");
      return {
        name: pathBaseName(path) || path,
        expected: checksumItemText(item, "expected"),
        actual: checksumItemText(item, "actual") || checksumItemText(item, "error"),
        status: checksumItemStatus(item),
      };
    });
    const currentArchiveDisabledReason = checksumCurrentArchiveDisabledReason();

    return {
      kind: "checksum",
      variant,
      title: tr("gui.screen.checksum", "Checksum"),
      tr,
      archiveReturn: toolsArchiveReturnSurface(variant),
      target: {
        name: checksumTargetName(),
        label: checksumTargetLabel(),
        currentArchiveDisabledReason,
      },
      algorithm: {
        options: checksumAlgorithms,
        selected: checksumAlgorithm,
        label: checksumAlgorithmLabel(checksumAlgorithm),
        labelFor: checksumAlgorithmLabel,
        hintFor: checksumAlgorithmHint,
        onSelect: selectChecksumAlgorithm,
      },
      metrics: {
        filesHashed: checksumResultNumber("checksum", "files_hashed").toLocaleString(),
        bytesHashed: formatBytes(checksumResultNumber("checksum", "bytes_hashed")),
        passed: checksumResultNumber("checksum_check", "passed").toLocaleString(),
        checked: checksumResultNumber("checksum_check", "checked").toLocaleString(),
        failed: checksumResultNumber("checksum_check", "failed").toLocaleString(),
      },
      excludes: {
        value: checksumExcludeText,
        rules: checksumExcludeRules(),
        countLabel: tr("gui.excludes.count", "{count} rules")
          .replace("{count}", String(checksumExcludeRules().length)),
        onInput: (value) => (checksumExcludeText = value),
      },
      manifestLabel: checksumManifestLabel(),
      result: {
        rows: checksumRows,
        state: taskStateLabel(latestChecksumTask("checksum")?.state),
        feedback: checksumCopyFeedbackFor("checksum"),
        feedbackDanger: checksumCopyFeedbackToneFor("checksum") === "danger",
        onCopy: () => void copyChecksumResults("checksum"),
      },
      verification: {
        rows: verificationRows,
        state: taskStateLabel(latestChecksumTask("checksum_check")?.state),
        feedback: checksumCopyFeedbackFor("checksum_check"),
        feedbackDanger: checksumCopyFeedbackToneFor("checksum_check") === "danger",
        onCopy: () => void copyChecksumResults("checksum_check"),
      },
      actions: {
        onChooseFile: () => void chooseChecksumFile(),
        onChooseFolder: () => void chooseChecksumFolder(),
        onUseCurrentArchive: useCurrentArchiveForChecksum,
        onCalculate: () => void submitChecksumJob(),
        onChooseManifest: () => void chooseChecksumManifest(),
        onVerifyManifest: () => void submitChecksumCheckJob(),
        onOpenDuplicates: () => setScreen("duplicates"),
        onOpenRecovery: () => setScreen("recovery"),
      },
      onPanelMount: (kind: ChecksumResultKind, node: HTMLElement | null) => {
        if (kind === "checksum") {
          checksumResultPanel = node;
        } else {
          checksumCheckResultPanel = node;
        }
      },
    };
  }

  function duplicatesWorkspaceSurface(variant: ToolsWorkspaceVariant): DuplicatesWorkspaceSurface {
    const duplicateGroups = duplicateResultNumber("duplicate_groups");

    return {
      kind: "duplicates",
      variant,
      title: tr("gui.screen.duplicates", "Duplicate Finder"),
      tr,
      archiveReturn: toolsArchiveReturnSurface(variant),
      target: {
        name: duplicateScanTargetName(),
        label: duplicateScanTargetLabel(),
      },
      minimumSize: {
        value: duplicateMinSize,
        label: formatBytes(duplicateMinSize),
        error: duplicateMinSizeError,
        onInput: updateDuplicateMinSizeFromInput,
      },
      excludes: {
        value: duplicateExcludeText,
        rules: duplicateExcludeRules(),
        countLabel: tr("gui.excludes.count", "{count} rules")
          .replace("{count}", String(duplicateExcludeRules().length)),
        onInput: (value) => (duplicateExcludeText = value),
      },
      metrics: {
        filesScanned: duplicateResultNumber("files_scanned").toLocaleString(),
        bytesScanned: formatBytes(duplicateResultNumber("bytes_scanned")),
        candidateFiles: duplicateResultNumber("candidate_files").toLocaleString(),
        hashedBytes: formatBytes(duplicateResultNumber("hashed_bytes")),
        duplicateFiles: duplicateResultNumber("duplicate_files").toLocaleString(),
        duplicateGroups: duplicateGroups.toLocaleString(),
        reclaimable: formatBytes(duplicateResultNumber("reclaimable_bytes")),
        taskState: taskStateLabel(latestDuplicateScanTask()?.state),
        reviewState: duplicateGroups > 0
          ? tr("gui.duplicates.review_only", "Review only")
          : tr("gui.duplicates.clean", "Clean"),
      },
      actions: {
        onChooseFolder: () => void chooseDuplicateScanFolder(),
        onUseArchiveFolder: useCurrentArchiveFolderForDuplicates,
        onScan: () => void submitDuplicateScanJob(),
        onOpenCreate: () => setScreen("create"),
        onOpenBatch: () => setScreen("batch"),
      },
    };
  }

  function checksumTarget(): string {
    if (checksumPath.trim()) return checksumPath.trim();
    return currentArchive && !currentArchive.read_only ? currentArchive.path : "";
  }

  function checksumCurrentArchiveDisabledReason(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return currentArchive.read_only
      ? tr("gui.checksum.nested_requires_extract", "Extract the inner archive before checksumming its container file.")
      : "";
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
    return copyTextToClipboard(text);
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
    const disabledReason = checksumCurrentArchiveDisabledReason();
    if (disabledReason) {
      showNotice(disabledReason);
      return;
    }
    if (!currentArchive) return;
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
      showNotice(tr("gui.checksum.queued", "Checksum added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.checksum.queued", "Checksum added to queue"),
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
      showNotice(tr("gui.checksum.verification_queued", "Checksum verification added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.checksum.verification_queued", "Checksum verification added to queue"),
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
      showNotice(tr("gui.duplicates.queued", "Duplicate scan added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.duplicates.queued", "Duplicate scan added to queue"),
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
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (!source || !jobSource) {
      showNotice(tr("gui.recovery.choose_archive_before_best_effort", "Choose an archive before extracting readable files."));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    const selection = recoverySelectionForJob();
    const dest = `${extractDestForPath(source)}-readable`;
    recoverySubmissionPending = true;
    try {
      const id = await submitJob({
        kind: "extract",
        path: jobSource,
        dest,
        selection,
        overwrite: "rename",
        symlinks: "preserve",
        smart: true,
        encoding: recoveryEncodingForJob(),
        password: null,
        best_effort: true,
      });
      recoveryContextTaskIds.add(id);
      const title = selection
        ? tr("gui.recovery.best_effort_selected_queued", "Selected best-effort extract added to queue")
        : tr("gui.recovery.best_effort_queued", "Best-effort extract added to queue");
      showNotice(title);
      recordOperation({
        status: "queued",
        title,
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.best_effort_requires_desktop_service", "Extracting readable files requires the desktop service."));
    } finally {
      recoverySubmissionPending = false;
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
        path: currentArchive.source,
        encoding: archiveEncodingForJob(),
        password: null,
      },
      tr("gui.test.queued", "Archive test added to queue"),
      tr("gui.precondition.open_before_test", "Open an archive before testing"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.test.queued", "Archive test added to queue"),
        detail: archiveTitle(),
      });
    }
  }

  async function submitRecoveryTestJob() {
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (!source || !jobSource) {
      showNotice(tr("gui.recovery.choose_archive_before_test", "Choose an archive before testing"));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    recoverySubmissionPending = true;
    try {
      const id = await submitJob({
        kind: "test",
        path: jobSource,
        encoding: recoveryEncodingForJob(),
        password: null,
      });
      recoveryContextTaskIds.add(id);
      showNotice(tr("gui.recovery.archive_test_started", "Archive test added to the queue from Recovery."));
      recordOperation({
        status: "queued",
        title: tr("gui.test.queued", "Archive test added to queue"),
        detail: pathBaseName(source),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.archive_test_requires_desktop_service", "Archive testing requires the desktop service."));
    } finally {
      recoverySubmissionPending = false;
    }
  }

  async function submitExportSqzJob() {
    const disabledReason = recoverySqzExportDisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (disabledReason || !source || !jobSource) {
      showNotice(disabledReason || tr("gui.recovery.choose_sqz_before_export", "Choose an SQZ archive before exporting."));
      return;
    }
    if (outputAuthorizationPending || recoverySubmissionPending || focusBlockingTaskIfAny()) return;
    outputAuthorizationPending = true;
    recoverySubmissionPending = true;
    try {
      const { confirm, save } = await getDialogModule();
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
      const authorization = await authorizeArchiveOutput(dest, confirm);
      if (!authorization) {
        showNotice(tr("gui.recovery.export_sqz_cancelled", "Export SQZ cancelled"));
        return;
      }
      await submitJob({
        kind: "export_sqz",
        src: jobSource,
        dest,
        level: createCompressionLevel(),
        dest_password: null,
        replace_existing: authorization.replaceExisting,
        replacement_guard: authorization.replacementGuard,
      });
      showNotice(tr("gui.recovery.sqz_export_queued", "SQZ export added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.sqz_export_queued", "SQZ export added to queue"),
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)} · ${createProfileLabel(activeCreateProfile)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      if (createDestinationInspectionCancelled(error)) {
        showNotice(tr("gui.recovery.export_sqz_cancelled", "Export SQZ cancelled"));
      } else if (error instanceof CreateDestinationInspectionError) {
        showNotice(
          error.detail
            ? tError(error.detail)
            : tr("gui.output.inspect_failed", "Could not check the output. Review the destination and try again."),
        );
      } else {
        showNotice(tr("gui.recovery.export_sqz_requires_desktop_service", "Export SQZ requires the desktop service"));
      }
    } finally {
      outputAuthorizationPending = false;
      recoverySubmissionPending = false;
    }
  }

  async function submitRepairSqzJob() {
    const disabledReason = recoverySqzRepairDisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (disabledReason || !source || !jobSource) {
      showNotice(disabledReason || tr("gui.recovery.choose_sqz_before_repair", "Choose an SQZ archive or SQZ volume before repairing."));
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
      if (sameFilePath(dest, source)) {
        showNotice(tr("gui.recovery.repair_output_must_differ", "Choose a new file for the repaired copy. The source archive will not be overwritten."));
        return;
      }
      await submitJob({
        kind: "repair_sqz",
        src: jobSource,
        dest,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.recovery.sqz_repair_queued", "SQZ repair added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.sqz_repair_queued", "SQZ repair added to queue"),
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.repair_sqz_requires_desktop_service", "Repair SQZ requires the desktop service"));
    }
  }

  async function submitRepairZipJob() {
    const disabledReason = recoveryZipDisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (disabledReason || !source || !jobSource) {
      showNotice(disabledReason || tr("gui.recovery.choose_archive_before_zip_rebuild", "Choose a ZIP-family archive before rebuilding its index."));
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
      if (sameFilePath(dest, source)) {
        showNotice(tr("gui.recovery.repair_output_must_differ", "Choose a new file for the repaired copy. The source archive will not be overwritten."));
        return;
      }
      await submitJob({
        kind: "repair_zip",
        src: jobSource,
        dest,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.recovery.zip_rebuild_queued", "ZIP index rebuild added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.zip_rebuild_queued", "ZIP index rebuild added to queue"),
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.zip_rebuild_requires_desktop_service", "ZIP index rebuild requires the desktop service"));
    }
  }

  async function submitProtectJob() {
    const disabledReason = recoveryProtectDisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    const redundancy = recoveryRedundancyValue();
    if (disabledReason || !source || !jobSource || redundancy === null) {
      showNotice(disabledReason || tr("gui.recovery.choose_archive_before_protect", "Choose an archive before creating PAR2 recovery data."));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "protect",
        path: jobSource,
        redundancy,
        recovery: null,
      });
      showNotice(tr("gui.recovery.par2_protection_queued", "PAR2 protection added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.par2_protection_queued", "PAR2 protection added to queue"),
        detail: pathBaseName(source),
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.protect_requires_desktop_service", "Creating PAR2 recovery data requires the desktop service."));
    }
  }

  async function submitVerifyRecoveryJob() {
    const disabledReason = recoveryVerifyDisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (disabledReason || !source || !jobSource) {
      showNotice(disabledReason || tr("gui.recovery.choose_archive_before_verify", "Choose the archive described by this PAR2 file."));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      await submitJob({
        kind: "verify_recovery",
        path: jobSource,
        recovery: recoveryPar2Override,
      });
      showNotice(tr("gui.recovery.par2_verify_queued", "PAR2 verification added to queue"));
      recordOperation({
        status: "queued",
        title: tr("gui.recovery.par2_verify_queued", "PAR2 verification added to queue"),
        detail: `${pathBaseName(source)} · ${pathBaseName(recoveryPar2Path() ?? "")}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.verify_requires_desktop_service", "Verifying PAR2 recovery data requires the desktop service."));
    }
  }

  async function submitRepairRecoveryJob() {
    const disabledReason = recoveryRepairPar2DisabledReason();
    const source = recoverySourcePath();
    const jobSource = recoverySourceForJob();
    if (disabledReason || !source || !jobSource) {
      showNotice(disabledReason || tr("gui.recovery.choose_archive_before_par2_repair", "Choose an archive before repairing with PAR2 data."));
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    try {
      const dialogs = await getDialogModule();
      const outputDirectory = recoveryRepairUsesDirectory();
      let dest: string | null = null;
      if (outputDirectory) {
        const selected = await openNativeDialog("recovery.repair-par2-set-parent", dialogs.open, {
          title: tr(
            "gui.recovery.choose_repaired_set_parent",
            "Choose where to create the repaired set folder",
          ),
          defaultPath: pathDir(source),
          multiple: false,
          directory: true,
        });
        if (typeof selected === "string") {
          const proposed = joinFolderPath(selected, defaultPar2RepairDirectoryName());
          dest = await ipc.uniqueCreateDestination(proposed, false);
        }
      } else {
        const extension = archiveExtensionMatch(pathBaseName(source));
        dest = await saveNativeDialog("recovery.repair-par2", dialogs.save, {
          title: tr("gui.recovery.repair_par2_as", "Repair with PAR2 as"),
          defaultPath: defaultPar2RepairDest(),
          filters: extension ? [{ name: tr("gui.recovery.repaired_archive_filter", "Repaired archive"), extensions: [extension] }] : [],
        });
      }
      if (!dest) {
        showNotice(tr("gui.recovery.repair_par2_cancelled", "PAR2 repair cancelled"));
        return;
      }
      if (sameFilePath(dest, source)) {
        showNotice(tr("gui.recovery.repair_output_must_differ", "Choose a new file for the repaired copy. The source archive will not be overwritten."));
        return;
      }
      await submitJob({
        kind: "repair_recovery",
        path: jobSource,
        output: dest,
        output_directory: outputDirectory,
        recovery: recoveryPar2Override,
      });
      showNotice(
        outputDirectory
          ? tr("gui.recovery.par2_set_repair_queued", "PAR2 set repair added to queue")
          : tr("gui.recovery.par2_repair_queued", "PAR2 repair added to queue"),
      );
      recordOperation({
        status: "queued",
        title: outputDirectory
          ? tr("gui.recovery.par2_set_repair_queued", "PAR2 set repair added to queue")
          : tr("gui.recovery.par2_repair_queued", "PAR2 repair added to queue"),
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}`,
      });
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      showNotice(tr("gui.recovery.repair_par2_requires_desktop_service", "PAR2 repair requires the desktop service"));
    }
  }

  async function submitAddToArchiveJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_add", "Open an archive before adding files"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
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
        path: currentArchive.source,
        add,
        delete: [],
        rename: [],
        mkdir: [],
        excludes: createContentPolicy === "custom" ? createExcludeRules() : [],
        content_policy: createContentPolicy,
        password: null,
        level: createCompressionLevel(),
      });
      showNotice(tr("gui.add.operations_queued", "{count} add operations queued").replace("{count}", add.length.toLocaleString()));
      recordOperation({
        status: "queued",
        title: tr("gui.add.queued", "Add files queued"),
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
    if (blockSelectionScopedAction()) return;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_delete", "Open an archive before deleting entries"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
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
        path: currentArchive.source,
        add: [],
        delete: patterns,
        rename: [],
        mkdir: [],
        excludes: [],
        password: null,
        level: 6,
      },
      (patterns.length === 1
        ? tr("gui.delete.operation_queued", "1 delete operation queued")
        : tr("gui.delete.operations_queued", "{count} delete operations queued").replace("{count}", patterns.length.toLocaleString())),
      tr("gui.precondition.open_before_delete", "Open an archive before deleting entries"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.delete.queued", "Delete entries queued"),
        detail: tr("gui.delete.entries_from_archive", "{count} entries from {archive}")
          .replace("{count}", patterns.length.toLocaleString())
          .replace("{archive}", archiveTitle()),
      });
    }
  }

  async function submitRenameSelectedJob() {
    if (blockSelectionScopedAction()) return;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_rename", "Open an archive before renaming entries"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
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
        path: currentArchive.source,
        add: [],
        delete: [],
        rename: [{ from, to }],
        mkdir: [],
        excludes: [],
        password: null,
        level: 6,
      },
      tr("gui.rename.queued_notice", "Rename queued: {from} -> {to}").replace("{from}", from).replace("{to}", to),
      tr("gui.precondition.open_before_rename", "Open an archive before renaming entries"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.rename.queued", "Rename entry queued"),
        detail: `${from} -> ${to}`,
      });
    }
  }

  async function submitMoveSelectedJob() {
    if (blockSelectionScopedAction()) return;
    const targetDir = normalizeMoveTargetDir();
    moveTargetDir = targetDir;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_move", "Open an archive before moving entries"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
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
    if (blockSelectionScopedAction()) return;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_move", "Open an archive before moving entries"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
      return;
    }
    if (rename.length === 0) {
      showNotice(tr("gui.move.no_non_conflicting_targets", "No non-conflicting move targets to submit"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.source,
        add: [],
        delete: [],
        rename,
        mkdir: [targetDir],
        excludes: [],
        password: null,
        level: 6,
      },
      (rename.length === 1
        ? tr("gui.move.operation_queued", "1 move operation queued")
        : tr("gui.move.operations_queued", "{count} move operations queued").replace("{count}", rename.length.toLocaleString())),
      tr("gui.precondition.open_before_move", "Open an archive before moving entries"),
    );
    if (queued) {
      moveConflictReview = null;
      recordOperation({
        status: "queued",
        title: tr("gui.move.queued", "Move entries queued"),
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
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
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
        path: currentArchive.source,
        add: [],
        delete: [],
        rename: [],
        mkdir: [folder],
        excludes: [],
        password: null,
        level: 6,
      },
      tr("gui.new_folder.queued_notice", "New folder queued: {folder}").replace("{folder}", folder),
      tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.new_folder.queued", "New folder queued"),
        detail: folder,
      });
    }
  }

  async function openArchiveDirectoryEntry(entryPath: string) {
    const name = pathBaseName(entryPath.replace(/\/+$/g, ""));
    if (!name) {
      showNotice(tr("gui.preview.open_folder_failed", "Could not open the folder"));
      return;
    }
    const targetDirectory = entryPath
      .replaceAll("\\", "/")
      .replace(/^\/+|\/+$/g, "");
    clearEntryPreviewState();
    clearSelection();
    await enterDirPath(entryPath);
    if (archiveBrowseError() || archiveDirs.join("/") !== targetDirectory) return;
    browseScrollTop = 0;
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
    virtualIndex: number | null = null,
  ) {
    if (!currentArchive) {
      showNotice(tr("gui.preview.open_archive_first", "Open an archive before opening or previewing entries"));
      return;
    }
    if (!entryPath) {
      showNotice(tr("gui.preview.select_one", "Select one entry to open or preview"));
      return;
    }
    if (entryType === "dir" || entryPath.endsWith("/") || entryTypeForPath(entryPath) === "dir") {
      await openArchiveDirectoryEntry(entryPath);
      return;
    }
    previewOriginEntryPath = entryPath;
    previewOriginVirtualIndex = virtualIndex;
    if (archiveLikePath(entryPath)) {
      await submitPreviewNestedArchive(entryPath, virtualIndex);
      return;
    }
    const archivePath = currentArchive.path;
    const archiveSource = currentArchive.source;
    const preparedEntry = entryPreviewForPath(entryPath);
    if (preparedEntry) {
      previewPhase = "entry";
      previewTargetName = preparedEntry.display_name;
      const opened = await openEntryPreview(preparedEntry);
      if (entryPreview?.preview_id === preparedEntry.preview_id) {
        previewPhase = "idle";
        previewTargetName = "";
      }
      if (opened) clearEntryPreviewState();
      return;
    }
    clearEntryPreviewState();
    previewOriginEntryPath = entryPath;
    previewOriginVirtualIndex = virtualIndex;
    const requestGeneration = ++previewRequestGeneration;
    previewPhase = "entry";
    previewTargetName = previewEntryDisplayName(entryPath);
    recordValidationEvent("frontend.entry.preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      if (requestGeneration !== previewRequestGeneration) return;
      const preparedPreview = await prepareEntryPreviewSerially(
        archiveSource,
        archivePath,
        entryPath,
        requestGeneration,
      );
      if (!preparedPreview) return;
      if (requestGeneration !== previewRequestGeneration) {
        void disposeEntryPreview(preparedPreview.preview_id);
        return;
      }
      entryPreview = preparedPreview;
      nestedPreview = null;
      entryPreviewFailure = null;
      recordValidationEvent("frontend.entry.preview_loaded", {
        entry_path: entryPath,
        display_name: entryPreview.display_name,
      });
      showNotice(
        tr("gui.preview.opening_system", "Opening: {name}")
          .replace("{name}", entryPreview.display_name),
      );
      if (!await openEntryPreview(entryPreview)) return;
      if (requestGeneration !== previewRequestGeneration) return;
      recordOperation({
        status: "info",
        title: tr("gui.preview.operation_title", "Archive entry opened"),
        detail: pathBaseName(entryPath),
      });
      clearEntryPreviewState();
    } catch (error) {
      if (requestGeneration !== previewRequestGeneration) return;
      const failurePolicy = previewPolicyFor(entryPath, entryType ?? entryTypeForPath(entryPath));
      const message = previewFailureMessage(
        error,
        false,
        "gui.preview.failed",
        "Could not open this item",
      );
      entryPreviewFailure = {
        entryPath,
        entryType: entryType ?? entryTypeForPath(entryPath),
        displayName: previewEntryDisplayName(entryPath),
        policyKind: failurePolicy.kind,
        outerSource: archiveSource,
        outerDisplayPath: archivePath,
        message,
        retryAction: "preview",
      };
      recordValidationEvent("frontend.entry.preview_failed", {
        entry_path: entryPath,
        policy_kind: failurePolicy.kind,
      });
      showNotice(message);
    } finally {
      if (requestGeneration === previewRequestGeneration) {
        previewPhase = "idle";
        previewTargetName = "";
      }
    }
  }

  async function submitPreviewNestedArchive(
    entryPath: string | null = selectedPreviewPath(),
    virtualIndex: number | null = null,
  ) {
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
    clearEntryPreviewState();
    previewOriginEntryPath = entryPath;
    previewOriginVirtualIndex = virtualIndex;
    const requestGeneration = ++previewRequestGeneration;
    const archiveSource = currentArchive.source;
    const archiveDisplayPath = currentArchive.path;
    previewPhase = "nested";
    previewTargetName = pathBaseName(entryPath);
    recordValidationEvent("frontend.entry.nested_preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      if (requestGeneration !== previewRequestGeneration) return;
      const preparedPreview = await ipc.previewNestedArchive(
        archiveSource,
        entryPath,
        null,
        archiveEncodingForJob(),
      );
      if (requestGeneration !== previewRequestGeneration) return;
      nestedPreview = preparedPreview;
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
    } catch (error) {
      if (requestGeneration !== previewRequestGeneration) return;
      const failurePolicy = previewPolicyFor(entryPath, entryTypeForPath(entryPath));
      const message = previewFailureMessage(
        error,
        true,
        "gui.preview.nested_failed",
        "Could not preview this nested archive",
      );
      entryPreviewFailure = {
        entryPath,
        entryType: entryTypeForPath(entryPath),
        displayName: pathBaseName(entryPath),
        policyKind: failurePolicy.kind,
        outerSource: archiveSource,
        outerDisplayPath: archiveDisplayPath,
        message,
        retryAction: "preview",
      };
      showNotice(message);
    } finally {
      if (requestGeneration === previewRequestGeneration) {
        previewPhase = "idle";
        previewTargetName = "";
      }
    }
  }

  async function revealEntryPreview() {
    const preview = entryPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_first", "Open a file entry first"));
      return;
    }
    const responseIdentity: PreviewResponseIdentity = {
      previewGeneration: previewRequestGeneration,
      actionGeneration: ++previewActionGeneration,
      previewId: preview.preview_id,
      archiveSource: currentArchive?.source ?? null,
    };
    const responseIsCurrent = () => previewResponseIsCurrent(responseIdentity, {
      previewGeneration: previewRequestGeneration,
      actionGeneration: previewActionGeneration,
      previewId: entryPreview?.preview_id ?? null,
      archiveSource: currentArchive?.source ?? null,
    });
    try {
      await ipc.revealPreviewSession(preview.preview_id);
      if (!responseIsCurrent()) return;
      showNotice(
        tr("gui.preview.revealed_system", "Shown in the file manager: {name}")
          .replace("{name}", preview.display_name),
      );
    } catch (error) {
      if (!responseIsCurrent()) return;
      showNotice(previewFailureMessage(
        error,
        false,
        "gui.preview.reveal_failed",
        "Cannot reveal the prepared file in the file manager",
      ));
    }
  }

  async function openEntryPreview(preview: EntryPreviewDto | null = entryPreview): Promise<boolean> {
    if (!preview) {
      showNotice(tr("gui.preview.preview_first", "Open a file entry first"));
      return false;
    }
    const responseIdentity: PreviewResponseIdentity = {
      previewGeneration: previewRequestGeneration,
      actionGeneration: ++previewActionGeneration,
      previewId: preview.preview_id,
      archiveSource: currentArchive?.source ?? null,
    };
    const outerDisplayPath = currentArchive?.path ?? preview.outer_path;
    const responseIsCurrent = () => previewResponseIsCurrent(responseIdentity, {
      previewGeneration: previewRequestGeneration,
      actionGeneration: previewActionGeneration,
      previewId: entryPreview?.preview_id ?? null,
      archiveSource: currentArchive?.source ?? null,
    });
    if (previewSystemOpenRequiresConfirmation(preview.entry_path)) {
      let confirmed: boolean;
      try {
        const { confirm } = await getDialogModule();
        confirmed = await confirm(
          tr(
            "gui.preview.risky_open_body",
            "Open “{name}” with the system? Files from archives can run code. Extract and inspect it first unless you trust the source.",
          ).replace("{name}", preview.display_name),
          {
            title: tr("gui.preview.risky_open_title", "Open a potentially executable file?"),
            kind: "warning",
            okLabel: tr("gui.preview.risky_open_action", "Open anyway"),
            cancelLabel: tr("gui.preview.risky_open_cancel", "Cancel"),
          },
        );
      } catch {
        if (!responseIsCurrent()) return false;
        showNotice(tr(
          "gui.preview.risky_open_confirm_failed",
          "The safety confirmation could not be shown, so the file was not opened.",
        ));
        return false;
      }
      if (!responseIsCurrent()) return false;
      if (!confirmed) {
        showNotice(
          tr(
            "gui.preview.risky_open_cancelled",
            "Potentially executable file was not opened: {name}",
          ).replace("{name}", preview.display_name),
        );
        return false;
      }
    }
    try {
      await ipc.openPreviewSession(preview.preview_id);
      if (!responseIsCurrent()) return false;
      entryPreviewFailure = null;
      showNotice(tr("gui.preview.opened_system", "Opened: {name}").replace("{name}", preview.display_name));
      return true;
    } catch (error) {
      if (!responseIsCurrent()) return false;
      const message = previewFailureMessage(
        error,
        false,
        "gui.preview.open_failed_ready",
        "Could not open this item. Try again or extract it instead.",
      ).replace("{name}", preview.display_name);
      const entryType = entryTypeForPath(preview.entry_path);
      entryPreviewFailure = {
        entryPath: preview.entry_path,
        entryType,
        displayName: preview.display_name,
        policyKind: previewPolicyFor(preview.entry_path, entryType).kind,
        outerSource: preview.outer_path,
        outerDisplayPath,
        message,
        retryAction: "preview",
      };
      showNotice(message);
      return false;
    }
  }

  async function focusArchiveRow(index: number, entryPath: string | null): Promise<void> {
    if (screen !== "browse" || !currentArchive) return;
    const archiveSource = currentArchive.source;
    const directoryKey = archiveDirs.join("\u0000");
    const filterKey = filterText();
    const listMode = mode;
    const listKind = listMode === "classic" ? "classic" : "modern";
    const selector = `[data-row-index="${index}"]`;
    const contextIsCurrent = () => (
      screen === "browse" &&
      currentArchive?.source === archiveSource &&
      archiveDirs.join("\u0000") === directoryKey &&
      filterText() === filterKey &&
      mode === listMode
    );
    const rowMatches = () => !entryPath || rowAt(index)?.path === entryPath;

    await tick();
    if (!contextIsCurrent()) return;
    let list = document.querySelector<HTMLElement>(`[data-virtual-list="${listKind}"]`);
    if (!list) return;
    let row = list.querySelector<HTMLElement>(selector);
    if (!row) {
      const rowHeight = listMode === "classic" ? CLASSIC_ROW_HEIGHT : MODERN_ROW_HEIGHT;
      const centeredOffset = Math.max(0, (list.clientHeight - rowHeight) / 2);
      const nextScrollTop = Math.max(0, index * rowHeight - centeredOffset);
      list.scrollTop = nextScrollTop;
      browseScrollTop = nextScrollTop;
      browseViewportHeight = list.clientHeight;
      await loadRowAt(index);
      await tick();
      if (!contextIsCurrent()) return;
      list = document.querySelector<HTMLElement>(`[data-virtual-list="${listKind}"]`);
      row = list?.querySelector<HTMLElement>(selector) ?? null;
    }
    if (!contextIsCurrent() || !rowMatches()) return;
    row?.focus({ preventScroll: true });
  }

  async function openNestedArchiveEntry(
    outerPath: string,
    entryPath: string,
    virtualIndex: number | null = previewOriginVirtualIndex,
  ) {
    clearEntryPreviewState();
    previewOriginEntryPath = entryPath;
    previewOriginVirtualIndex = virtualIndex;
    const requestGeneration = ++previewRequestGeneration;
    const encoding = currentArchive?.source === outerPath ? archiveEncodingForJob() : null;
    previewPhase = "nested";
    previewTargetName = pathBaseName(entryPath);
    try {
      await waitForPreviewFeedbackFrame();
      if (requestGeneration !== previewRequestGeneration) return;
      const info = await ipc.openNestedArchive(
        outerPath,
        entryPath,
        null,
        encoding,
      );
      if (requestGeneration !== previewRequestGeneration) {
        void ipc.closeArchive(info.id).catch(() => undefined);
        return;
      }
      if (!await adoptOpenedArchive(info)) return;
      recoverySourceMode = "current";
      recoverySourceOverride = null;
      recoveryPar2Override = null;
      clearEntryPreviewState();
      showNotice(tr("gui.preview.opened_nested_archive", "Opened nested archive · {name}").replace("{name}", info.name));
      recordOperation({
        status: "done",
        title: tr("gui.preview.nested_opened_operation_title", "Nested archive opened"),
        detail: `${pathBaseName(entryPath)} -> ${info.name}`,
      });
    } catch (error) {
      if (requestGeneration !== previewRequestGeneration) return;
      const failurePolicy = previewPolicyFor(entryPath, entryTypeForPath(entryPath));
      const outerDisplayPath = currentArchive?.source === outerPath
        ? currentArchive.path
        : outerPath;
      const message = previewFailureMessage(
        error,
        true,
        "gui.preview.open_nested_requires_desktop_service",
        "Could not open this inner archive. Preview it or extract it instead.",
      );
      entryPreviewFailure = {
        entryPath,
        entryType: entryTypeForPath(entryPath),
        displayName: pathBaseName(entryPath),
        policyKind: failurePolicy.kind,
        outerSource: outerPath,
        outerDisplayPath,
        message,
        retryAction: "open",
      };
      showNotice(message);
    } finally {
      if (requestGeneration === previewRequestGeneration) {
        previewPhase = "idle";
        previewTargetName = "";
      }
    }
  }

  async function openNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_open", "Preview a nested archive before opening it"));
      return;
    }
    await openNestedArchiveEntry(preview.outer_path, preview.entry_path);
  }

  async function submitNestedExtract(
    outerSource: string,
    outerDisplayPath: string,
    entryPath: string,
  ): Promise<boolean> {
    if (focusBlockingTaskIfAny()) return false;
    try {
      const dest = nestedExtractDest(outerDisplayPath, entryPath);
      await submitJob({
        kind: "extract_nested",
        outer_path: outerSource,
        entry_path: entryPath,
        dest,
        overwrite: "ask",
        symlinks: "preserve",
        smart: true,
        encoding: currentArchive?.source === outerSource ? archiveEncodingForJob() : null,
        password: null,
        best_effort: false,
      });
      showNotice(tr("gui.preview.extract_nested_queued", "Nested extract added to queue · {name}").replace("{name}", pathBaseName(entryPath)));
      recordOperation({
        status: "queued",
        title: tr("gui.preview.nested_extract_queued_operation_title", "Nested archive extract queued"),
        detail: `${pathBaseName(entryPath)} -> ${pathBaseName(dest)}`,
      });
      return true;
    } catch (error) {
      if (isJobSubmitBlocked(error)) return false;
      showNotice(tr("gui.preview.extract_nested_requires_desktop_service", "Could not extract this inner archive. Check that its format and password are supported."));
      return false;
    }
  }

  async function extractNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_extract", "Preview a nested archive before extracting it"));
      return;
    }
    const outerDisplayPath = currentArchive?.source === preview.outer_path
      ? currentArchive.path
      : preview.outer_path;
    await submitNestedExtract(preview.outer_path, outerDisplayPath, preview.entry_path);
  }

  async function repairFilenameEncoding(encoding = "gbk") {
    if (!currentArchive) {
      showNotice(tr("gui.encoding.open_before_repair", "Open an archive before repairing filename encoding"));
      return;
    }
    const ok = await reopenWithEncoding(encoding);
    if (ok) {
      markExtractPresetDraftTouched();
      extractPresetEncodingLabel = null;
    }
    showNotice(
      ok
        ? tr("gui.encoding.reopened_with", "Filename encoding reopened with {encoding}").replace("{encoding}", encoding.toUpperCase())
        : tr("gui.encoding.reopen_failed", "Could not reopen archive with that encoding"),
    );
  }

  async function submitCreateJob(sourceKind: "files" | "folder") {
    if (focusBlockingTaskIfAny()) return;
    if (createSourcesLocked()) {
      showNotice(createSourcesLockedReason());
      return;
    }
    createSourcePickerBusy = sourceKind;
    showNotice(sourceKind === "files" ? tr("gui.create.opening_file_picker", "Opening file picker...") : tr("gui.create.opening_folder_picker", "Opening folder picker..."));
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog(`create.${sourceKind}`, open, {
        title: sourceKind === "files" ? tr("gui.create.choose_files_to_archive", "Choose files to archive") : tr("gui.create.choose_folder_to_archive", "Choose folder to archive"),
        multiple: true,
        directory: sourceKind === "folder",
      });
      const inputs = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (inputs.length === 0) {
        showNotice(
          tr(
            "gui.create.sources.picker_cancelled",
            "Source selection cancelled · the current list was kept",
          ),
        );
        return;
      }
      showCreateSourcesAdded(
        appendCreateSources(inputs, sourceKind === "files" ? "file" : "folder"),
      );
    } catch {
      showNotice(
        tr("gui.create.requires_desktop_dialog", "Create archive requires the desktop file dialog"),
      );
    } finally {
      createSourcePickerBusy = null;
    }
  }

  async function submitCreateInputs(inputs: string[], source: "dialog" | "drop", capturedDraft?: CreateRunDraft) {
    if (!capturedDraft && createConfigurationPending()) {
      showNotice(createConfigurationPendingMessage());
      return;
    }
    if (!capturedDraft && createPreflightBusy()) {
      showNotice(tr("gui.create.preflight_already_running", "Create preflight already running"));
      return;
    }
    const draft = capturedDraft ?? captureCreateRunDraft();
    if (!draft) {
      createPreflightPhase = "idle";
      return;
    }
    const normalizedInputs = uniqueNonEmptyPaths(inputs);
    if (normalizedInputs.length === 0) {
      finishCreatePreflightWithIssue("source", tr("gui.create.no_source_items", "No source items selected"));
      return;
    }
    if (focusBlockingTaskIfAny()) {
      createPreflightPhase = "idle";
      return;
    }
    beginCreatePreflight(draft, "choosingDest");
    const base = normalizedInputs.length === 1
      ? archiveBaseOrDefault(archiveStemName(desktopBasename(normalizedInputs[0], platformKind())))
      : "archive";
    let destination: ResolvedCreateDestination | null;
    try {
      destination = await resolveCreateDestination(normalizedInputs, base, draft, source);
    } catch (error) {
      if (createDestinationInspectionCancelled(error)) {
        finishCreatePreflightWithIssue(
          "destination",
          tr(
            "gui.create.destination_check_cancelled",
            "Output check cancelled · no archive was created",
          ),
          "cancelled",
        );
        focusCreatePrimaryAction();
        return;
      }
      finishCreatePreflightWithIssue(
        "destination",
        error instanceof CreateDestinationInspectionError
          ? error.detail
            ? tError(error.detail)
            : tr("gui.create.destination_recheck_failed", "Could not check the destination. Review it and try again.")
          : tr("gui.create.save_dialog_requires_desktop_dialog", "Save dialog requires the desktop file dialog"),
      );
      return;
    }
    if (!destination) {
      finishCreatePreflightWithIssue(
        "destination",
        tr("gui.create.destination_selection_cancelled", "Destination selection cancelled · no archive was created"),
        "cancelled",
      );
      return;
    }
    const { path: dest, replaceExisting, replacementGuard } = destination;
    lastCreateDest = dest;
    const spec: JobSpec = {
      kind: "compress",
      inputs: normalizedInputs,
      dest,
      level: draft.level,
      password: draft.password,
      encrypt_names: draft.encryptNames,
      split_size: draft.splitSize,
      split_mode: draft.splitMode,
      excludes: [...draft.excludes],
      content_policy: draft.contentPolicy,
      sqz_inner_format: draft.sqzInnerFormat,
      sfx_target: draft.sfxTarget,
      replace_existing: replaceExisting,
      replacement_guard: replacementGuard,
      completion: draft.completion,
      post_success: draft.postSuccess,
      test_after_create: draft.testAfterCreate,
    };

    createPreflightPhase = "measuring";
    await ensureCreatePreflightListener();
    const preflightRequestId = nextPreflightRequestId();
    createPreflightRequestId = preflightRequestId;
    createPreflightRequestKind = "source";
    createPreflightProcessedBytes = 0;
    let plan: CreatePlanDto;
    try {
      plan = await ipc.planCreate(spec, preflightRequestId);
    } catch {
      finishCreatePreflightWithIssue(
        "source",
        tr(
          "gui.create.check_excludes_or_permissions",
          "Make sure the output is not selected as a source, then check exclude rules and permissions.",
        ),
      );
      return;
    } finally {
      if (createPreflightRequestId === preflightRequestId) {
        createPreflightRequestId = null;
        createPreflightRequestKind = null;
      }
    }
    if (plan.entries === 0) {
      lastCreatePlan = plan;
      lastDiskSpace = null;
      lastTempDiskSpace = null;
      lastSystemTempDiskSpace = null;
      finishCreatePreflightWithIssue("source", tr("gui.create.no_entries_after_excludes", "No entries after excludes"));
      return;
    }
    lastCreatePlan = plan;
    createPreflightScanned = plan.entries + plan.deduplicated_entries;
    createPreflightCurrent = "";
    lastDiskSpace = null;
    lastTempDiskSpace = null;
    lastSystemTempDiskSpace = null;

    let tempDisk: DiskSpaceDto;
    try {
      createPreflightPhase = "checkingTemp";
      tempDisk = await ipc.checkDiskSpace(desktopDirname(dest, platformKind()), plan.workspace_budget_bytes);
    } catch {
      finishCreatePreflightWithIssue(
        "temp",
        tr("gui.create.temp_preflight_requires_desktop_service", "Workspace check requires the desktop service"),
      );
      return;
    }
    lastTempDiskSpace = tempDisk;
    if (!tempDisk.ok) {
      finishCreatePreflightWithIssue(
        "temp",
        tr("gui.create.not_enough_temp_space", "Not enough destination space for the creation workspace · {available} available")
          .replace("{available}", formatBytes(tempDisk.available_bytes)),
      );
      return;
    }
    if (plan.system_temp_budget_bytes > 0) {
      let systemTempDisk: DiskSpaceDto;
      try {
        const systemTempDir = await ipc.tempDir();
        systemTempDisk = await ipc.checkDiskSpace(systemTempDir, plan.system_temp_budget_bytes);
      } catch {
        finishCreatePreflightWithIssue(
          "temp",
          tr("gui.create.temp_preflight_requires_desktop_service", "Workspace check requires the desktop service"),
        );
        return;
      }
      lastSystemTempDiskSpace = systemTempDisk;
      if (!systemTempDisk.ok) {
        finishCreatePreflightWithIssue(
          "temp",
          tr("gui.create.not_enough_system_temp_space", "Not enough space in the system temporary directory · {available} available")
            .replace("{available}", formatBytes(systemTempDisk.available_bytes)),
        );
        return;
      }
    }

    let disk: DiskSpaceDto;
    try {
      createPreflightPhase = "checkingDest";
      disk = await ipc.checkDiskSpace(desktopDirname(dest, platformKind()), plan.final_output_budget_bytes);
    } catch {
      finishCreatePreflightWithIssue(
        "destination",
        tr("gui.create.destination_preflight_requires_desktop_service", "Destination disk preflight requires the desktop service"),
      );
      return;
    }
    lastDiskSpace = disk;
    if (!disk.ok) {
      finishCreatePreflightWithIssue(
        "destination",
        tr("gui.create.not_enough_destination_space", "Not enough free space in destination · {available} available")
          .replace("{available}", formatBytes(disk.available_bytes)),
      );
      return;
    }

    pendingCreateSubmission = {
      spec,
      source,
      format: draft.format,
      profile: draft.profile,
      creatingSfx: draft.sfxEnabled,
      artifactLabel: draft.sfxEnabled ? createSfxOutputLabel() : createFormats[draft.format].label,
      splitSize: draft.splitSize,
      confirmLateConflict: destination.confirmLateConflict,
      restoreCredentialPrompt: draft.restoreCredentialPrompt,
      restoreEncryptNames: draft.restoreEncryptNames,
    };
    createPreflightIssue = "";
    createPreflightIssueStage = null;
    createPreflightPhase = "reviewing";
    showNotice(
      plan.deduplicated_entries > 0
        ? tr(
          "gui.create.review_ready_overlap_notice",
          "Checks complete · {count} repeated entries merged · review before creating",
        ).replace("{count}", plan.deduplicated_entries.toLocaleString())
        : tr("gui.create.review_ready_notice", "Checks complete · review before creating"),
    );
    await tick();
    document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
  }

  function createPlanConfirmLabel(): string {
    if (createPreflightPhase === "submitting") {
      return tr("gui.create.review.submitting", "Adding to queue");
    }
    if (createPreflightIssueStage === "submit" || createPreflightIssueStage === "destination") {
      return tr("gui.create.review.retry", "Try creating again");
    }
    return tr("gui.create.review.confirm", "Create now");
  }

  function cancelCreatePlanReview() {
    if (!pendingCreateSubmission || createPreflightBusy()) return;
    discardPendingCreatePlan(true);
    createOptionsValidationAttempted = false;
    showNotice(tr("gui.create.review.cancelled", "Create plan cancelled · no task was added"));
  }

  async function refreshConfirmedCreateDestination(
    spec: JobSpec,
    confirmLateConflict: boolean,
  ): Promise<JobSpec | null> {
    if (
      spec.kind !== "compress"
      || (!confirmLateConflict && (spec.replace_existing !== true || !spec.replacement_guard))
    ) {
      return spec;
    }
    const inspection = await inspectCreateDestinationForCreate(
      spec.dest,
      spec.split_size !== null,
      spec.sfx_target ?? null,
    );
    if (!inspection.conflict) {
      return applyCreateDestinationAuthorization(spec, null);
    }
    if (inspection.guard === null) {
      throw new Error("create destination inspection did not return a replacement guard");
    }
    if (
      spec.replace_existing === true
      && inspection.guard === spec.replacement_guard
    ) return spec;

    const { confirm } = await getDialogModule();
    const replaceCurrent = await confirm(
      tr(
        "gui.create.replace_changed.body",
        "The output at {path} changed after your earlier confirmation. Replace the current output with the new archive?",
      ).replace("{path}", spec.dest),
      {
        title: tr("gui.create.replace_changed.title", "Destination changed · replace current output?"),
        kind: "warning",
        okLabel: tr("gui.create.replace_changed.action", "Replace current output"),
        cancelLabel: tr("gui.create.replace_changed.cancel", "Keep current output"),
      },
    );
    if (!replaceCurrent) return null;
    return applyCreateDestinationAuthorization(spec, inspection.guard);
  }

  async function confirmCreatePlan() {
    if (createPreflightBusy()) return;
    const pending = pendingCreateSubmission;
    const plan = lastCreatePlan;
    if (!pending || !plan) {
      showNotice(tr("gui.create.review.expired", "This create plan is no longer current. Choose the sources again."));
      invalidateCreatePreflightResult();
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    createPreflightIssue = "";
    createPreflightIssueStage = null;
    createPreflightPhase = "submitting";
    if (!taskWindowMode) {
      taskCenterReturnFocus = document.querySelector<HTMLElement>(".create-plan-review")
        ?? taskCenterReturnFocus;
    }
    let submissionSpec: JobSpec;
    try {
      const refreshed = await refreshConfirmedCreateDestination(
        pending.spec,
        pending.confirmLateConflict,
      );
      if (!refreshed) {
        createPreflightPhase = "reviewing";
        showNotice(tr("gui.create.replace_changed.kept", "Current output kept · nothing was added to the queue"));
        return;
      }
      submissionSpec = refreshed;
      if (refreshed !== pending.spec) {
        pendingCreateSubmission = { ...pending, spec: refreshed };
      }
    } catch (error) {
      if (createDestinationInspectionCancelled(error)) {
        createPreflightIssueStage = "destination";
        createPreflightIssue = tr(
          "gui.create.destination_recheck_cancelled",
          "Output recheck cancelled · the plan was not submitted",
        );
        createPreflightCurrent = "";
        createPreflightRequestId = null;
        createPreflightRequestKind = null;
        createPreflightProcessedBytes = 0;
        createPreflightCancelPending = false;
        createPreflightPhase = "reviewing";
        showNotice(createPreflightIssue);
        void tick().then(() => {
          document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
        });
        return;
      }
      finishCreatePreflightWithIssue(
        "destination",
        error instanceof CreateDestinationInspectionError && error.detail
          ? tError(error.detail)
          : tr("gui.create.destination_recheck_failed", "Could not recheck the destination. Review it and try again."),
      );
      return;
    }
    try {
      await submitJob(submissionSpec);
      clearCreateSources();
      resetCreateCredentialsAfterPlan(pending);
      createOptionsValidationAttempted = false;
    } catch (error) {
      if (isJobSubmitBlocked(error)) {
        createPreflightIssueStage = "submit";
        createPreflightIssue = jobSubmitBlockedMessage(error);
        createPreflightPhase = "blocked";
        return;
      }
      finishCreatePreflightWithIssue(
        "submit",
        tr("gui.create.submission_requires_desktop_service", "Create archive submission requires the desktop service"),
      );
      return;
    }
    const shouldRestorePrimaryFocus = !taskWindowMode
      && !taskCenterOpen
      && document.activeElement instanceof HTMLElement
      && document.activeElement.closest(".create-plan-review") !== null;
    pendingCreateSubmission = null;
    createPreflightPhase = "ready";
    showNotice(
      (pending.creatingSfx
        ? tr("gui.create.sfx_queued_notice", "Self-extractor added to queue · {size} input")
        : tr("gui.create.queued_notice", "Create archive added to queue · {size} input"))
        .replace("{size}", formatBytes(plan.total_bytes)),
    );
    recordOperation({
      status: "queued",
      title: pending.creatingSfx
        ? tr("gui.create.sfx_queued", "Self-extractor queued")
        : pending.source === "drop"
          ? tr("gui.create.dropped_items_queued", "Dropped items queued")
          : tr("gui.create.queued", "Create archive queued"),
      detail: tr("gui.create.operation_detail", "{name} · {profile} · {size} input")
        .replace("{name}", pathBaseName(plan.primary_output))
        .replace("{profile}", createProfileLabel(pending.profile))
        .replace("{size}", formatBytes(plan.total_bytes)),
    });
    if (shouldRestorePrimaryFocus) {
      await tick();
      createPrimaryAction()?.focus();
    }
  }

  function blockingTask(): Task | null {
    return activeCurrentTask ?? jobRows.find((task) => isTaskActiveState(task.state)) ?? null;
  }

  function submittingTaskModel(): TaskDialogModel | null {
    if (!submittingJobSpec) return null;
    return {
      id: null,
      version: 0,
      spec: submittingJobSpec,
      title: titleForJobSpec(submittingJobSpec),
      origin: "app",
      ownedByRequester: true,
      interaction: null,
      state: "submitting",
      queuePosition: null,
      queueWaitReason: null,
      cpuThreads: 1,
      streamBufferLimitBytes: null,
      done: 0,
      total: 0,
      current: "",
      currentDone: 0,
      currentTotal: 0,
      scanEntries: null,
      speed: 0,
      phase: null,
      interruptible: true,
      pausable: true,
      error: null,
      result: null,
      revealPath: null,
      historyRecorded: false,
      localEffects: false,
      snapshotSeen: false,
      controlIntent: null,
      queueMoveIntent: null,
      expanded: true,
    };
  }

  function currentTaskStatusLabel(): string {
    const failed = jobRows.find((task) => task.state === "failed");
    const task = activeCurrentTask ?? failed ?? null;
    if (!task) return tr("gui.state.ready", "Ready");
    return `${titleForJobSpec(task.spec)} · ${taskStateLabel(task.state)}`;
  }

  function taskCenterSelectedTask(): Task | null {
    if (taskCenterSelectedTaskId === null) return null;
    return jobRows.find((task) => task.id === taskCenterSelectedTaskId) ?? null;
  }

  function taskCenterBadgeCount(): number {
    return taskCenterActionableCount(jobRows) + (submittingJobSpec ? 1 : 0);
  }

  function taskCenterHasAttention(): boolean {
    return taskCenterCounts(jobRows).attention > 0;
  }

  function taskCenterSummaryLabel(): string {
    const counts = taskCenterCounts(jobRows);
    if (counts.attention > 0) {
      return tr("gui.task_center.summary_attention", "{count} need attention")
        .replace("{count}", counts.attention.toLocaleString());
    }
    const active = counts.active + (submittingJobSpec ? 1 : 0);
    if (active > 0 || counts.waiting > 0) {
      return tr("gui.task_center.summary_active", "{active} active · {waiting} waiting")
        .replace("{active}", active.toLocaleString())
        .replace("{waiting}", counts.waiting.toLocaleString());
    }
    if (counts.completed > 0) {
      return tr("gui.task_center.summary_completed", "{count} recent tasks")
        .replace("{count}", counts.completed.toLocaleString());
    }
    return tr("gui.task_center.summary_idle", "No tasks");
  }

  function taskCenterTriggerLabel(): string {
    return `${tr("gui.task_center.title", "Task center")} · ${taskCenterSummaryLabel()}`;
  }

  function openTaskCenter(source: HTMLElement | null = null): void {
    taskCenterReturnFocus = source;
    taskCenterFocusTaskId = null;
    taskCenterSelectedTaskId = null;
    taskCenterOpen = true;
    activePopover = null;
  }

  function closeTaskCenter(): void {
    const returnFocus = taskCenterReturnFocus;
    taskCenterOpen = false;
    taskCenterSelectedTaskId = null;
    taskCenterFocusTaskId = null;
    taskCenterReturnFocus = null;
    void tick().then(() => {
      if (returnFocus?.isConnected) {
        returnFocus.focus();
        return;
      }
      if (screen === "create") createPrimaryAction()?.focus();
    });
  }

  function openTaskCenterDetails(task: Task): void {
    taskCenterFocusTaskId = task.id;
    taskCenterSelectedTaskId = task.id;
  }

  function returnToTaskCenter(task: TaskDialogModel): void {
    taskCenterSelectedTaskId = null;
    taskCenterFocusTaskId = task.id;
  }

  async function clearCompletedTasks(): Promise<void> {
    const ids = clearableTaskIds(jobRows);
    if (ids.length === 0) return;
    if (!await clearFinished(ids)) return;
    showNotice(
      tr("gui.task_center.cleared", "Cleared {count} completed tasks")
        .replace("{count}", ids.length.toLocaleString()),
    );
  }

  function returnTaskQuestionToCenter(taskId: number): void {
    if (taskWindowMode) return;
    taskDialogDismissedId = taskId;
    taskDialogTaskId = null;
    taskCenterOpen = true;
    taskCenterSelectedTaskId = taskId;
    taskCenterFocusTaskId = taskId;
  }

  function taskDialogTask(): TaskDialogModel | null {
    const submitting = submittingTaskModel();
    if (taskWindowMode && submitting) return submitting;
    if (taskDialogTaskId !== null) {
      const remembered = jobRows.find((task) => task.id === taskDialogTaskId);
      if (remembered) return remembered;
    }
    return taskWindowMode ? blockingTask() : null;
  }

  function taskDialogVisible(): boolean {
    const task = taskDialogTask();
    if (!task) return false;
    if (task.id === null) return true;
    return taskDialogDismissedId !== task.id;
  }

  function blockingModalVisible(): boolean {
    return taskDialogVisible() || macosSfxPublisherTask !== null;
  }

  function loadMacosSfxPublisher(): Promise<MacosSfxPublisherComponentType> {
    macosSfxPublisherLoad ??= import("./components/MacosSfxPublisher.svelte")
      .then((module) => module.default)
      .catch((error: unknown) => {
        macosSfxPublisherLoad = null;
        throw error;
      });
    return macosSfxPublisherLoad;
  }

  async function openMacosSfxPublisher(task: TaskDialogModel): Promise<void> {
    if (activePlatform !== "macos") return;
    try {
      LoadedMacosSfxPublisher = await loadMacosSfxPublisher();
      macosSfxPublisherTask = task;
    } catch {
      showNotice(
        tr(
          "gui.sfx_publish.load_failed_detail",
          "The unsigned app is unchanged. Retry this view or return to the task result.",
        ),
      );
    }
  }

  function cancelMacosSfxPublisher(): void {
    macosSfxPublisherTask = null;
  }

  async function chooseMacosSfxPublishOutput(suggested: string): Promise<string | null> {
    const { save } = await getDialogModule();
    return saveNativeDialog("sfx.publish-macos", save, {
      title: tr("gui.sfx_publish.choose_output", "Save the published macOS app"),
      defaultPath: suggested,
      filters: [{
        name: tr("gui.sfx_publish.app_filter", "macOS application"),
        extensions: ["app"],
      }],
    });
  }

  function formatMacosSfxPublishSubmitError(error: unknown): string | null {
    if (isJobSubmitBlocked(error)) return jobSubmitBlockedMessage(error);
    return isErrorDto(error) ? tError(error) : null;
  }

  function taskDialogSurface(task: TaskDialogModel): TaskProgressDialogSurfaceProps {
    return {
      task,
      rootClass: `task-modal-overlay design-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`,
      rootVariables: customPaletteVariables(),
      copyFeedback: taskChecksumCopyFeedback(task),
      copyFeedbackTone: taskChecksumCopyFeedbackTone(task),
      passwordQuestion: taskPasswordQuestion(task),
      passwordValue: jobPasswordValue,
      passwordError: passwordSubmissionError,
      conflictQuestion: taskConflictQuestion(task),
      conflictApplyAll,
      taskOutputPath,
      taskRevealOutputLabel,
      taskWindowMode,
      macosSfxPublishingAvailable: activePlatform === "macos",
      onPause: pauseCurrentTask,
      onResume: resumeCurrentTask,
      onCancel: cancelCurrentTask,
      onCopyChecksumResults: copyTaskChecksumResults,
      onOpenOutput: openTaskOutput,
      onPublishMacosSfx: openMacosSfxPublisher,
      onReviewFailure: reviewFailedTask,
      onToggleDetails: toggleTaskDetails,
      onViewResults: viewTaskResults,
      onRevealOutput: revealTaskOutput,
      onDismiss: dismissTaskDialog,
      onPasswordValueChange: (value) => (jobPasswordValue = value),
      onSubmitPassword: submitPasswordRequest,
      onCancelPassword: cancelPasswordRequest,
      onConflictApplyAllChange: (applyAll) => (conflictApplyAll = applyAll),
      onAnswerConflict: answerConflictDecision,
    };
  }

  function taskCenterDetailSurface(task: TaskDialogModel): TaskProgressDialogSurfaceProps {
    return {
      task,
      rootId: "squallz-task-center",
      rootClass: `task-center-detail design-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`,
      rootVariables: customPaletteVariables(),
      presentation: "panel",
      copyFeedback: taskChecksumCopyFeedback(task),
      copyFeedbackTone: taskChecksumCopyFeedbackTone(task),
      taskOutputPath,
      taskRevealOutputLabel,
      taskWindowMode: false,
      macosSfxPublishingAvailable: activePlatform === "macos",
      onPause: pauseCurrentTask,
      onResume: resumeCurrentTask,
      onCancel: cancelCurrentTask,
      onCopyChecksumResults: copyTaskChecksumResults,
      onOpenOutput: openTaskOutput,
      onPublishMacosSfx: openMacosSfxPublisher,
      onReviewFailure: reviewFailedTask,
      onToggleDetails: toggleTaskDetails,
      onViewResults: viewTaskResults,
      onRevealOutput: revealTaskOutput,
      onDismiss: returnToTaskCenter,
      onPasswordValueChange: (value) => (jobPasswordValue = value),
      onSubmitPassword: submitPasswordRequest,
      onCancelPassword: cancelPasswordRequest,
      onConflictApplyAllChange: (applyAll) => (conflictApplyAll = applyAll),
      onAnswerConflict: answerConflictDecision,
    };
  }

  function taskCenterSurface(): TaskCenterSurfaceProps {
    return {
      tasks: jobRows,
      submittingTask: submittingTaskModel(),
      rootClass: `task-center-panel design-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`,
      rootVariables: customPaletteVariables(),
      focusTaskId: taskCenterFocusTaskId,
      onClose: closeTaskCenter,
      onPause: pauseCurrentTask,
      onResume: resumeCurrentTask,
      onMoveEarlier: moveQueuedTaskEarlier,
      onMoveLater: moveQueuedTaskLater,
      onMoveBefore: moveQueuedTaskBefore,
      onCancel: cancelCurrentTask,
      onDetails: openTaskCenterDetails,
      onClear: clearCompletedTasks,
    };
  }

  function taskInteractionWorkspaceSurface(
    variant: TaskInteractionWorkspaceVariant,
    kind: TaskInteractionWorkspaceKind,
  ): TaskInteractionWorkspaceSurface {
    if (kind === "password") {
      const forgetDisabledReason = passwordBookForgetDisabledReason();
      return {
        kind,
        variant,
        tr,
        active: Boolean(jobPasswordPrompt || archivePasswordPrompt),
        name: passwordPromptName(),
        detail: passwordPromptDetail(),
        sessionDetail: passwordSessionDetail(),
        failureDetail: passwordFailureDetail(),
        secretStoreLabel: secretStoreLabel(),
        value: jobPasswordValue,
        busy: archiveOpenStatus === "opening",
        error: passwordSubmissionError,
        forgetVisible: Boolean(jobPasswordPrompt),
        forgetDisabledReason,
        forgetAriaLabel: labelWithDisabledReason(
          tr("gui.settings.password_book.forget_current", "Forget current archive"),
          forgetDisabledReason,
        ),
        onInputMount: (input) => (standalonePasswordInput = input),
        onValueChange: (value) => (jobPasswordValue = value),
        onSubmit: submitPasswordRequest,
        onCancel: cancelPasswordRequest,
        onForget: forgetPasswordBookPanel,
        onBack: () => setScreen("browse"),
      };
    }
    return {
      kind,
      variant,
      tr,
      active: Boolean(jobConflictPrompt),
      title: conflictPromptTitle(),
      detail: conflictPromptDetail(),
      rows: conflictRowsView().map((row) => ({
        ...row,
        decision: conflictDecisionLabel(row.decision),
      })),
      applyAll: conflictApplyAll,
      onApplyAllChange: (value) => (conflictApplyAll = value),
      onAnswer: answerConflictDecision,
      onCancel: cancelConflictPrompt,
      onBack: () => setScreen("extract"),
    };
  }

  function taskPasswordQuestion(task: TaskDialogModel) {
    if (
      task.id === null ||
      !isTaskActiveState(task.state) ||
      jobPasswordPrompt?.id !== task.id
    ) return null;
    return {
      name: passwordPromptName(),
      detail: passwordPromptDetail(),
      sessionDetail: passwordSessionDetail(),
    };
  }

  function taskConflictQuestion(task: TaskDialogModel) {
    if (
      task.id === null ||
      !isTaskActiveState(task.state) ||
      jobConflictPrompt?.id !== task.id
    ) return null;
    const row = conflictRowsView()[0];
    return row
      ? { path: row.path, existing: row.existing, incoming: row.incoming }
      : null;
  }

  function openTaskDialog(task: Task | null = blockingTask()): void {
    if (!task) return;
    taskDialogTaskId = task.id;
    taskDialogDismissedId = null;
  }

  function focusBlockingTaskIfAny(replacesExistingOutput = false): TaskSubmissionBlockReason | null {
    const active = blockingTask();
    const reason = taskSubmissionBlockReason({
      submitInFlight: jobSubmitInFlight,
      taskWindowMode,
      hasActiveTask: active !== null,
      replacesExistingOutput,
    });
    if (reason === null) return null;
    if (reason === "starting") {
      if (import.meta.env.DEV && params.has("validationTrace")) {
        const win = window as ValidationWindow;
        win.__squallzValidationJobSubmitBlockedWhileStarting = (win.__squallzValidationJobSubmitBlockedWhileStarting ?? 0) + 1;
      }
    }
    if (reason === "task-window-busy" && active) openTaskDialog(active);
    showNotice(jobSubmitBlockedMessage(new JobSubmitBlockedError(reason)));
    return reason;
  }

  async function dismissTaskDialog(task: TaskDialogModel): Promise<void> {
    if (task.id === null) return;
    if (isTaskActiveState(task.state)) return;
    if (!taskWindowMode && taskCenterSelectedTaskId === task.id) {
      closeTaskCenter();
      return;
    }
    if (taskWindowMode && await closeNativeTaskWindow()) return;
    taskDialogDismissedId = task.id;
    taskDialogTaskId = null;
  }

  function isJobSubmitBlocked(error: unknown): boolean {
    return error instanceof JobSubmitBlockedError;
  }

  function jobSubmitBlockedMessage(error: unknown): string {
    if (!(error instanceof JobSubmitBlockedError)) {
      return tr("gui.task.one_at_a_time_notice", "Finish or cancel the current task before starting another one");
    }
    if (error.reason === "starting") {
      return tr(
        "gui.task.starting_notice",
        "The previous task is still entering the queue. Try again in a moment",
      );
    }
    if (error.reason === "replace-existing") {
      return tr(
        "gui.task.replace_existing_queue_blocked",
        "Wait for the current queue to finish before replacing existing output.",
      );
    }
    return tr("gui.task.one_at_a_time_notice", "Finish or cancel the current task before starting another one");
  }

  async function submitJob(spec: JobSpec): Promise<number> {
    const blockReason = focusBlockingTaskIfAny(
      spec.kind === "compress" && spec.replace_existing === true,
    );
    if (blockReason) {
      throw new JobSubmitBlockedError(blockReason);
    }
    jobSubmitInFlight = true;
    submittingJobSpec = spec;
    if (taskWindowMode) {
      taskDialogTaskId = null;
      taskDialogDismissedId = null;
    } else {
      if (taskCenterReturnFocus === null) {
        const focused = document.activeElement;
        if (
          focused instanceof HTMLElement &&
          focused !== document.body &&
          !focused.closest("#squallz-task-center")
        ) {
          taskCenterReturnFocus = focused;
        }
      }
      taskCenterFocusTaskId = null;
      taskCenterSelectedTaskId = null;
      taskCenterOpen = true;
    }
    try {
      if (import.meta.env.DEV && params.has("validationTrace")) {
        const win = window as ValidationWindow;
        win.__squallzValidationJobSubmitAttempts = (win.__squallzValidationJobSubmitAttempts ?? 0) + 1;
      }
      if (import.meta.env.DEV && runtimePreviews.jobSubmitDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, runtimePreviews.jobSubmitDelayMs));
      }
      const id = await submitArchiveJob(spec);
      if (taskWindowMode) {
        taskDialogTaskId = id;
        taskDialogDismissedId = null;
      }
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

  function moveQueuedTaskEarlier(task: Task): void {
    moveTaskEarlier(task.id);
    showNotice(tr("gui.task_center.moving_earlier", "Moving task earlier in the queue…"));
  }

  function moveQueuedTaskLater(task: Task): void {
    moveTaskLater(task.id);
    showNotice(tr("gui.task_center.moving_later", "Moving task later in the queue…"));
  }

  function moveQueuedTaskBefore(task: Task, beforeTask: Task | null): void {
    moveTaskBefore(task.id, beforeTask?.id ?? null);
    showNotice(tr("gui.task_center.moving_position", "Moving task to its new queue position…"));
  }

  function taskRevealOutputLabel(): string {
    return t("gui.task.show_in_file_manager", { fileManager: fileManagerLabel() });
  }

  function adoptRecoveryTargetFromTask(task: TaskDialogModel): void {
    let source: string | null = null;
    let sidecar: string | null | undefined;
    switch (task.spec.kind) {
      case "protect":
        source = task.spec.path;
        sidecar = null;
        break;
      case "verify_recovery":
      case "repair_recovery":
        source = task.spec.path;
        sidecar = task.spec.recovery;
        break;
      case "repair_zip":
      case "repair_sqz":
      case "export_sqz":
        source = task.spec.src;
        sidecar = null;
        break;
      case "extract":
        if (!task.spec.best_effort) return;
        source = task.spec.path;
        sidecar = recoverySourcePath() && sameFilePath(recoverySourcePath() ?? "", source)
          ? undefined
          : null;
        break;
      case "test":
        source = task.spec.path;
        sidecar = recoverySourcePath() && sameFilePath(recoverySourcePath() ?? "", source)
          ? undefined
          : null;
        break;
      default:
        return;
    }
    if (!source) return;
    const preserveSelectedSource = Boolean(
      recoverySourceMode === "selected" &&
      recoverySourceOverride &&
      sameFilePath(recoverySourceOverride, source),
    );
    recoverySourceMode = !preserveSelectedSource && currentArchive && sameFilePath(currentArchive.path, source)
      ? "current"
      : "selected";
    recoverySourceOverride = recoverySourceMode === "selected" ? source : null;
    if (sidecar !== undefined) recoveryPar2Override = sidecar;
  }

  function testTaskUsesRecoveryContext(task: TaskDialogModel): boolean {
    if (task.spec.kind !== "test") return false;
    if (task.id !== null && recoveryContextTaskIds.has(task.id)) return true;
    if (
      recoverySourceMode === "selected" &&
      recoverySourceOverride &&
      sameFilePath(recoverySourceOverride, task.spec.path)
    ) {
      return true;
    }
    return !currentArchive || !sameFilePath(currentArchive.path, task.spec.path);
  }

  function viewTaskResults(task: TaskDialogModel): void {
    if (testTaskUsesRecoveryContext(task)) {
      if (task.id !== null) setTaskExpanded(task.id, true);
      return;
    }
    if (taskHasInlineResults(task)) {
      if (task.id !== null) setTaskExpanded(task.id, !task.expanded);
      return;
    }
    const target = taskResultScreen(task);
    if (!target) {
      return;
    }
    if (taskWindowMode) {
      if (task.id !== null) setTaskExpanded(task.id, true);
      return;
    }
    if (target === "recovery") adoptRecoveryTargetFromTask(task);
    setScreen(target);
    void dismissTaskDialog(task);
    if (target === "checksum") {
      void focusChecksumResultPanel(task.spec.kind === "checksum_check" ? "checksum_check" : "checksum");
    }
  }

  function toggleTaskDetails(task: TaskDialogModel): void {
    if (task.id === null || task.state !== "failed") return;
    setTaskExpanded(task.id, !task.expanded);
  }

  function reviewFailedTask(task: TaskDialogModel): void {
    if (taskWindowMode) return;
    let target = taskFailureReviewScreen(task);
    if (target === "archiveInfo" && testTaskUsesRecoveryContext(task)) {
      target = "recovery";
    }
    if (target === "extract" && task.spec.kind === "extract" && task.spec.best_effort) {
      target = "recovery";
    }
    if (!target) return;
    if (target === "recovery") adoptRecoveryTargetFromTask(task);
    setScreen(target);
    void dismissTaskDialog(task);
  }

  async function openTaskOutput(task: TaskDialogModel): Promise<void> {
    const outputPath = taskOutputPath(task);
    if (!outputPath || !taskOutputCanOpen(task)) return;
    if (task.spec.kind === "compress") {
      await openArchivePath(outputPath, "open-file");
      return;
    }
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
    return jobPasswordPrompt?.name
      ?? (archivePasswordPrompt ? pathBaseName(archivePasswordPrompt.path) : null)
      ?? tr("gui.password.no_prompt", "No password prompt");
  }

  function passwordPromptDetail(): string {
    if (jobPasswordPrompt) {
      return jobPasswordPrompt.wrong
        ? tr("gui.password.previous_rejected", "Previous password was rejected. Try again or cancel this job.")
        : tr("gui.password.task_paused", "This task is waiting for the archive password.");
    }
    if (archivePasswordPrompt) {
      return archivePasswordPrompt.wrong
        ? tr("gui.password.open_previous_rejected", "That password was rejected. Try again or return to the archive list.")
        : tr("gui.password.archive_waiting", "Enter the password to open this archive.");
    }
    return tr("gui.password.no_prompt_pending", "No password request is active.");
  }

  function passwordSessionDetail(): string {
    return jobPasswordPrompt
      ? tr("gui.password.session_only_separate_book", "Session only for this job; saved passwords use the separate Password Book flow")
      : tr("gui.password.open_session_only", "Used only to open this archive in the current app session.");
  }

  function passwordFailureDetail(): string {
    return jobPasswordPrompt
      ? tr("gui.password.return_to_prompt", "Return to prompt, do not fail whole batch")
      : tr("gui.password.open_return_to_prompt", "Stay on this prompt so you can retry or cancel.");
  }

  function jobQuestionReturnScreen(promptId: number): Screen {
    const task = jobRows.find((item) => item.id === promptId);
    return recoverySubmissionPending ||
      recoveryContextTaskIds.has(promptId) ||
      (task?.spec.kind === "extract" && task.spec.best_effort)
      ? "recovery"
      : "extract";
  }

  async function submitPasswordRequest() {
    if (!jobPasswordPrompt && !archivePasswordPrompt) {
      showNotice(tr("gui.password.no_prompt_pending", "No password request is active."));
      return;
    }
    passwordSubmissionAttempted = true;
    if (!taskPasswordReady(jobPasswordValue)) return;
    passwordSubmissionAttempted = false;
    if (jobPasswordPrompt) {
      const promptId = jobPasswordPrompt.id;
      const returnScreen = jobQuestionReturnScreen(promptId);
      answerJobPassword(jobPasswordValue);
      jobPasswordValue = "";
      setScreen(returnScreen);
      returnTaskQuestionToCenter(promptId);
      showNotice(tr("gui.password.sent_to_task", "Password sent to task"));
      return;
    }
    const prompt = archivePasswordPrompt;
    if (!prompt) return;
    if (archiveOpenStatus === "opening") return;
    const requestGeneration = ++archiveOpenGeneration;
    standalonePasswordFocusedInput = null;
    archiveOpenStatus = "opening";
    const ok = await openArchiveStore(prompt.path, jobPasswordValue, prompt.encoding);
    if (requestGeneration !== archiveOpenGeneration) return;
    archiveOpenStatus = "idle";
    jobPasswordValue = "";
    if (ok) {
      finishOpenedArchive(prompt.path, "password");
      return;
    }
    if (openPasswordPrompt()?.path === prompt.path) {
      showNotice(tr("gui.password.open_previous_rejected", "That password was rejected. Try again or return to the archive list."));
      return;
    }
    if (archiveOpenError(prompt.path)?.key === "error.corrupt_archive") {
      recoverySourceMode = "selected";
      recoverySourceOverride = prompt.path;
      recoveryPar2Override = null;
      setScreenRespectingJobQuestion("recovery");
      showNotice(
        tr("gui.recovery.open_failed_routed", "{name} could not be opened. It is ready for recovery checks.")
          .replace("{name}", pathBaseName(prompt.path)),
      );
      return;
    }
    if (!archiveOpenError(prompt.path)) return;
    setScreenRespectingJobQuestion("browse");
    showNotice(archiveOpenFailureNotice(prompt.path));
  }

  function cancelPasswordRequest() {
    passwordSubmissionAttempted = false;
    if (jobPasswordPrompt) {
      const promptId = jobPasswordPrompt.id;
      const returnScreen = jobQuestionReturnScreen(promptId);
      answerJobPassword(null);
      showNotice(tr("gui.password.prompt_cancelled", "Password prompt cancelled"));
      jobPasswordValue = "";
      setScreen(returnScreen);
      returnTaskQuestionToCenter(promptId);
      return;
    }
    if (archivePasswordPrompt) {
      archiveOpenGeneration += 1;
      archiveOpenStatus = "idle";
      cancelArchivePasswordPrompt();
      showNotice(tr("gui.password.archive_open_cancelled", "Archive opening cancelled."));
    }
    jobPasswordValue = "";
    setScreen("browse");
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
    const source = recoverySourcePath();
    const sidecar = recoveryPar2Path();
    if (!source || !sidecar) return null;
    return latestMatchingRecoveryTask(jobRows, (task) => {
      if (task.spec.kind !== "verify_recovery" && task.spec.kind !== "repair_recovery") return false;
      const taskSidecar = task.spec.recovery ?? `${task.spec.path}.par2`;
      return sameFilePath(task.spec.path, source) && sameFilePath(taskSidecar, sidecar);
    });
  }

  function recoveryReport(): Record<string, unknown> | null {
    return latestRecoveryReportTask()?.result ?? null;
  }

  function recoveryMetrics(): Record<string, unknown> | null {
    return recoveryResultMetrics(recoveryReport());
  }

  function recoveryMetricNumber(key: string): number | null {
    return recoveryResultMetricNumber(recoveryReport(), key);
  }

  function recoveryMetricBoolean(key: string): boolean | null {
    return recoveryResultMetricBoolean(recoveryReport(), key);
  }

  function recoveryResultAvailable(): boolean {
    return recoveryReport() !== null;
  }

  function recoveryMetricsAvailable(): boolean {
    return recoveryMetrics() !== null;
  }

  function recoveryBeyondCapacity(): boolean {
    return recoveryMetricsAvailable() && recoveryMetricBoolean("repair_possible") === false;
  }

  function recoveryNoDamage(): boolean {
    return recoveryResultHasNoDamage(recoveryReport());
  }

  function recoveryResultTone(): RecoveryWorkspaceView["resultTone"] {
    return recoveryResultToneFor(recoveryReport());
  }

  function recoveryVerifyRecommended(): boolean {
    return !recoveryResultAvailable() && !recoveryVerifyDisabledReason();
  }

  function recoveryRepairRecommended(): boolean {
    return recoveryResultOperation(recoveryReport()) === "verify"
      && recoveryResultConfirmsRepairCapacity(recoveryReport())
      && !recoveryRepairPar2DisabledReason();
  }

  function recoveryRemainingMargin(): number | null {
    const needed = recoveryMetricNumber("blocks_needed");
    const available = recoveryMetricNumber("recovery_blocks_available");
    if (needed === null || available === null) return null;
    return Math.max(0, available - needed);
  }

  function recoveryReportString(key: string): string | null {
    const value = recoveryReport()?.[key];
    return typeof value === "string" && value.length > 0 ? value : null;
  }

  function recoveryReportNumber(key: string): number | null {
    const value = recoveryReport()?.[key];
    return typeof value === "number" && Number.isFinite(value) && value >= 0
      ? value
      : null;
  }

  function recoveryOverCapacityDetail(): string {
    const needed = recoveryMetricNumber("blocks_needed") ?? 0;
    const available = recoveryMetricNumber("recovery_blocks_available") ?? 0;
    return tr("gui.recovery.damage_over_capacity_values", "{needed} damaged or missing blocks exceed {available} available recovery blocks.")
      .replace("{needed}", needed.toLocaleString())
      .replace("{available}", available.toLocaleString());
  }

  function recoveryResultTitle(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.no_archive_selected", "No archive selected");
    if (!recoveryResultAvailable()) return tr("gui.recovery.not_verified", "Not verified");
    const operation = recoveryResultOperation(recoveryReport());
    const ok = recoveryResultOk(recoveryReport());
    if (operation === "repair") {
      if (ok === true) return tr("gui.recovery.repair_completed", "Repair completed");
      return tr("gui.recovery.repair_not_completed", "Repair did not complete");
    }
    if (recoveryNoDamage()) {
      return tr("gui.recovery.no_damage", "No damage");
    }
    const repairPossible = recoveryMetricBoolean("repair_possible");
    if (repairPossible === true) return tr("gui.recovery.repairable", "Repairable");
    if (repairPossible === false) return tr("gui.recovery.not_repairable", "Not repairable");
    return ok === true
      ? tr("gui.recovery.verification_passed", "Verification passed")
      : tr("gui.recovery.damage_detected", "Damage detected");
  }

  function recoveryResultDetail(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.choose_archive_before_verify", "Choose the archive described by this PAR2 file.");
    if (!recoveryResultAvailable()) return tr("gui.recovery.run_verify_capacity", "Verify recovery capacity before creating a repaired copy.");
    const needed = recoveryMetricNumber("blocks_needed");
    const available = recoveryMetricNumber("recovery_blocks_available");
    if (needed !== null && available !== null) {
      return tr("gui.recovery.capacity_summary", "{needed} blocks needed · {available} recovery blocks available")
        .replace("{needed}", needed.toLocaleString())
        .replace("{available}", available.toLocaleString());
    }
    return recoveryResultOk(recoveryReport()) === true
      ? tr("gui.recovery.tool_no_block_counts_ok", "Verification completed, but the PAR2 tool did not report block counts.")
      : tr("gui.recovery.tool_no_block_counts_failed", "Verification found a problem, but the PAR2 tool did not report block counts.");
  }

  function recoveryResultExplanation(): string {
    if (!recoveryResultAvailable() || !recoveryMetricsAvailable()) return recoveryResultDetail();
    if (recoveryResultOperation(recoveryReport()) === "repair" && recoveryResultOk(recoveryReport()) === false) {
      if (recoveryBeyondCapacity()) return recoveryOverCapacityDetail();
      return tr("gui.recovery.repair_incomplete_despite_capacity", "Recovery data appears sufficient, but the repair task did not complete. Review the report before trying again.");
    }
    if (recoveryNoDamage()) {
      return tr("gui.recovery.no_damage_body", "Verification found no damaged or missing blocks.");
    }
    if (recoveryMetricBoolean("repair_possible") === true) {
      return tr("gui.recovery.damage_within_rs_capacity", "Detected damage is within the available Reed-Solomon recovery capacity.");
    }
    return recoveryOverCapacityDetail();
  }

  function recoveryResultFooter(): string {
    if (!recoverySourcePath()) return tr("gui.recovery.choose_archive_before_verify", "Choose the archive described by this PAR2 file.");
    if (!recoveryResultAvailable()) return tr("gui.recovery.no_verification_result", "No verification result yet");
    const repaired = recoveryMetricNumber("blocks_repaired") ?? 0;
    if (repaired > 0) {
      return tr("gui.recovery.blocks_repaired", "{count} blocks repaired").replace("{count}", repaired.toLocaleString());
    }
    if (recoveryResultOperation(recoveryReport()) === "repair" && recoveryResultOk(recoveryReport()) === true) {
      const output = recoveryReportString("output");
      return output
        ? tr("gui.recovery.repaired_copy_ready", "Repaired copy: {name}").replace("{name}", pathBaseName(output))
        : tr("gui.recovery.repair_completed", "Repair completed");
    }
    if (recoveryResultOk(recoveryReport()) === false) {
      return recoveryBeyondCapacity()
        ? tr("gui.recovery.damage_exceeds_capacity", "Damage exceeds available recovery data")
        : tr("gui.recovery.verification_did_not_pass", "Verification did not pass");
    }
    return taskStateLabel(latestRecoveryReportTask()?.state);
  }

  function recoveryRedundancyLabel(): string {
    const redundancy = recoveryRedundancyValue();
    if (redundancy === null) return recoveryRedundancyError();
    return tr("gui.recovery.redundancy_percent", "{percent}% requested redundancy")
      .replace("{percent}", redundancy.toLocaleString());
  }

  function recoveryWorkspaceView(): RecoveryWorkspaceView {
    const source = recoverySourcePath();
    const protectSourceDisabledReason = recoveryProtectSourceDisabledReason();
    const protectDisabledReason = recoveryProtectDisabledReason();
    const resultAvailable = recoveryResultAvailable();
    const metricsAvailable = recoveryMetricsAvailable();
    const sourceIsSqz = isRecoverySourceSqz();
    const notAvailable = tr("gui.recovery.not_available_for_target", "Not available for this target");
    const protectedSourceCount = recoveryReportNumber("source_file_count")
      ?? (recoverySourceMatchesCurrentArchive()
        ? Math.max(1, currentArchive?.volumes?.length ?? 1)
        : 1);
    const metrics = metricsAvailable
      ? {
          blocksNeeded: recoveryMetricNumber("blocks_needed")?.toLocaleString() ?? "-",
          recoveryBlocksAvailable:
            recoveryMetricNumber("recovery_blocks_available")?.toLocaleString() ?? "-",
          remainingMargin: recoveryRemainingMargin()?.toLocaleString() ?? "-",
        }
      : null;
    const formatWorkflowTitle = sourceIsSqz
      ? tr("gui.recovery.sqz_capable_not_checked", "SQZ recovery capable · not checked")
      : source
        ? tr("gui.recovery.par2_sidecar_workflow", "PAR2 sidecar workflow")
        : tr("gui.recovery.no_archive_selected", "No archive selected");
    const formatWorkflowBody = sourceIsSqz
      ? tr(
          "gui.recovery.sqz_capability_body",
          "SQZ supports embedded recovery, but this file has not been checked until a repair or test task reports a result.",
        )
      : source
        ? tr(
            "gui.recovery.standard_archive_capability_body",
            "Use PAR2 for damage recovery. ZIP index rebuild is a separate workflow for ZIP-family archives with readable local headers.",
          )
        : tr(
            "gui.recovery.choose_archive_for_capabilities",
            "Choose an archive to see the recovery tools available for its format.",
          );

    return {
      archiveName: source,
      par2Name: recoveryPar2Path(),
      currentArchiveAvailable: currentArchive !== null,
      usesCurrentArchive: recoverySourceMatchesCurrentArchive(),
      usesDefaultPar2: source !== null && recoveryPar2Override === null,
      pickerBusy: recoveryPickerStatus !== "idle",
      pickerBusyReason: recoveryPickerBusyReason(),
      testDisabledReason: recoveryTestDisabledReason(),
      sourceName: recoverySourceName() ?? tr("gui.recovery.no_archive_selected", "No archive selected"),
      requestedRedundancy: protectSourceDisabledReason
        ? notAvailable
        : recoveryRedundancyLabel(),
      redundancyDraft: recoveryRedundancyDraft,
      redundancyError: recoveryRedundancyError(),
      protectedSourceCount,
      repairCapacity: resultAvailable
        ? recoveryResultDetail()
        : tr("gui.recovery.shown_after_verify", "Shown after verify"),
      repairOutputMode: recoveryRepairUsesDirectory()
        ? tr(
            "gui.recovery.repair_output_new_folder",
            "New folder with all {count} protected files",
          ).replace("{count}", protectedSourceCount.toLocaleString())
        : tr(
            "gui.recovery.repair_output_new_file",
            "New file; source stays unchanged",
          ),
      plannedIndex: protectSourceDisabledReason
        ? notAvailable
        : pathBaseName(defaultRecoveryPath() ?? ""),
      resultTone: recoveryResultTone(),
      resultTitle: recoveryResultTitle(),
      resultDetail: recoveryResultDetail(),
      resultExplanation: recoveryResultExplanation(),
      resultFooter: recoveryResultFooter(),
      resultAvailable,
      metrics,
      beyondCapacity: recoveryBeyondCapacity(),
      formatWorkflowTitle,
      formatWorkflowBody,
      protectDisabledReason,
      verifyDisabledReason: recoveryVerifyDisabledReason(),
      repairDisabledReason: recoveryRepairPar2DisabledReason(),
      zipDisabledReason: recoveryZipDisabledReason(),
      sqzRepairDisabledReason: recoverySqzRepairDisabledReason(),
      sqzExportDisabledReason: outputAuthorizationPending || recoverySubmissionPending
        ? tr("gui.output.checking_existing", "Checking output…")
        : recoverySqzExportDisabledReason(),
      bestEffortDisabledReason: recoveryBestEffortDisabledReason(),
      verifyRecommended: recoveryVerifyRecommended(),
      repairRecommended: recoveryRepairRecommended(),
    };
  }

  const recoveryWorkspaceActions: RecoveryWorkspaceActions = {
    chooseArchive: () => void chooseRecoveryArchive(),
    choosePar2: () => void chooseRecoveryPar2(),
    useCurrentArchive: useCurrentArchiveForRecovery,
    useDefaultPar2: useDefaultPar2ForRecovery,
    testArchive: () => void submitRecoveryTestJob(),
    setRedundancy: setRecoveryRedundancy,
    protect: () => void submitProtectJob(),
    verify: () => void submitVerifyRecoveryJob(),
    repair: () => void submitRepairRecoveryJob(),
    repairZip: () => void submitRepairZipJob(),
    repairSqz: () => void submitRepairSqzJob(),
    exportSqz: () => void submitExportSqzJob(),
    extractReadable: () => void submitBestEffortExtractJob(),
  };

  function answerConflictDecision(decision: TaskConflictDecision, applyAll: boolean) {
    if (!jobConflictPrompt) {
      showNotice(tr("gui.conflict.no_prompt_pending", "No conflict request is active"));
      return;
    }
    const answer = normalizeTaskConflictAnswer(decision, applyAll);
    const promptId = jobConflictPrompt.id;
    const returnScreen = jobQuestionReturnScreen(promptId);
    answerJobConflict(answer.decision, answer.applyAll);
    if (answer.decision === "abort") {
      showNotice(tr("gui.conflict.extraction_cancelled", "Extraction cancelled"));
    } else {
      showNotice(
        answer.applyAll
          ? tr("gui.conflict.decision_applied_remaining", "Conflict decision applied to remaining files")
          : tr("gui.conflict.decision_sent", "Conflict decision sent to task"),
      );
    }
    conflictApplyAll = false;
    setScreen(returnScreen);
    returnTaskQuestionToCenter(promptId);
  }

  function currentArchiveName(): string {
    return currentArchive?.name ?? noArchiveLabel();
  }

  function passwordBookSecretStoreLabel(): string {
    if (!currentArchive) return tr("gui.settings.password_book.not_checked", "Not checked");
    if (passwordBookStatus.state === "checking") return tr("gui.settings.password_book.checking", "Checking");
    if (passwordBookStatus.state === "idle") return tr("gui.settings.password_book.not_checked", "Not checked");
    if (passwordBookStatus.state === "error") return tr("gui.settings.password_book.unavailable_status", "Unavailable");
    return passwordBookStatus.available
      ? tr("gui.settings.password_book.available", "Available")
      : tr("gui.settings.password_book.unavailable_status", "Unavailable");
  }

  function passwordBookCurrentLabel(): string {
    if (!currentArchive) return noArchiveLabel();
    if (currentArchive.read_only) return tr("gui.settings.password_book.unavailable_status", "Unavailable");
    if (passwordBookStatus.state === "checking") return tr("gui.settings.password_book.checking", "Checking");
    if (passwordBookStatus.state !== "ready") return tr("gui.settings.password_book.not_checked", "Not checked");
    return passwordBookStatus.saved
      ? tr("gui.settings.password_book.saved_status", "Saved")
      : tr("gui.settings.password_book.not_saved_status", "Not saved");
  }

  function passwordBookDetailLabel(): string {
    if (!currentArchive) return tr("gui.settings.password_book.open_archive_to_check", "Open an archive to check saved password status");
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable", "Extract the inner archive before saving or checking its password.");
    }
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.checking_detail", "Checking the system secret store");
    }
    if (passwordBookStatus.state === "error") {
      return tr("gui.settings.password_book.check_failed_detail", "Refresh to check the system secret store again");
    }
    if (passwordBookStatus.state === "idle") {
      return tr("gui.settings.password_book.refresh_to_check", "Refresh to check for a saved password");
    }
    if (!passwordBookStatus.available) {
      return tr("gui.settings.password_book.secret_store_unavailable_detail", "The system secret store is unavailable. Make sure it is installed and running with a default password collection; passwords stay in this session until then.");
    }
    return passwordBookStatus.saved
      ? tr("gui.settings.password_book.current_has_saved_entry", "Current archive has a saved secret-store entry")
      : tr("gui.settings.password_book.prompt_or_save_after_unlock", "Prompt or save after unlocking this archive");
  }

  function passwordBookForgetDisabledReason(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable", "Extract the inner archive before saving or checking its password.");
    }
    if (archiveHasSessionPassword()) return "";
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.wait_for_status", "Wait for the password status check to finish");
    }
    if (passwordBookStatus.state === "error") {
      return tr("gui.settings.password_book.refresh_after_failure", "Refresh password status before forgetting it");
    }
    if (passwordBookStatus.state !== "ready") {
      return tr("gui.settings.password_book.refresh_before_forgetting", "Check password status before forgetting it");
    }
    if (!passwordBookStatus.saved) {
      return tr("gui.settings.password_book.no_saved_entry", "The current archive has no stored password");
    }
    return "";
  }

  function passwordBookRefreshDisabledReason(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable", "Extract the inner archive before saving or checking its password.");
    }
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.wait_for_status", "Wait for the password status check to finish");
    }
    return "";
  }

  async function refreshPasswordBookPanel() {
    const disabledReason = passwordBookRefreshDisabledReason();
    if (disabledReason) {
      showNotice(disabledReason);
      return;
    }
    if (!currentArchive) return;

    try {
      await refreshArchivePasswordBookStatus(currentArchive.path);
      showNotice(tr("gui.settings.password_book.status_refreshed", "Password Book status refreshed"));
    } catch {
      showNotice(tr("gui.settings.password_book.status_check_failed", "Could not check Password Book status. Start or unlock the system secret store, then try again."));
    }
  }

  async function forgetPasswordBookPanel() {
    const disabledReason = passwordBookForgetDisabledReason();
    if (disabledReason) {
      showNotice(disabledReason);
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

  const convertRouteBridge: ConvertRouteBridge = {
    getArchive: () => currentArchive,
    tr,
    tError: (error) => isErrorDto(error) ? tError(error) : String(error),
    showNotice,
    ensurePreflightListener: ensureCreatePreflightListener,
    getDialogModule,
    saveNativeDialog,
    submitJob,
    focusBlockingTaskIfAny: () => Boolean(focusBlockingTaskIfAny()),
    isJobSubmitBlocked,
    jobSubmitBlockedMessage,
    recordQueuedOperation: (title, detail) => recordOperation({
      status: "queued",
      title,
      detail,
    }),
    archiveStemName: (name) => archiveStemName(name),
    platform: platformKind,
    prepareSubmitFocus: () => {
      if (taskWindowMode) return;
      taskCenterReturnFocus = document.querySelector<HTMLElement>(".create-plan-review")
        ?? taskCenterReturnFocus;
    },
    shouldRestorePrimaryFocus: () => !taskWindowMode
      && !taskCenterOpen
      && document.activeElement instanceof HTMLElement
      && document.activeElement.closest(".create-plan-review") !== null,
    register: (handle) => {
      convertRouteHandle = handle;
      handle.syncArchive(currentArchive);
    },
  };

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
      const selectionBusyReason = archiveSelectionBusyReason();
      if (selectionBusyReason) {
        showNotice(selectionBusyReason);
        return;
      }
      openExtractWorkspace(hasArchiveSelection() ? "selection" : "all");
      return;
    }
    if (label === "Test") {
      void submitTestJob();
      return;
    }
    if (label === "Protect") {
      openRecoveryConfiguration();
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
      setScreen("convert");
      return;
    }
    if (label === "Info") {
      setScreen("archiveInfo");
      return;
    }
    setScreen(screenForCommand(label));
  }

  function classicCommandDisabled(label: string): boolean {
    if (currentArchive?.read_only && ["Add", "Protect", "Delete", "Rename", "Move", "New Folder"].includes(label)) return true;
    if (label === "Checksum" || label === "Duplicates" || label === "Info" || label === "Protect") return false;
    if (label === "Extract To" && archiveSelectionBusyReason()) return true;
    if (label === "Rename") return !canRenameSelection();
    if (label === "Move" || label === "Delete") return !hasArchiveSelection();
    if (label === "View") return !canPreviewEntrySelection();
    return !hasArchiveOpen();
  }

  function classicCommandDisabledTitle(label: string): string {
    if (label === "Extract To") {
      const selectionBusyReason = archiveSelectionBusyReason();
      if (selectionBusyReason) return selectionBusyReason;
    }
    if (!classicCommandDisabled(label)) return "";
    if (!currentArchive) {
      if (label === "Add") return tr("gui.precondition.open_before_add", "Open an archive before adding files");
      if (label === "Extract To") return tr("gui.precondition.open_before_extract", "Open an archive before extracting");
      if (label === "Test") return tr("gui.precondition.open_before_test", "Open an archive before testing");
      if (label === "Protect") return tr("gui.precondition.open_before_protect", "Open an archive before protecting");
      if (label === "View") return tr("gui.preview.open_archive_first", "Open an archive before opening or previewing entries");
      if (label === "Delete") return tr("gui.precondition.open_before_delete", "Open an archive before deleting entries");
      if (label === "Rename") return tr("gui.precondition.open_before_rename", "Open an archive before renaming entries");
      if (label === "Move") return tr("gui.precondition.open_before_move", "Open an archive before moving entries");
      if (label === "New Folder") return tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder");
      if (label === "Convert") {
        return currentArchive
          ? ""
          : tr("gui.precondition.open_before_convert", "Open an archive before converting");
      }
      return openArchiveFirstLabel();
    }
    if (currentArchive.read_only) return archiveMutationDisabledReason();
    if (label === "Rename") return tr("gui.precondition.select_one_before_rename", "Select exactly one file entry before renaming");
    if (label === "Move") return tr("gui.precondition.select_entries_before_move", "Select entries before moving");
    if (label === "Delete") return tr("gui.precondition.select_entries_before_delete", "Select entries before deleting");
    if (label === "View") return tr("gui.preview.select_one", "Select one entry to open or preview");
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
    return labelWithDisabledReason(classicCommandDisplayLabel(label), classicCommandDisabledTitle(label));
  }

  function classicCommandDisplayLabel(label: string): string {
    if (label === "View") {
      const entryPath = selectedPreviewPath();
      return entryPath ? previewActionLabel(entryPath) : tr("gui.action.open_preview", "Open");
    }
    return label === "Extract To" && hasArchiveSelection()
      ? actionLabel("Extract selected")
      : classicCommandLabel(label);
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
    if (screen === "browse" && !currentArchive) return navLabel("Archives");
    if (screen === "recent") return tr("gui.screen.recent", "Recent Archives");
    if (screen === "create") return tr("gui.screen.create", "Create Archive");
    if (screen === "extract") return tr("gui.screen.extract", "Extract");
    if (screen === "convert") return tr("gui.screen.convert", "Convert Archive");
    if (screen === "batch") return tr("gui.screen.batch", "Batch Extract Review");
    if (screen === "checksum") return tr("gui.screen.checksum", "Checksum");
    if (screen === "duplicates") return tr("gui.screen.duplicates", "Duplicate Finder");
    if (screen === "password") return tr("gui.screen.password", "Password Required");
    if (screen === "conflict") return tr("gui.screen.conflict", "Conflict Handling");
    if (screen === "recovery") return tr("gui.screen.recovery", "Recovery");
    if (screen === "archiveInfo") return tr("gui.screen.archive_info", "Archive Info");
    if (screen === "integration") return tr("gui.screen.integration", "Formats & Integration");
    if (screen === "appearance") return tr("gui.screen.appearance", "Appearance");
    if (screen === "colors") return tr("gui.screen.colors", "Appearance · Theme Colors");
    if (screen === "settingsGeneral") return tr("gui.screen.settings_general", "Settings · General");
    if (screen === "settingsSecurity") return tr("gui.screen.settings_security", "Settings · Security");
    if (screen === "settingsPerformance") return tr("gui.screen.settings_performance", "Settings · Performance");
    if (screen === "passwordBook") return tr("gui.screen.password_book", "Settings · Password Book");
    return archiveTitle();
  }
</script>

{#if !modeSelectionBlocked}
  <ToastHost
    rootClass={`themed-root palette-${activePalette} theme-${activeTheme}`}
    rootVariables={customPaletteVariables()}
    blocked={blockingModalVisible()}
  />
{/if}

{#if appNotice && (!firstRunRequired || taskWindowMode)}
  <div
    class={`app-notice mode-${mode} themed-root palette-${activePalette} theme-${activeTheme}`}
    use:cssVariables={customPaletteVariables()}
    role="status"
    aria-hidden={blockingModalVisible() ? "true" : undefined}
  >{appNotice}</div>
{/if}

{#if macosSfxPublisherTask && LoadedMacosSfxPublisher}
  <LoadedMacosSfxPublisher
    task={macosSfxPublisherTask}
    rootClass={`sfx-publish-overlay design-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`}
    rootVariables={customPaletteVariables()}
    platform={platformKind()}
    previewSkipSave={import.meta.env.DEV && params.has("previewSfxPublisher")}
    chooseOutput={chooseMacosSfxPublishOutput}
    {submitJob}
    formatSubmitError={formatMacosSfxPublishSubmitError}
    onNotice={showNotice}
    onClose={cancelMacosSfxPublisher}
  />
{/if}

{#if taskDialogVisible() && !macosSfxPublisherTask}
  {@const task = taskDialogTask()}
  {#if task}
    <TaskProgressDialogHost
      surface={taskDialogSurface(task)}
      loadingTitle={tr("gui.task_surface.loading", "Loading task view")}
      loadingBody={tr("gui.task_surface.loading_body", "Preparing live progress, results, and task controls.")}
      failureTitle={tr("gui.task_surface.load_failed", "Task view could not be loaded")}
      failureBody={tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}
      retryLabel={tr("gui.task_surface.retry", "Retry view")}
      backLabel={tr("gui.task.back_to_tasks", "Back to tasks")}
    />
  {/if}
{/if}

{#if !taskWindowMode && taskCenterOpen && !blockingModalVisible() && !modeSelectionBlocked}
  {@const selectedTask = taskCenterSelectedTask()}
  {#if selectedTask}
    <TaskProgressDialogHost
      surface={taskCenterDetailSurface(selectedTask)}
      loadingTitle={tr("gui.task_surface.loading", "Loading task view")}
      loadingBody={tr("gui.task_surface.loading_body", "Preparing live progress, results, and task controls.")}
      failureTitle={tr("gui.task_surface.load_failed", "Task view could not be loaded")}
      failureBody={tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}
      retryLabel={tr("gui.task_surface.retry", "Retry view")}
      backLabel={tr("gui.task.back_to_tasks", "Back to tasks")}
    />
  {:else}
    <TaskCenterHost
      surface={taskCenterSurface()}
      loadingTitle={tr("gui.task_surface.center_loading", "Loading task center")}
      loadingBody={tr("gui.task_surface.center_loading_body", "Preparing recent tasks and the shared queue.")}
      failureTitle={tr("gui.task_surface.center_load_failed", "Task center could not be loaded")}
      failureBody={tr("gui.task_surface.center_load_failed_body", "Your tasks are unchanged. Retry loading the task center.")}
      retryLabel={tr("gui.task_surface.retry", "Retry view")}
      closeLabel={tr("gui.task_center.close", "Close task center")}
    />
  {/if}
{/if}

{#if (dragActive || lastDropKind !== "none") && !modeSelectionBlocked}
  <div
    class={`drop-status mode-${mode} themed-root palette-${activePalette} theme-${activeTheme}`}
    use:cssVariables={customPaletteVariables()}
    class:active={dragActive}
    role="status"
    aria-hidden={blockingModalVisible() ? "true" : undefined}
  >{dropStatusLabel()}</div>
{/if}

{#if entryContext}
  <div
    bind:this={entryContextMenu}
    class={`entry-context-menu themed-root palette-${activePalette} theme-${activeTheme}`}
    use:cssVariables={entryContextCssVariables(entryContext)}
    role="menu"
    aria-label={tr("gui.context.actions_for", "Actions for {name}").replace("{name}", entryContext.name)}
    aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined}
    inert={blockingModalVisible() || modeSelectionBlocked}
  >
    <div class="entry-context-head">
      <span>{tr("gui.context.selection_actions", "Selection actions")}</span>
      <strong>{entryContext.name}</strong>
    </div>
    <button role="menuitem" disabled={!currentArchive || Boolean(archiveSelectAllProgress)} title={archiveSelectionBusyReason() || (currentArchive ? "" : openArchiveFirstLabel())} onclick={() => void runEntryContextAction("extract")}><Icon name="archive" size={15} />{actionLabel("Extract selected")}</button>
    <button role="menuitem" disabled={Boolean(archiveMutationDisabledReason()) || !hasArchiveSelection()} title={deleteSelectedDisabledReason()} onclick={() => void runEntryContextAction("delete")}><Icon name="x-circle" size={15} />{actionLabel("Delete selected")}</button>
    <button role="menuitem" disabled={!entryContext.canRename || !canRenameSelection()} title={entryContext.canRename && canRenameSelection() ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")} onclick={() => void runEntryContextAction("rename")}><Icon name="repeat" size={15} />{actionLabel("Rename selected")}</button>
    <button role="menuitem" disabled={Boolean(archiveMutationDisabledReason()) || !hasArchiveSelection()} title={moveSelectedDisabledReason()} onclick={() => void runEntryContextAction("move")}><Icon name="repeat" size={15} />{actionLabel("Move selected")}</button>
    <button role="menuitem" disabled={!entryContext.path} title={entryContext.path ? previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file") : tr("gui.preview.select_one", "Select one entry to open or preview")} onclick={() => void runEntryContextAction("preview")}><Icon name={previewActionIcon(entryContext.path, entryContext.isDir ? "dir" : "file")} size={15} />{previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file")}</button>
    <button role="menuitem" disabled={!currentArchive} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void runEntryContextAction("test")}><Icon name="shield-alert" size={15} />{actionLabel("Test archive")}</button>
  </div>
{/if}

{#if firstRunRequired && !taskWindowMode}
  <div
    class={`first-run-overlay themed-root palette-${activePalette} theme-${activeTheme}`}
    use:cssVariables={customPaletteVariables()}
    role="presentation"
    aria-hidden={blockingModalVisible() ? "true" : undefined}
    inert={blockingModalVisible()}
  >
    <div
      bind:this={firstRunPanel}
      class="first-run-panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="squallz-first-run-title"
      aria-describedby="squallz-first-run-description"
      tabindex="-1"
      onkeydown={onFirstRunKeydown}
    >
      <div class="first-run-brand">
        <AppIcon size={46} title="Squallz" />
        <div>
          <span class="eyebrow">Squallz</span>
          <h1 id="squallz-first-run-title">{tr("gui.first_run.title", "Start with Squallz")}</h1>
          <p id="squallz-first-run-description">{tr("gui.first_run.body", "Modern is ready with safe everyday defaults. Create or open an archive now, and switch to Classic any time.")}</p>
        </div>
      </div>

      <div class="first-run-choices">
        <section class="first-run-modern-card" aria-labelledby="squallz-first-run-modern-title">
          <div class="first-run-mode-head">
            <span class="first-run-mode-mark" aria-hidden="true"><Icon name="sparkles" size={20} /></span>
            <div class="first-run-mode-copy">
              <span class="mode-kicker">{tr("gui.first_run.recommended", "Recommended")}</span>
              <strong id="squallz-first-run-modern-title">{tr("gui.mode.modern", "Modern")}</strong>
              <small>{tr("gui.first_run.modern_body", "A clean workspace for everyday compression and extraction.")}</small>
            </div>
          </div>
          <span class="first-run-mode-features">
            <span><Icon name="sparkles" size={14} />{tr("gui.first_run.modern_focus", "Focused workspace")}</span>
            <span><Icon name="check-circle" size={14} />{tr("gui.first_run.modern_guided", "Guided create and extract")}</span>
          </span>
          <div class="first-run-primary-actions">
            <button bind:this={firstRunRecommendedButton} class="primary" onclick={() => void startFirstRun("create")}>
              <Icon name="sparkles" size={16} />{tr("gui.classic.create_archive", "Create archive")}
            </button>
            <button aria-busy={archiveOpenStatus === "opening"} onclick={() => void startFirstRun("open")}>
              <Icon name="folder-open" size={16} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            </button>
          </div>
        </section>

        <button class="first-run-classic-card" onclick={() => setMode("classic")}>
          <span class="first-run-mode-mark" aria-hidden="true"><Icon name="list" size={20} /></span>
          <span class="first-run-mode-copy">
            <span class="mode-kicker">{tr("gui.first_run.power_workflow", "Detailed controls")}</span>
            <strong>{tr("gui.mode.classic", "Classic")}</strong>
            <small>{tr("gui.first_run.classic_body", "More controls on screen, with a detailed file table and keyboard shortcuts.")}</small>
          </span>
          <span class="first-run-classic-features">
            <span><Icon name="list" size={14} />{tr("gui.first_run.classic_table", "Detailed file table")}</span>
            <span><Icon name="search" size={14} />{tr("gui.first_run.classic_keyboard", "Keyboard-first navigation")}</span>
          </span>
          <span class="first-run-choice-action">
            {tr("gui.first_run.choose_mode", "Use {mode}").replace("{mode}", tr("gui.mode.classic", "Classic"))}
            <Icon name="chevron-right" size={14} />
          </span>
        </button>
      </div>

      <footer class="first-run-footer">
        <span role="status" aria-live="polite" aria-atomic="true">{firstRunDropFeedback ?? tr("gui.first_run.mode_changed_settings", "Mode can be changed in Settings")}</span>
        <button onclick={reviewFirstRunSettings}>{tr("gui.first_run.review_settings_action", "Review settings first")}</button>
      </footer>
    </div>
  </div>
{/if}

{#if taskWindowMode}
  <main
    class={`design-root task-window-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`}
    use:cssVariables={customPaletteVariables()}
    aria-label={tr("gui.external_task.window_label", "Squallz task window")}
    aria-hidden={blockingModalVisible() ? "true" : undefined}
    inert={blockingModalVisible()}
  >
    {#if !blockingModalVisible()}
      <section class="task-window-empty" role="status">
        <AppIcon size={42} title="Squallz" />
        <div>
          <span class="eyebrow">{tr("gui.external_task.eyebrow", "Squallz task")}</span>
          <h1>{taskWindowShellTitleCopy}</h1>
          <p>{taskWindowShellCopy}</p>
        </div>
      </section>
    {/if}
  </main>
{:else if mode === "modern" || isSettingsScreen()}
  <main class={`design-root modern-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`} use:cssVariables={customPaletteVariables()} class:drop-active={dragActive} aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined} inert={blockingModalVisible() || modeSelectionBlocked}>
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
            <button onclick={() => openRecoveryConfiguration()}><Icon name="shield-alert" size={16} />{tr("gui.recovery.title", "Recovery")}</button>
            <button class="primary" disabled={!currentArchive} title={archiveActionTitle(hasArchiveOpen())} onclick={() => openExtractWorkspace("all")}><Icon name="archive" size={16} />{toolbarLabel("Extract")}</button>
            <button
              bind:this={quickActionButton}
              class="icon-only"
              aria-label={tr("gui.quick.title", "Quick actions")}
              aria-haspopup="dialog"
              aria-expanded={activePopover === "quickActions"}
              onclick={toggleQuickActions}
            ><Icon name="sparkles" size={16} /></button>
          {/if}
          <button
            class="icon-only task-center-trigger"
            class:attention={taskCenterHasAttention()}
            aria-label={taskCenterTriggerLabel()}
            title={taskCenterTriggerLabel()}
            aria-expanded={taskCenterOpen}
            aria-controls="squallz-task-center"
            onclick={(event) => openTaskCenter(event.currentTarget)}
          >
            <Icon name="list" size={16} />
            {#if taskCenterBadgeCount() > 0}
              <span class="task-center-trigger-badge">{Math.min(taskCenterBadgeCount(), 99)}</span>
            {/if}
          </button>
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
        class:no-inspector-shell={screen === "recent" || screen === "convert" || screen === "create" || screen === "extract"}
      >
        <aside class="modern-sidebar" aria-label={tr("gui.aria.navigation", "Navigation")}>
          <div class="sidebar-section">
            {#each nav as item}
              <button
                class:current={(screen === "recent" && item[1] === "Recent") || (screen === "browse" && item[1] === "Archives") || (screen === "create" && item[1] === "Create") || ((screen === "extract" || screen === "batch" || screen === "password" || screen === "conflict") && item[1] === "Extract") || (screen === "convert" && item[1] === "Convert") || (screen === "checksum" && item[1] === "Checksum") || (screen === "duplicates" && item[1] === "Duplicates") || (screen === "recovery" && item[1] === "Recovery") || (isSettingsScreen() && item[1] === "Settings")}
                onclick={() => navigateToScreen(screenForNav(item[1]))}
              >
                <Icon name={item[0]} size={16} />
                <span>{navLabel(item[1])}</span>
              </button>
            {/each}
          </div>
          {#if !hideOperationHistory}
            <button
              class="recent-card history-card"
              type="button"
              aria-expanded={taskCenterOpen}
              aria-controls="squallz-task-center"
              aria-label={taskCenterTriggerLabel()}
              onclick={(event) => openTaskCenter(event.currentTarget)}
            >
              <span>{tr("gui.task_center.title", "Task center")}</span>
              <strong>{taskCenterSummaryLabel()}</strong>
              <small>{historyLastLabel()}</small>
            </button>
          {/if}
        </aside>

        <section
          class="modern-content"
          class:settings-workspace={isSettingsScreen()}
          class:browse-workspace={screen === "browse"}
          aria-label={isSettingsScreen() ? tr("gui.settings.workspace", "Settings workspace") : tr("gui.aria.archive_contents", "Archive contents")}
        >
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
                  <p>{tr("gui.recent.subtitle", "Reopen recent archives. Squallz stores paths only, never archive contents.")}</p>
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
            <ArchiveOperationWorkspaceHost
              kind="convert"
              variant="modern"
              owner={convertRouteOwner}
              bridge={convertRouteBridge}
              loadingTitle={tr("gui.convert.workspace_loading", "Loading Convert")}
              loadingBody={tr("gui.convert.workspace_loading_body", "Preparing output formats and compression choices.")}
              failureTitle={tr("gui.convert.workspace_load_failed", "Convert could not be loaded")}
              failureBody={tr("gui.convert.workspace_load_failed_body", "The open archive and conversion choices were not changed. Retry loading the convert workspace.")}
              retryLabel={tr("gui.convert.workspace_retry", "Retry")}
            />
          {:else if screen === "create"}
            <ArchiveOperationWorkspaceHost
              kind="create"
              variant="modern"
              surface={createWorkspaceSurface("modern")}
              loadingTitle={tr("gui.create.workspace_loading", "Loading Create")}
              loadingBody={tr("gui.create.workspace_loading_body", "Preparing formats, presets, and output options.")}
              failureTitle={tr("gui.create.workspace_load_failed", "Create could not be loaded")}
              failureBody={tr("gui.create.workspace_load_failed_body", "Your create settings and selected sources were not changed. Retry loading the create workspace.")}
              retryLabel={tr("gui.create.workspace_retry", "Retry")}
            />
          {:else if screen === "extract"}
            <ArchiveOperationWorkspaceHost
              kind="extract"
              variant="modern"
              surface={extractWorkspaceSurface("modern")}
              loadingTitle={tr("gui.extract.workspace_loading", "Loading Extract")}
              loadingBody={tr("gui.extract.workspace_loading_body", "Preparing the destination, write plan, and safety options.")}
              failureTitle={tr("gui.extract.workspace_load_failed", "Extract could not be loaded")}
              failureBody={tr("gui.extract.workspace_load_failed_body", "Your destination and safety choices were not changed. Retry loading the extract workspace.")}
              retryLabel={tr("gui.extract.workspace_retry", "Retry")}
            />
          {:else if screen === "batch"}
            <ToolsWorkspaceHost surface={batchWorkspaceSurface("modern")} />
          {:else if screen === "checksum"}
            <ToolsWorkspaceHost surface={checksumWorkspaceSurface("modern")} />
          {:else if screen === "duplicates"}
            <ToolsWorkspaceHost surface={duplicatesWorkspaceSurface("modern")} />
          {:else if screen === "password"}
            <TaskInteractionWorkspaceHost
              surface={taskInteractionWorkspaceSurface("modern", "password")}
            />
          {:else if screen === "conflict"}
            <TaskInteractionWorkspaceHost
              surface={taskInteractionWorkspaceSurface("modern", "conflict")}
            />
          {:else if screen === "recovery"}
            <RecoveryWorkspaceHost
              variant="modern"
              view={recoveryWorkspaceView()}
              actions={recoveryWorkspaceActions}
              {tr}
            />
          {:else if screen === "archiveInfo"}
            <div class="settings-view modern-archive-info">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.archive.info_eyebrow", "Archive / Info")}</span>
                  <h1>{tr("gui.archive.info_title", "Archive information")}</h1>
                  <p>{tr("gui.archive.info_subtitle", "Archive, selection, destination, encoding, and volume details.")}</p>
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
                  <div class="panel-title"><Icon name="archive" size={16} />{extractDestinationFieldLabel()}</div>
                  <div class="setting-callout">
                    <strong>{extractDestinationTitle(extractDestinationMode)}</strong>
                    <span>{effectiveExtractDest()}</span>
                  </div>
                  <div class="settings-route-list">
                    <button class="settings-route-card" onclick={() => openExtractWorkspace("all")}>
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
          {:else if isSettingsWorkspaceScreen(screen)}
            <SettingsWorkspaceHost
              workspace={settingsWorkspaceProps(screen)}
              loadingTitle={tr("gui.settings.workspace_loading", "Loading Settings")}
              loadingBody={tr("gui.settings.workspace_loading_body", "Preparing appearance, safety, performance, and integration controls.")}
              failureTitle={tr("gui.settings.workspace_load_failed", "Settings could not be loaded")}
              failureBody={tr("gui.settings.workspace_load_failed_body", "No settings were changed. Retry loading this workspace.")}
              retryLabel={tr("gui.settings.workspace_retry", "Retry")}
            />
	          {:else}
	            <div class="archive-workspace" class:no-archive={!currentArchive}>
	              {#if currentArchive}
	                <ModernArchiveBrowserHost
	                  surface={modernArchiveBrowserSurface()}
	                  loadingTitle={tr("gui.archive.browser_loading", "Loading archive browser")}
	                  loadingBody={tr("gui.archive.browser_loading_body", "Preparing the file list and archive actions.")}
	                  failureTitle={tr("gui.archive.browser_load_failed", "Archive browser could not be loaded")}
	                  failureBody={tr("gui.archive.browser_load_failed_body", "The open archive and your selection are unchanged. Retry loading the browser.")}
	                  retryLabel={tr("gui.archive.browser_retry", "Retry browser")}
	                />
		            {:else}
	              <ArchiveStartState
	                variant="modern"
	                eyebrow={tr("gui.archive.secure_archive", "Secure archive")}
	                title={noArchiveLabel()}
	                body={tr("gui.empty.no_archive_summary", "Open an archive to browse entries, inspect metadata, and run archive actions.")}
	                openLabel={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
	                createLabel={toolbarLabel("Create")}
	                openBusy={archiveOpenStatus === "opening"}
	                onOpen={() => void openArchiveFromDialog()}
	                onCreate={() => setScreen("create")}
		              />
		            {/if}
	            </div>
	          {/if}
        </section>

        {#if !isSettingsScreen() && screen !== "recent" && screen !== "convert" && screen !== "create" && screen !== "extract" && (screen !== "browse" || currentArchive)}
          <ModernInspectorHost
            surface={modernInspectorSurface()}
            ariaLabel={tr("gui.aria.archive_inspector", "Archive inspector")}
            loadingTitle={tr("gui.inspector.workspace_loading", "Loading inspector")}
            loadingBody={tr("gui.inspector.workspace_loading_body", "Preparing archive details and selection actions.")}
            failureTitle={tr("gui.inspector.workspace_load_failed", "Inspector could not be loaded")}
            failureBody={tr("gui.inspector.workspace_load_failed_body", "The archive and your selection are unchanged. Retry loading the inspector.")}
            retryLabel={tr("gui.inspector.workspace_retry", "Retry inspector")}
          />
        {/if}
      </div>
    </section>
  </main>
{:else}
  <main class={`design-root classic-root platform-${activePlatform} palette-${activePalette} theme-${activeTheme} density-${activeDensityChoice}`} use:cssVariables={customPaletteVariables()} class:drop-active={dragActive} aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined} inert={blockingModalVisible() || modeSelectionBlocked}>
    <section class="window classic-window" aria-label={tr("gui.aria.classic_archive_browser", "Squallz Classic archive browser")}>
      <header class="classic-titlebar" data-tauri-drag-region>
        <div class="classic-title">
          <AppIcon size={19} title="Squallz" />
          <strong>{titleForScreen()}</strong>
          <span>Squallz Classic</span>
        </div>
        <div class="classic-top-actions">
          <button
            aria-busy={archiveOpenStatus === "opening"}
            aria-label={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            title={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            onclick={() => void openArchiveFromDialog()}
          >
            <Icon name="folder-open" size={15} />
            <span class="classic-action-label">{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</span>
          </button>
          <button
            aria-label={tr("gui.classic.new_archive", "New archive")}
            title={tr("gui.classic.new_archive", "New archive")}
            onclick={() => setScreen("create")}
          >
            <Icon name="archive" size={15} />
            <span class="classic-action-label">{tr("gui.classic.new_archive", "New archive")}</span>
          </button>
          {#if nestedPreview}
            <button
              aria-label={tr("gui.action.open_nested", "Open")}
              title={tr("gui.action.open_nested", "Open")}
              onclick={() => void openNestedPreviewArchive()}
            >
              <Icon name="folder-open" size={15} />
              <span class="classic-action-label">{tr("gui.action.open_nested", "Open")}</span>
            </button>
            <button
              aria-label={tr("gui.action.extract_nested", "Extract")}
              title={tr("gui.action.extract_nested", "Extract")}
              onclick={() => void extractNestedPreviewArchive()}
            >
              <Icon name="archive" size={15} />
              <span class="classic-action-label">{tr("gui.action.extract_nested", "Extract")}</span>
            </button>
          {/if}
          <button
            class="task-center-trigger"
            class:attention={taskCenterHasAttention()}
            aria-label={taskCenterTriggerLabel()}
            title={taskCenterTriggerLabel()}
            aria-expanded={taskCenterOpen}
            aria-controls="squallz-task-center"
            onclick={(event) => openTaskCenter(event.currentTarget)}
          >
            <Icon name="list" size={15} />
            <span class="classic-action-label">{tr("gui.task_center.short_title", "Tasks")}</span>
            {#if taskCenterBadgeCount() > 0}
              <span class="task-center-trigger-badge">{Math.min(taskCenterBadgeCount(), 99)}</span>
            {/if}
          </button>
          <button
            aria-label={navLabel("Settings")}
            title={navLabel("Settings")}
            onclick={() => setScreen("settingsGeneral")}
          >
            <Icon name="settings" size={15} />
            <span class="classic-action-label">{navLabel("Settings")}</span>
          </button>
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
              <span><Icon name={command[1] === "View" ? previewActionIcon() : command[0]} size={23} /></span>
              <strong>{classicCommandDisplayLabel(command[1])}</strong>
            </button>
          {/each}
        </div>

        {#if !classicArchiveStartVisible()}
          <div
            class="classic-pathrow"
            class:has-move={screen === "browse" && hasArchiveSelection()}
            class:empty-address={!currentArchive}
          >
          <div class="classic-path-navigation" aria-label={tr("gui.nav.archive_navigation", "Archive navigation")}>
            <button
              class="path-button"
              disabled={!canGoUpArchive()}
              aria-label={tr("gui.nav.up", "Up one level")}
              title={tr("gui.nav.up", "Up one level")}
              onclick={() => void goArchiveUp()}
            ><Icon name="chevron-up" size={14} /><span>{tr("gui.nav.up_short", "Up")}</span></button>
            <button
              class="path-button"
              disabled={!canGoUpArchive()}
              aria-label={tr("gui.nav.root", "Archive root")}
              title={tr("gui.nav.root", "Archive root")}
              onclick={() => void openArchiveBreadcrumb(-1)}
            ><Icon name="archive" size={14} /><span>{tr("gui.nav.root_short", "Root")}</span></button>
          </div>
          <div class="address" bind:this={classicArchiveAddress}>
            <button type="button" disabled={!currentArchive} title={archiveTitle()} onclick={() => void openArchiveBreadcrumb(-1)}>{archiveTitle()}</button>
            {#each archiveDirs as dir, index}
              <i>/</i><button type="button" title={dir} onclick={() => void openArchiveBreadcrumb(index)}>{dir}</button>
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
          {:else if screen === "recovery"}
            <div class="encoding-chip accent"><Icon name="shield-alert" size={14} />{recoverySourceName() ?? tr("gui.recovery.no_archive_selected", "No archive selected")}</div>
          {:else if screen === "checksum"}
            <div class="encoding-chip accent"><Icon name="check-circle" size={14} />{checksumAlgorithmLabel(checksumAlgorithm)}</div>
          {:else if screen === "duplicates"}
            <div class="encoding-chip accent"><Icon name="search" size={14} />{tr("gui.duplicates.blake3_scan", "BLAKE3 scan")}</div>
          {:else if currentArchive && hasArchiveStructureWarning()}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{tr("gui.archive.zip_index_damaged", "ZIP index damaged")}</div>
          {:else if currentArchive && hasEncodingWarning()}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{tr("gui.encoding.gbk_suggested", "GBK suggested")}</div>
          {:else if currentArchive}
            <div
              class="encoding-chip accent archive-state-chip"
              aria-label={tr("gui.archive.open", "Archive open")}
              title={tr("gui.archive.open", "Archive open")}
            ><Icon name="archive" size={14} /><span>{tr("gui.archive.open", "Archive open")}</span></div>
          {:else}
            <div class="encoding-chip accent"><Icon name="folder-open" size={14} />{openArchiveFirstLabel()}</div>
          {/if}
          <div class="classic-path-tools">
            <div class="classic-search" class:searching={Boolean(filterText().trim())} role="search" title={archiveFilterStatus()}>
              <Icon name="search" size={14} />
              <input
                bind:this={archiveSearchInput}
                value={filterText()}
                disabled={!currentArchive || screen !== "browse"}
                aria-label={tr("gui.list.search_aria", "Search paths across the entire archive")}
                aria-busy={filterPending()}
                title={tr("gui.list.search_shortcut", "Search the entire archive (⌘F / Ctrl+F)")}
                placeholder={tr("gui.list.search_placeholder", "Search the entire archive")}
                oninput={(event) => updateArchiveFilter(event.currentTarget.value)}
                onkeydown={onArchiveFilterKeydown}
              />
              {#if filterText()}
                <button type="button" aria-label={tr("gui.list.search_clear", "Clear search")} title={tr("gui.list.search_clear", "Clear search")} onclick={clearArchiveFilter}><Icon name="x-circle" size={13} /></button>
              {/if}
            </div>
            <button
              bind:this={quickActionButton}
              class="classic-quick-trigger"
              aria-label={tr("gui.quick.title", "Quick actions")}
              title={tr("gui.quick.title", "Quick actions")}
              aria-haspopup="dialog"
              aria-expanded={activePopover === "quickActions"}
              onclick={toggleQuickActions}
            ><Icon name="sparkles" size={14} /></button>
          </div>
          </div>
        {/if}

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
                <p>{tr("gui.archive.info_subtitle", "Archive, selection, destination, encoding, and volume details.")}</p>
              </div>
              <div class="classic-button-row">
                <button onclick={() => setScreen("browse")}>{tr("gui.nav.back_to_archive", "Back to archive")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(extractArchiveRequiredReason())}
                  title={extractArchiveRequiredReason()}
                  aria-label={labelWithDisabledReason(tr("gui.screen.extract", "Extract"), extractArchiveRequiredReason())}
                  onclick={() => openExtractWorkspace("all")}
                >{tr("gui.screen.extract", "Extract")}</button>
              </div>
            </header>
            <div class="classic-batch-grid">
              <section>
                <h2>{tr("gui.inspector.archive", "Archive")}</h2>
                <div class="classic-form-grid compact">
                  {#each archiveInfoRows() as row}
                    <div class="classic-label">{row[0]}</div>
                    <div class="classic-input classic-copy-wrap">{row[1]}</div>
                  {/each}
                </div>
              </section>
              <aside>
                <h2>{extractDestinationFieldLabel()}</h2>
                <div class="classic-form-grid compact no-pad">
                  <div class="classic-label">{tr("common.mode", "Mode")}</div>
                  <div class="classic-input accent">{extractDestinationTitle(extractDestinationMode)}</div>
                  <div class="classic-label">{tr("common.destination", "Destination")}</div>
                  <div class="classic-input classic-copy-wrap">{effectiveExtractDest()}</div>
                </div>
              </aside>
            </div>
          </section>
        </div>
      {:else if screen === "convert"}
        <ArchiveOperationWorkspaceHost
          kind="convert"
          variant="classic"
          owner={convertRouteOwner}
          bridge={convertRouteBridge}
          loadingTitle={tr("gui.convert.workspace_loading", "Loading Convert")}
          loadingBody={tr("gui.convert.workspace_loading_body", "Preparing output formats and compression choices.")}
          failureTitle={tr("gui.convert.workspace_load_failed", "Convert could not be loaded")}
          failureBody={tr("gui.convert.workspace_load_failed_body", "The open archive and conversion choices were not changed. Retry loading the convert workspace.")}
          retryLabel={tr("gui.convert.workspace_retry", "Retry")}
        />
      {:else if screen === "create"}
        <ArchiveOperationWorkspaceHost
          kind="create"
          variant="classic"
          surface={createWorkspaceSurface("classic")}
          loadingTitle={tr("gui.create.workspace_loading", "Loading Create")}
          loadingBody={tr("gui.create.workspace_loading_body", "Preparing formats, presets, and output options.")}
          failureTitle={tr("gui.create.workspace_load_failed", "Create could not be loaded")}
          failureBody={tr("gui.create.workspace_load_failed_body", "Your create settings and selected sources were not changed. Retry loading the create workspace.")}
          retryLabel={tr("gui.create.workspace_retry", "Retry")}
        />
      {:else if screen === "extract"}
        <ArchiveOperationWorkspaceHost
          kind="extract"
          variant="classic"
          surface={extractWorkspaceSurface("classic")}
          loadingTitle={tr("gui.extract.workspace_loading", "Loading Extract")}
          loadingBody={tr("gui.extract.workspace_loading_body", "Preparing the destination, write plan, and safety options.")}
          failureTitle={tr("gui.extract.workspace_load_failed", "Extract could not be loaded")}
          failureBody={tr("gui.extract.workspace_load_failed_body", "Your destination and safety choices were not changed. Retry loading the extract workspace.")}
          retryLabel={tr("gui.extract.workspace_retry", "Retry")}
        />
      {:else if screen === "batch"}
        <ToolsWorkspaceHost surface={batchWorkspaceSurface("classic")} />
      {:else if screen === "checksum"}
        <ToolsWorkspaceHost surface={checksumWorkspaceSurface("classic")} />
      {:else if screen === "duplicates"}
        <ToolsWorkspaceHost surface={duplicatesWorkspaceSurface("classic")} />
      {:else if screen === "password"}
        <TaskInteractionWorkspaceHost
          surface={taskInteractionWorkspaceSurface("classic", "password")}
        />
      {:else if screen === "conflict"}
        <TaskInteractionWorkspaceHost
          surface={taskInteractionWorkspaceSurface("classic", "conflict")}
        />
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
          <RecoveryWorkspaceHost
            variant="classic"
            view={recoveryWorkspaceView()}
            actions={recoveryWorkspaceActions}
            {tr}
          />
        </div>
      {:else}
        {#if classicArchiveStartVisible()}
          <div class="classic-start-body">
            <ArchiveStartState
              variant="classic"
              eyebrow={tr("gui.archive.secure_archive", "Secure archive")}
              title={noArchiveLabel()}
              body={tr("gui.empty.no_archive_summary", "Open an archive to browse entries, inspect metadata, and run archive actions.")}
              openLabel={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
              createLabel={tr("gui.classic.create_archive", "Create archive")}
              openBusy={archiveOpenStatus === "opening"}
              onOpen={() => void openArchiveFromDialog()}
              onCreate={() => setScreen("create")}
            />
          </div>
        {:else}
          <ClassicArchiveBrowserHost
            surface={classicArchiveBrowserSurface()}
            loadingTitle={tr("gui.archive.browser_loading", "Loading archive browser")}
            loadingBody={tr("gui.archive.browser_loading_body", "Preparing the file list and archive actions.")}
            failureTitle={tr("gui.archive.browser_load_failed", "Archive browser could not be loaded")}
            failureBody={tr("gui.archive.browser_load_failed_body", "The open archive and your selection are unchanged. Retry loading the browser.")}
            retryLabel={tr("gui.archive.browser_retry", "Retry browser")}
          />
        {/if}
      {/if}

      <footer class="classic-statusbar">
        {#if screen === "create"}
          <span>{lastCreatePlan ? tr("gui.create.source_files_count", "{count} source files").replace("{count}", lastCreatePlan.files.toLocaleString()) : tr("gui.create.source_files_pending", "Source files pending")}</span>
          <span>{createSfxEnabled ? createSfxOutputLabel() : activeCreateFormatData().label} · {createMethodLabel()}</span>
          <span>{createSplitCapability()} · {createRecoveryCapability()}</span>
          <strong>{diskPreflightStatusbar()}</strong>
        {:else if screen === "convert"}
          <span>{tr("gui.convert.title", "Convert archive")}</span>
          <span>{tr("gui.convert.status_source_target", "{source} → {target}")
            .replace("{source}", convertRouteStatus.sourceFormat)
            .replace("{target}", convertRouteStatus.targetLabel)}</span>
          <span>{convertRouteStatus.profileLabel} · {convertRouteStatus.methodLabel}</span>
          <strong>{tr("gui.convert.status_destination", "Destination: {destination}").replace("{destination}", convertRouteStatus.destination)}</strong>
        {:else if screen === "extract"}
          {@const extractActionStatus = extractActionLabel()}
          {@const extractScopeStatus = `${extractSelectionLabel()} · ${extractDestinationTitle(extractDestinationMode)}`}
          {@const extractConflictStatus = tr("gui.extract.status_conflicts", "Conflicts: {mode} · {password}").replace("{mode}", currentExtractOverwriteLabel).replace("{password}", extractPasswordLabel())}
          {@const extractConflictCompactStatus = tr("gui.extract.status_conflicts", "Conflicts: {mode} · {password}").replace("{mode}", currentExtractOverwriteLabel).replace("{password}", extractPasswordStatusbarLabel())}
          {@const extractDestinationStatus = tr("gui.extract.status_destination", "Destination: {destination}").replace("{destination}", effectiveExtractDest())}
          <span title={extractActionStatus}>{extractActionStatus}</span>
          <span title={extractScopeStatus}>{extractScopeStatus}</span>
          <span title={extractConflictStatus}>{extractConflictCompactStatus}</span>
          <strong title={extractDestinationStatus}>{extractDestinationStatus}</strong>
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
	        {:else if screen === "recovery"}
	          <span>{tr("gui.recovery.status_par2_sidecar", "Recovery: PAR2 sidecar")}</span>
	          <span>{recoverySourceName() ?? tr("gui.recovery.no_archive_selected", "No archive selected")}</span>
	          <span>{recoveryResultAvailable() ? recoveryResultDetail() : tr("gui.recovery.verify_first", "Verify first")}</span>
	          <strong>{recoveryResultFooter()}</strong>
        {:else if screen === "archiveInfo"}
          <span>{tr("gui.screen.archive_info", "Archive Info")}</span>
          <span>{currentArchive ? archiveTitle() : openArchiveFirstLabel()}</span>
          <span>{currentArchive ? archiveFormat() : "-"}</span>
          <strong>{extractDestinationHint()}</strong>
        {:else if classicArchiveStartVisible()}
          <span role="status" aria-live="polite">{navLabel("Archives")}</span>
          <span></span>
          <span></span>
          <strong>{currentTaskStatusLabel()}</strong>
        {:else}
          <span role="status" aria-live="polite">{currentArchive ? archiveFilterStatus() : noArchiveLabel()}</span>
          <span>{selectedSummary()}</span>
          <span>{currentArchive ? `${archiveFormat()} · ${currentArchive.volumes?.length ? archiveVolumeCountLabel(currentArchive.volumes.length) : tr("gui.archive.single_file", "single file")}` : openArchiveFirstLabel()}</span>
          <strong>{currentTaskStatusLabel()}</strong>
        {/if}
      </footer>
    </section>
  </main>
{/if}
