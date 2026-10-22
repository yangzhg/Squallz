<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import {
    SettingsSession,
    isSettingsPersistenceFailure,
    type SettingsEffect,
    type SettingsSaveState,
  } from "./lib/settings-session.svelte";
  import { ExtractWorkspaceSession } from "./lib/extract-workspace.svelte";
  import { ExtractPresentation } from "./lib/extract-presentation";
  import {
    type ExtractScope,
    type ExtractTaskDraft,
  } from "./lib/extract-options-draft.svelte";
  import { CreateSession } from "./lib/create-session.svelte";
  import ArchiveStartState from "./components/ArchiveStartState.svelte";
  import ArchiveReturnStrip from "./components/ArchiveReturnStrip.svelte";
  import ArchiveEntryEditor from "./components/ArchiveEntryEditor.svelte";
  import type { ArchiveEditingState } from "./components/ArchiveEditingStatus.svelte";
  import AppIcon from "./components/AppIcon.svelte";
  import ClassicArchiveBrowserHost from "./components/ClassicArchiveBrowserHost.svelte";
  import type { ArchiveBrowserActions, ArchiveBrowserView, ArchivePreviewActions, ArchivePreviewView } from "./lib/archive-browser";
  import type { ClassicArchiveBrowserSurfaceProps } from "./components/ClassicArchiveBrowserHost.svelte";
  import ArchiveOperationWorkspaceHost from "./components/ArchiveOperationWorkspaceHost.svelte";
  import type {
    ExtractWorkspaceBridge,
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
  import PasswordWorkspaceHost from "./components/PasswordWorkspaceHost.svelte";
  import type {
    PasswordWorkspaceSurface,
    PasswordWorkspaceVariant,
  } from "./components/PasswordWorkspaceHost.svelte";
  import TaskProgressDialogHost from "./components/TaskProgressDialogHost.svelte";
  import type { TaskProgressDialogSurfaceProps } from "./components/TaskProgressDialogHost.svelte";
  import type MacosSfxPublisherComponent from "./components/MacosSfxPublisher.svelte";
  import ToastHost from "./components/ToastHost.svelte";
  import ToolsWorkspaceHost from "./components/ToolsWorkspaceHost.svelte";
  import type {
    BatchWorkspaceSurface,
    ArchiveUpdateWorkspaceSurface,
    ChecksumResultKind,
    ChecksumWorkspaceSurface,
    DuplicatesWorkspaceSurface,
    ToolsWorkspaceVariant,
  } from "./components/ToolsWorkspaceHost.svelte";
  import {
    operationHistory,
    recordOperation,
  } from "./lib/history.svelte";
  import { ToolsSession } from "./lib/tools-session.svelte";
  import { ArchivePresetSession, archivePresetDisplayName } from "./lib/archive-presets.svelte";
  import { createContentPolicyLabel, extractOverwriteLabel } from "./lib/archive-option-labels";
  import { archiveSelectionRoots } from "./lib/archive-editing";
  import { ArchiveEditSession, type ArchiveEditKind, type ArchiveEditOwner, type ArchiveMoveReview, type ArchiveEditEffect } from "./lib/archive-edit-session.svelte";
  import { archiveKeyboardTarget } from "./lib/archive-keyboard";
  import { appActionAvailability, appActionForShortcut, dispatchAppAction, isTextEditingTarget, type AppAction } from "./lib/app-actions";
  import { connectNativeMenu, type NativeMenuConnection } from "./lib/native-menu";
  import { trapModalFocus } from "./lib/modal-focus";
  import type { UpdateCheckPreview } from "./lib/app-update.svelte";
  import {
    adoptOpenedArchive,
    archive,
    archiveBrowseError,
    archiveRefreshStatus,
    archiveOpenError,
    archivePasswordBookStatus,
    archivePasswordSave,
    archivePasswordSaveBusy,
    archiveEncoding,
    rememberArchivePassword,
    cancelArchivePasswordSave,
    clearArchivePasswordSave,
    allCurrentRowsSelected,
    cancelPendingArchiveOpen,
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
    openArchive as openArchiveStore,
    openPasswordPrompt,
    reopenWithEncoding,
    refreshCurrentArchive,
    retryArchiveBrowse,
    refreshArchivePasswordBookStatus,
    recentFiles,
    rememberRecent,
    PAGE_SIZE as ARCHIVE_PAGE_SIZE,
    prefetchAround,
    rowAt,
    selectAllRows,
    selectRangeTo,
    selectRow,
    selectedPaths,
    selectedSize,
    setFilter,
    toggleSelect,
    totalRows,
    type RowSelectionResult,
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
    type ExtractPlanPreflightDto,
    type EntryPreviewDto,
    type EntryDto,
    type NestedArchivePasswords,
    type JobSpec,
    type NamedArchivePreset,
    type LanguageDto,
    type SettingsDto,
    type SourceCleanupRecoveryNotice,
    type FormatDto,
  } from "./lib/ipc";
  import {
    previewSampleForEntry,
    nestedPasswordPreviewSample,
    preparedNestedPreviewRows,
    readRuntimePreviews,
  } from "./lib/dev-preview-data";
  import {
    basename as pathBaseName,
    dirname as pathDir,
    formatBytes,
    formatModified,
  } from "./lib/format";
  import type {
    ConvertRouteBridge,
    ConvertRouteHandle,
    ConvertTaskDraft,
    ConvertRouteOwner,
    ConvertRouteStatus,
  } from "./lib/convert-route";
  import {
    joinDesktopPath,
    normalizeDesktopFolder,
    sameDesktopPath,
  } from "./lib/desktop-path";
  import {
    createSourcePaths,
    mergeCreateSources,
  } from "./lib/create-sources";
  import {
    type CreatePreflightEffect,
  } from "./lib/create-preflight.svelte";
  import { platformTrashName } from "./lib/platform-labels";
  import { cssVariables, type CssVariableMap } from "./lib/css-variables";
  import { readWindowRecovery, rememberWindowRecovery, reloadWindow } from "./lib/window-recovery";
  import {
    externalOpenAction,
    type ExternalOpenAction,
  } from "./lib/external-tasks";
  import {
    taskWindowLaunchStateFromParams,
    taskWindowRecoveryState,
    taskWindowTask,
    taskWindowShellMessage,
    taskWindowShellTitle,
    taskWindowSubmitFailureStatus,
    taskWindowSubmitPlan,
    taskWindowSubmitTransition,
    type TaskWindowSubmitTransition,
  } from "./lib/task-window";
  import { allFormats, formatRegistryStatus, loadFormats } from "./lib/formats.svelte";
  import { currentLang, i18nReady, listBundledLanguages, loadLocale, t, tError, tFallback as tr } from "./lib/i18n.svelte";
  import { pushToast, removeToastByKey } from "./lib/toasts.svelte";
  import { isNewSourceCleanupRecoveryGeneration } from "./lib/source-cleanup";
  import { currentWebviewWindowListener } from "./lib/tauri-events";
  import { outputPasswordRequired } from "./lib/job-snapshot";

  import { reviewNestedExtract, type NestedExtractDraft } from "./lib/nested-extract";
  import { ArchiveUpdateReview } from "./lib/archive-update.svelte";
  import { previewSystemOpenRequiresConfirmation } from "./lib/preview-presentation";
  import { PreviewSession, type PreviewPreparation } from "./lib/preview-session.svelte";
  import {
    activeTask,
    answerConflict as answerJobConflict,
    answerPassword as answerJobPassword,
    cancelTask,
    clearFinished,
    initJobEvents,
    jobSnapshotStatus,
    pauseTask,
    pendingConflict,
    pendingPassword,
    resumeTask,
    installActiveTaskPreview,
    installCompletedTaskPreview,
    previewTaskSpecForReview,
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
    isTaskActiveState,
    normalizeTaskConflictAnswer,
    taskPasswordReady,
    taskOutcomeStateLabel,
    taskStatusUnavailable,
    taskReviewScreen,
    taskOutputCanOpen,
    taskOutputIsFolder,
    taskOutputPath,
    taskHasInlineResults,
    taskResultScreen,
    type TaskConflictDecision,
    type TaskDialogModel,
  } from "./lib/task-model";
  import { RecoverySession } from "./lib/recovery-session.svelte";
  import {
    activeUiMode,
    initUiMode,
    setUiMode as persistUiMode,
    uiModeChoice,
    type UiMode,
  } from "./lib/uiMode.svelte";
  import { deriveCustomPaletteTokens } from "./lib/theme";
  import {
    classicCommands,
    nav,
    paletteIds,
    quickActions,
    screenIds,
    settingsSections,
  } from "./lib/ui-model";
  import type {
    FormatCapabilityCard,
    DensityChoice,
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
  type MacosSfxPublisherComponentType = typeof MacosSfxPublisherComponent;
  type ThemeChoice = "system" | "light" | "dark";
  type AppearanceSetting = "mode" | "theme" | "density";
  type AppearanceSaveState = Exclude<SettingsSaveState, "dirty">;
  type ArchiveTaskReview = { path: string; encoding: string | null; restore: () => boolean };
  class JobSubmitBlockedError extends Error {
    readonly reason: TaskSubmissionBlockReason;

    constructor(reason: TaskSubmissionBlockReason) {
      super(`job-submit-blocked:${reason}`);
      this.reason = reason;
    }
  }

  const devToolsChordKeys = new Set(["i", "j", "c"]);
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
    encoding: string;
    attr: string;
    source?: EntryDto;
    virtualIndex?: number;
  };
  type EntryContext = {
    x: number;
    y: number;
    name: string;
    path: string | null;
    canRename: boolean;
    isDir: boolean;
  };
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
  type ValidationWindow = Window & {
    __squallzValidationSetScreen?: (next: Screen) => boolean;
    __squallzValidationJobSubmitAttempts?: number;
    __squallzValidationJobSubmitBlockedWhileStarting?: number;
  };

  const params = new URLSearchParams(window.location.search);
  const windowRecovery = readWindowRecovery(params);
  const modeParam = params.get("mode");
  const defaultExtractDirParam = params.get("defaultExtractDir");
  const initialMode: Mode | null = modeParam === "classic" || modeParam === "modern" ? modeParam : null;
  const forceFirstRun = params.get("firstRun") === "1" || modeParam === "unset";
  const runtimePreviews = readRuntimePreviews(params, ARCHIVE_PAGE_SIZE);
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
  let passwordSaveState = $derived(archivePasswordSave());
  let jobRows = $derived(tasks());
  let activeCurrentTask = $derived(activeTask());
  let jobPasswordPrompt = $derived(pendingPassword());
  const previewSession = new PreviewSession();
  let previewPasswordPrompt = $derived(previewSession.passwordPrompt);
  let archivePasswordPrompt = $derived(openPasswordPrompt());
  let activeJobPasswordPromptIdentity = $derived(
    jobPasswordPrompt ? `${jobPasswordPrompt.id}:${jobPasswordPrompt.version}` : null,
  );
  let previousJobPasswordPromptIdentity: string | null = null;
  let activeWorkspacePasswordPromptIdentity = $derived(
    archivePasswordPrompt
      ? `archive:${archivePasswordPrompt.path}`
      : previewPasswordPrompt
        ? `preview:${previewPasswordPrompt.id}`
        : null,
  );
  let previousWorkspacePasswordPromptIdentity: string | null = null;
  let jobConflictPrompt = $derived(pendingConflict());
  let activeConflictPromptIdentity = $derived.by(() => {
    const prompt = jobConflictPrompt ?? jobRows.find((task) =>
      task.ownedByRequester && task.question?.kind === "conflict")?.question?.prompt;
    return prompt ? `${prompt.id}:${prompt.version}` : null;
  });
  let previousConflictPromptIdentity: string | null = null;
  let taskDialogTaskId = $state<number | null>(windowRecovery.taskId ?? null);
  let taskDialogDismissedId = $state<number | null>(null);
  let macosSfxPublisherTask = $state<TaskDialogModel | null>(null);
  let LoadedMacosSfxPublisher = $state<MacosSfxPublisherComponentType | null>(null);
  let macosSfxPublisherLoad: Promise<MacosSfxPublisherComponentType> | null = null;
  let taskCenterOpen = $state(windowRecovery.taskCenter ?? false);
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
  let displayedTaskWindowState = $derived(
    initialTaskWindowLaunchState.mode && !initialTaskWindowLaunch && taskWindowLaunchState.status === "waiting"
      ? taskWindowRecoveryState(jobSnapshotStatus()) : taskWindowLaunchState,
  );
  let taskWindowShellTitleCopy = $derived(taskWindowShellTitle(displayedTaskWindowState, tr));
  let taskWindowShellCopy = $derived(taskWindowShellMessage(displayedTaskWindowState, tr));
  let jobSubmitInFlight = $state(false);
  let submittingJobSpec = $state<JobSpec | null>(null);
  let jobPasswordValue = $state("");
  let standalonePasswordInput = $state<HTMLInputElement | null>(null);
  let standalonePasswordFocusedInput: HTMLInputElement | null = null;
  let jobPasswordSubmissionAttempted = $state(false);
  let jobPasswordSubmissionError = $derived(
    jobPasswordSubmissionAttempted && !taskPasswordReady(jobPasswordValue)
      ? tr("gui.password.empty_error")
      : null,
  );
  let workspacePasswordValue = $state("");
  let workspacePasswordSubmissionAttempted = $state(false);
  let workspacePasswordSubmissionError = $derived(
    workspacePasswordSubmissionAttempted && !taskPasswordReady(workspacePasswordValue)
      ? tr("gui.password.empty_error")
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
  let checksumResultFocusPending: { kind: ChecksumResultKind; origin: Element | null } | null = null;
  let duplicateReportPanel = $state<HTMLElement | null>(null);
  let duplicateReportFocusPending = false;
  let browseScrollTop = $state(0);
  let browseViewportHeight = $state(0);
  let archiveKeyboardRequest = 0;
  const refreshedUpdateJobs = new Set<number>();
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
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

  const paletteParam = params.get("palette");
  const hasPaletteOverride = isPaletteId(paletteParam);
  const themeParam = params.get("theme");
  const initialThemeChoice: ThemeChoice | null = isThemeChoice(themeParam) ? themeParam : null;
  const densityParam = params.get("density");
  const hasDensityOverride = isDensityChoice(densityParam);
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
  const settingsSession = new SettingsSession({
    platform: platformKind,
    tr,
    emit: applySettingsEffect,
  }, {
    paletteOverride: hasPaletteOverride ? paletteParam : null,
    defaultExtractDir: defaultExtractDirParam,
  });
  let resolveInitialGeneralSettingsReady!: () => void;
  const initialGeneralSettingsReady = new Promise<void>((resolve) => {
    resolveInitialGeneralSettingsReady = resolve;
  });
  let incomingOpenFilesGeneration = 0;
  const initialPlatform = buildTargetPlatform();
  let activePlatform = $state<PlatformKind>(initialPlatform);
  let prefersDarkTheme = $state(
    typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  let activeTheme = $derived<ResolvedTheme>(
    activeThemeChoice === "system" ? (prefersDarkTheme ? "dark" : "light") : activeThemeChoice,
  );
  let availableLanguages = $state<LanguageDto[]>([]);
  const extraction: ExtractWorkspaceSession = new ExtractWorkspaceSession({
    read: () => ({
      archive: currentArchive,
      screen,
      selection: selectedPaths(),
      selectionBusy: archiveSelectionBusyReason(),
      defaultDirectory: normalizedDefaultExtractDir(settingsSession.appliedGeneral.defaultExtractDir),
      fallbackPaths: runtimePreviews.batchPaths.length > 0
        ? runtimePreviews.batchPaths : currentArchive ? [currentArchive.source] : [],
      platform: platformKind(),
      presetId: presets.selection("extract").id,
      archiveTitle: archiveTitle(),
    }),
    choose: async (purpose, options, isCurrent) => {
      const { open } = await getDialogModule();
      return isCurrent() ? openNativeDialog(purpose, open, options) : null;
    },
    submit: submitJob,
    submitArchive: submitCurrentArchiveJob,
    blocked: () => Boolean(focusBlockingTaskIfAny()),
    isSubmitBlocked: isJobSubmitBlocked,
    notice: showNotice,
    focusDestination: (index) => document.getElementById(`batch-destination-${index}`)?.focus(),
    archiveFolder: defaultExtractDest,
    requestId: nextPreflightRequestId,
    preview: (input) => previewExtractPlan(input.dest, input.selection, input.smart),
    clearPreset: () => presets.clearSelection("extract"),
  });
  const extractView = new ExtractPresentation(extraction, defaultExtractDest);
  let pendingArchiveTaskReview: ArchiveTaskReview | null = null;
  let extractReviewFocusPending = false;
  let convertReviewFocusPending = false;
  let securitySettingsFocusPending = false;
  let taskReviewRequestGeneration = 0;
  let pendingTaskReviewId = $state<number | null>(null);
  let archiveAddPreparation = $state.raw<{
    screen: Screen;
    mode: Mode;
    generation: number;
    id: number;
    source: string;
    encoding: string | null;
    phase: "preparing" | "submitting";
  } | null>(null);
  let archiveAddPending = $derived(archiveAddPreparation !== null);
  const presets: ArchivePresetSession = new ArchivePresetSession({
    create: {
      options: () => create.options.snapshotPreset(), apply: (preset, announce) => create.applyPreset(preset, announce),
      ready: () => create.capabilityReady, touched: () => create.options.state.touched,
      disabledReason: () => create.preflight.busy() ? tr("gui.presets.busy") : "",
      saveIssue: () => create.saveIssue(),
    },
    extract: {
      options: () => extraction.options.snapshotPreset(archiveEncodingForJob()), apply: applyExtractPreset,
      ready: () => true, touched: () => extraction.options.state.touched, disabledReason: () => "",
    },
    notice: showNotice,
  });
  let createPrimaryFocusPending: object | null = null;
  const create: CreateSession = new CreateSession({
    getDialogModule, openNativeDialog, saveNativeDialog, nextRequestId: nextPreflightRequestId,
    platform: platformKind, archiveStemName,
    read: () => ({ screen, mode, defaultDirectory: normalizedDefaultCreateDir(settingsSession.appliedGeneral.defaultCreateDir) }),
    presets: () => presets, notice: showNotice, submit: submitJob,
    blocked: () => Boolean(focusBlockingTaskIfAny()), tError,
    submissionBlockedMessage: (error) => isJobSubmitBlocked(error) ? jobSubmitBlockedMessage(error) : null,
    emit: applyCreatePreflightEffect,
    clearFocus: () => { createPrimaryFocusPending = null; },
    primaryFocusPending: () => createPrimaryFocusPending !== null,
    focusPrimary: () => void focusCreatePrimaryAction(),
    focusInvalid: () => void tick().then(() => {
      if (screen === "create" && create.options.state.validationAttempted && !blockingModalVisible()) {
        document.querySelector<HTMLInputElement>(".create-option-input[aria-invalid='true']")?.focus();
      }
    }),
    scrollSection: (target) => void tick().then(() => document.getElementById(target)?.scrollIntoView({ block: "start", inline: "nearest" })),
    openRecovery: openRecoveryConfiguration, fileManager: fileManagerLabel, trashName: trashNameLabel,
    featuredFormats: featuredFormatCards,
    forwardEvent: (event) => convertRouteHandle?.applyPreflightEvent(event) ?? false,
  }, createFormatParam, {
    scanned: runtimePreviews.preflightScanned, current: runtimePreviews.preflightCurrent,
    destinationBytes: runtimePreviews.preflightDestinationBytes, destinationCurrent: runtimePreviews.preflightDestinationCurrent,
  });
  let archiveOpenStatus = $state<"idle" | "opening">("idle");
  let archiveOpenGeneration = 0;
  let archivePickerRequest: number | null = null;
  let filenameEncodingRequest: { generation: number } | null = null;
  let archivePasswordAttempt = 0;
  let archiveSelectionProgress = $state<{ loaded: number; total: number } | null>(null);
  const recoverySession = new RecoverySession({
    read: () => ({
      active: screen === "recovery", mode, generation: archiveOpenGeneration,
      archive: currentArchive, tasks: jobRows, selection: selectedJobPaths(),
      platform: platformKind(), level: create.options.compressionLevel,
      profile: create.view.profileLabel(create.options.state.profile),
    }),
    dialogs: getDialogModule, openDialog: openNativeDialog, saveDialog: saveNativeDialog,
    submit: submitJob, blocked: isJobSubmitBlocked, focusBlockingTask: () => Boolean(focusBlockingTaskIfAny()),
    notice: showNotice, extension: archiveExtensionMatch, stem: archiveStemName,
    extractDestination: extractDestForPath, mutationDisabledReason: archiveSourceMutationDisabledReason,
    outputFilterName: archiveOutputFilterName, inspectionId: nextPreflightRequestId,
  }, Boolean(runtimePreviews.archive));
  let openDialogModulePromise: Promise<DialogModule> | null = null;
  let nestedExtractReviewFocusPending = false;
  const archiveUpdateReview = new ArchiveUpdateReview();
  let archiveUpdateReviewFocusPending = false;
  let batchReviewFocusPending = false;
  const tools = new ToolsSession({
    read: () => ({
      archive: currentArchive, tasks: jobRows, screen,
      copyScope: [screen, mode, taskCenterOpen, taskCenterSelectedTaskId, taskDialogTaskId, taskDialogDismissedId].join(":"),
      visibleTaskId: taskWindowMode ? taskDialogTask()?.id ?? null : taskCenterOpen
        ? taskCenterSelectedTaskId : taskDialogDismissedId === taskDialogTaskId ? null : taskDialogTaskId,
    }),
    choose: async (purpose, options, isCurrent) => {
      const { open } = await getDialogModule();
      if (!isCurrent()) return null;
      return openNativeDialog(purpose, open, options);
    },
    submitJob,
    submissionBlocked: () => Boolean(focusBlockingTaskIfAny()),
    isSubmitBlocked: isJobSubmitBlocked,
    notice: showNotice,
  }, {
    checksumPath: runtimePreviews.checksumPath, manifestPath: runtimePreviews.checksumManifestPath,
    duplicatePath: runtimePreviews.duplicateScanPath, minimumSize: runtimePreviews.duplicateMinSize,
  });
  $effect(() => tools.syncContext());
  let dragActive = $state(false);
  let lastDropKind = $state<"none" | "archives" | "create" | "recovery">("none");
  const convertRouteOwner: ConvertRouteOwner = {};
  let convertRouteHandle = $state<ConvertRouteHandle | null>(null);
  let convertRouteStatus = $derived<ConvertRouteStatus>(convertRouteHandle?.status() ?? {
    sourceFormat: currentArchive?.format.toUpperCase() ?? "-",
    targetLabel: "-",
    profileLabel: "-",
    methodLabel: "-",
    destination: currentArchive?.path ?? openArchiveFirstLabel(),
  });
  let nestedPreview = $derived(previewSession.nested);
  let entryPreview = $derived(previewSession.file);
  let entryPreviewFailure = $derived(previewSession.failure);
  let previewOriginEntryPath = $derived(previewSession.origin?.entryPath ?? null);
  let previewOriginVirtualIndex = $derived(previewSession.origin?.virtualIndex ?? null);
  let previewPhase = $derived(previewSession.phase);
  let previewTargetName = $derived(previewSession.targetName);
  const archiveEdit = new ArchiveEditSession(
    () => ({
      archive: currentArchive, generation: archiveOpenGeneration, opening: archiveOpenStatus !== "idle",
      directory: archiveDirs.join("/"), selectedPaths: selectedPaths(),
      mutationDisabledReason: archiveMutationDisabledReason(),
    }),
    submitCurrentArchiveJob,
    blockSelectionScopedAction,
    { tr, tError, emit: applyArchiveEditEffect },
  );
  let archiveEditReturnFocus: { owner: ArchiveEditOwner; target: HTMLElement | null } | null = null;
  let moveConflictReturnFocus: { owner: ArchiveMoveReview; target: HTMLElement | null } | null = null;
  let historyRows = $derived(operationHistory());
  let activePopover = $state<"quickActions" | null>(null);
  let archiveSearchInput = $state<HTMLInputElement | null>(null);
  let retryingArchiveId = $state<number | null>(null);
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
    if (settingsSession.colors.palette !== "custom") return {};
    return customPaletteVariablesFor(activeTheme);
  }

  function customPaletteVariablesFor(theme: ResolvedTheme): CssVariableMap {
    return deriveCustomPaletteTokens(settingsSession.colors.accent, theme, settingsSession.colors.contrastGuard);
  }

  function entryContextCssVariables(context: EntryContext): CssVariableMap {
    return {
      ...customPaletteVariables(),
      "--entry-context-left": `${context.x}px`,
      "--entry-context-top": `${context.y}px`,
    };
  }
  $effect(() => {
    document.documentElement.dataset.theme = activeTheme;
    document.documentElement.dataset.palette = settingsSession.colors.palette;
    document.documentElement.dataset.density = activeDensityChoice;
  });

  $effect(() => {
    recoverySession.syncContext();
  });

  $effect(() => {
    const request = archiveAddPreparation;
    if (request?.phase === "preparing" && !isCurrentArchiveAddPreparation(request)) dismissArchiveAddPreparation();
  });

  $effect(() => {
    screen; mode; create.sources;
    untrack(create.syncContext);
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
    const path = currentArchive?.path;
    if (screen !== "passwordBook" && screen !== "password") return;
    if (path) untrack(() => { void refreshArchivePasswordBookStatus(path).catch(() => undefined); });
  });

  $effect(() => {
    currentArchive?.id;
    archiveDirs.join("\u0000");
    selectedPaths();
    untrack(() => archiveEdit.browseContextChanged());
  });

  $effect(() => {
    if (archivePasswordPrompt || previewPasswordPrompt) {
      untrack(() => setScreen("password"));
    }
  });

  $effect(() => {
    const identity = activeJobPasswordPromptIdentity;
    if (identity === previousJobPasswordPromptIdentity) return;
    previousJobPasswordPromptIdentity = identity;
    jobPasswordValue = "";
    jobPasswordSubmissionAttempted = false;
  });

  $effect(() => {
    const identity = activeWorkspacePasswordPromptIdentity;
    if (identity === previousWorkspacePasswordPromptIdentity) return;
    previousWorkspacePasswordPromptIdentity = identity;
    workspacePasswordValue = "";
    workspacePasswordSubmissionAttempted = false;
  });

  $effect(() => {
    const promptIdentity = activeWorkspacePasswordPromptIdentity;
    const input = standalonePasswordInput;
    const ready =
      promptIdentity !== null &&
      jobPasswordPrompt === null &&
      screen === "password" &&
      archiveOpenStatus === "idle" &&
      !previewPasswordPrompt?.busy &&
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
        activeWorkspacePasswordPromptIdentity === promptIdentity &&
        jobPasswordPrompt === null &&
        screen === "password" &&
        archiveOpenStatus === "idle" &&
        !previewPasswordPrompt?.busy &&
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
    if (taskWindowMode) {
      if (taskDialogTaskId === null && taskWindowLaunchState.status !== "waiting") return;
      const task = taskWindowTask(jobRows, taskDialogTaskId);
      if (!task) return;
      taskDialogTaskId = task.id;
      taskDialogDismissedId = null;
      rememberWindowRecovery({ taskWindow: true, taskId: task.id });
      return;
    }
    const questionTaskId = jobPasswordPrompt?.id ?? jobConflictPrompt?.id ?? null;
    if (questionTaskId !== null) {
      rememberTaskWorkspaceFocus();
      taskDialogTaskId = questionTaskId;
      taskDialogDismissedId = null;
      return;
    }
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
    const id = untrack(() => installCompletedTaskPreview(completedTask, params.has("previewChecksumHistory") || params.has("previewDuplicateHistory")));
    if (id === null) return;
    taskDialogTaskId = id;
    taskDialogDismissedId = null;
  });

  $effect(() => {
    const activeTaskPreview = runtimePreviews.activeTask;
    if (!activeTaskPreview) return;
    const id = untrack(() => installActiveTaskPreview(activeTaskPreview, import.meta.env.DEV && params.has("previewStatusUnavailable")));
    if (id === null) return;
    const surface = params.get("previewTaskSurface");
    if (surface === "panel" || surface === "center") {
      taskDialogTaskId = null;
      taskDialogDismissedId = id;
      taskCenterOpen = true;
      taskCenterSelectedTaskId = surface === "panel" ? id : null;
      taskCenterFocusTaskId = id;
      return;
    }
    taskDialogTaskId = id;
    taskDialogDismissedId = null;
  });

  $effect(() => {
    const waitReason = runtimePreviews.taskQueue;
    if (!waitReason) return;
    if (untrack(() => installTaskQueuePreview(waitReason)) === null) return;
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

  $effect(() => extraction.sync());

  let nativeMenuConnection = $state<NativeMenuConnection | null>(null);
  let textEditingFocused = $state(false);
  let taskCenterFocused = $state(false);
  let reconnectNativeMenu: (() => Promise<void>) | null = null;
  function appActionContext() {
    return {
      blocked: modeSelectionBlocked || blockingModalVisible(),
      taskWindow: taskWindowMode,
      opening: archiveOpenStatus === "opening",
      addingFiles: archiveAddPending,
      archive: Boolean(currentArchive),
      writable: !archiveMutationDisabledReason(),
      browsing: screen === "browse",
      selectionBusy: Boolean(archiveSelectionBusyReason()),
      hasSelection: hasArchiveSelection(),
      canRename: canRenameSelection(),
      canPreview: canPreviewEntrySelection(),
      canSelectAll: !archiveSelectionControl().disabled,
      hasParent: canGoUpArchive(),
      textSelection: textEditingFocused || taskCenterFocused,
      taskCenterAvailable: !modeSelectionBlocked && !blockingModalVisible(),
    };
  }
  const availableAppActions = $derived(appActionAvailability(appActionContext()));

  type PreviewActionTarget = { path: string; type: EntryDto["entry_type"] };

  function contextualAppActions(target: PreviewActionTarget) {
    return appActionAvailability({
      ...appActionContext(),
      canPreview: !previewBusy() && previewPolicyFor(target.path, target.type).kind !== "none",
    });
  }

  function appActionEnabled(action: AppAction): boolean {
    return availableAppActions[action];
  }

  async function runAppAction(action: string, previewTarget?: PreviewActionTarget): Promise<boolean> {
    try {
      return await dispatchAppAction(action, previewTarget ? contextualAppActions(previewTarget) : availableAppActions, {
        open_archive: openArchiveFromDialog,
        create_archive: () => setScreen("create"),
        extract_all: () => openExtractWorkspace("all"),
        extract_selection: () => openExtractWorkspace("selection"),
        add_files: submitAddToArchiveJob,
        new_folder: () => openArchiveEditor("new-folder"),
        rename_entry: () => openArchiveEditor("rename"),
        move_entries: () => openArchiveEditor("move"),
        delete_entries: submitDeleteSelectedJob,
        preview_entry: () => submitPreviewEntry(previewTarget?.path, previewTarget?.type),
        copy_entries: () => extraction.copyOut(),
        test_archive: submitTestJob,
        convert_archive: () => setScreen("convert"),
        archive_info: () => setScreen("archiveInfo"),
        search_archive: () => {
          closeQuickActions(false);
          archiveSearchInput?.focus();
          archiveSearchInput?.select();
        },
        go_up: goArchiveUp,
        task_center: () => openTaskCenter(document.activeElement instanceof HTMLElement ? document.activeElement : null),
        select_all: () => {
          if (!isTextEditingTarget(document.activeElement) && !taskWindowMode && !modeSelectionBlocked
            && !blockingModalVisible() && !document.activeElement?.closest("#squallz-task-center")
            && screen === "browse" && currentArchive) {
            closeQuickActions(false);
            return selectAllArchiveEntries();
          }
          const target = document.activeElement;
          if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) target.select();
          else document.execCommand("selectAll");
        },
        settings: () => setScreen("settingsGeneral"),
      });
    } catch (error) {
      pushToast({
        key: "app-action-failed",
        kind: "danger",
        title: t("gui.native_menu.action_failed"),
        body: isErrorDto(error) ? tError(error) : undefined,
      });
      return false;
    }
  }

  function nativeMenuSnapshot() {
    return {
      language: currentLang(),
      enabled: Object.entries(availableAppActions).filter(([, enabled]) => enabled).map(([id]) => id as AppAction),
    };
  }

  function reportNativeMenuFailure(): void {
    pushToast({
      key: "native-menu-unavailable",
      kind: "warning",
      title: t("gui.native_menu.unavailable"),
      action: {
        label: t("gui.error.retry"),
        run: async () => {
          if (nativeMenuConnection) nativeMenuConnection.publish(nativeMenuSnapshot(), true);
          else await reconnectNativeMenu?.();
        },
      },
    });
  }

  onMount(() => {
    let disposed = false;
    let connecting = false;
    let focusUpdateQueued = false;
    const syncTextFocus = () => {
      if (focusUpdateQueued) return;
      focusUpdateQueued = true;
      // Removing a focused control can fire focusout during a Svelte DOM update.
      queueMicrotask(() => {
        focusUpdateQueued = false;
        if (!disposed) {
          textEditingFocused = isTextEditingTarget(document.activeElement);
          taskCenterFocused = Boolean(document.activeElement?.closest("#squallz-task-center"));
        }
      });
    };
    const connect = async () => {
      if (disposed || connecting || nativeMenuConnection) return;
      connecting = true;
      try {
        const connection = await connectNativeMenu(
          (action) => { if (!disposed) void runAppAction(action); },
          () => { if (!disposed) reportNativeMenuFailure(); },
          () => { if (!disposed) removeToastByKey("native-menu-unavailable"); },
        );
        if (disposed) connection?.dispose();
        else nativeMenuConnection = connection;
      } catch {
        if (!disposed) reportNativeMenuFailure();
      } finally {
        connecting = false;
      }
    };
    reconnectNativeMenu = connect;
    syncTextFocus();
    document.addEventListener("focusin", syncTextFocus);
    document.addEventListener("focusout", syncTextFocus);
    void connect();
    return () => {
      disposed = true;
      reconnectNativeMenu = null;
      nativeMenuConnection?.dispose();
      nativeMenuConnection = null;
      document.removeEventListener("focusin", syncTextFocus);
      document.removeEventListener("focusout", syncTextFocus);
    };
  });

  $effect(() => {
    if (i18nReady()) nativeMenuConnection?.publish(nativeMenuSnapshot());
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
    if (preview.nestedPreview) {
      const nested = preview.nestedPreview;
      const preparation = previewSession.beginPreparation({
        outerSource: nested.outer_path, outerDisplayPath: preview.info.path,
        entryPath: nested.entry_path, virtualIndex: null,
      }, "nested", pathBaseName(nested.entry_path));
      if (preparation) {
        previewSession.acceptNested(preparation, nested);
        previewSession.finishPreparation(preparation);
      }
    }
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
      const isCurrent = captureIncomingOpenFilesRequest();
      void ipc.takeOpenFiles()
        .then(async (event) => {
          if (!cancelled && (externalOpenAction(event.action) || isCurrent())) await handleOpenFilesPayload(event);
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
      incomingOpenFilesGeneration += 1;
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
    archiveAddPreparation = null;
    recoverySession.dispose();
    tools.dispose();
    presets.dispose();
    extraction.dispose();
    checksumResultFocusPending = null;
    createPrimaryFocusPending = null;
    create.dispose();
    archiveEdit.dispose();
    previewSession.dispose();
    archiveEditReturnFocus = null;
    moveConflictReturnFocus = null;
    convertRouteHandle?.dispose();
  });

  onMount(() => {
    void retryArchiveFormats();
  });

  onMount(() => {
    void presets.load();
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
    void create.loadCapability();
  });

  onMount(() => {
    if (forceFirstRun) {
      resolveInitialGeneralSettingsReady();
      void loadLocale(null).finally(() => {
        sourceCleanupRecoveryReady = true;
      });
      return;
    }

    let cancelled = false;
    const requestedDraftGenerations = settingsSession.captureGenerations();
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
        applyAppearanceSettingsSnapshot(
          settings,
          appearanceSaveGenerations.density !== requestedAppearanceGenerations.density,
        );
        settingsSession.applySnapshot(settings, requestedDraftGenerations);
        resolveInitialGeneralSettingsReady();
        await loadLocale(settings.language).catch(() => undefined);
        if (cancelled) return;
        settingsSession.updateSnapshotLabel();
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
        settingsSession.applyPreviewLanguage(previewLanguage, requestedDraftGenerations);
        resolveInitialGeneralSettingsReady();
        await loadLocale(previewLanguage).catch(() => undefined);
        if (cancelled) return;
        settingsSession.setDefaultsSnapshotLabel();
        settingsStatus = "preview";
        sourceCleanupRecoveryReady = true;
      });

    return () => {
      cancelled = true;
      resolveInitialGeneralSettingsReady();
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
      label: tr("gui.common.close"),
      run: () => undefined,
    };
    pushToast({
      kind: "warning",
      title: tr(
        "gui.toast.compress_done_preserved",
      )
        .replace("{name}", "reports.7z.001")
        .replace("{count}", "3"),
      body: tr(
        "gui.toast.compress_done_preserved_detail",
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
      if (!nativeMenuConnection) {
        const action = appActionForShortcut(event, activePlatform);
        if (action && appActionEnabled(action)) {
          if (action === "select_all" && isTextEditingTarget(event.target)) return;
          event.preventDefault();
          void runAppAction(action);
          return;
        }
      }
      if (modeSelectionBlocked || archiveEditorVisible()) return;
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
    if (!currentArchive || archiveOpenStatus !== "idle" || archivePasswordPrompt
      || archiveRefreshStatus() === "refreshing") return;
    const path = currentArchive.path;
    const generation = archiveOpenGeneration;
    const completed = pendingArchiveUpdateJobs();
    if (completed.length === 0) return;
    for (const task of completed) refreshedUpdateJobs.add(task.id);
    untrack(() => {
      void refreshCurrentArchive(() => browseVirtualWindow(
        mode === "classic" ? CLASSIC_ROW_HEIGHT : MODERN_ROW_HEIGHT,
      )).then((result) => {
        if (result.status === "applied" && result.isCurrent()
          && generation === archiveOpenGeneration && archiveOpenStatus === "idle"
          && currentArchive?.path === path && archiveRefreshStatus() === "idle"
          && pendingArchiveUpdateJobs().length === 0) {
          showNotice(tr("gui.archive.list_refreshed"));
        }
      });
    });
  });

  function pendingArchiveUpdateJobs() {
    return jobRows.filter((task) => task.spec.kind === "update" && task.state === "done"
      && currentArchive?.path === task.spec.path && !refreshedUpdateJobs.has(task.id));
  }

  function syncUrl(nextMode: Mode = mode) {
    const url = new URL(window.location.href);
    url.searchParams.set("mode", nextMode);
    url.searchParams.set("screen", screen);
    url.searchParams.set("palette", settingsSession.colors.palette);
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
            ? settingsSession.persistenceFailureLabel()
            : previewFailureLabel,
        );
      });
  }

  function setMode(next: Mode) {
    if (next !== mode) {
      tools.leave();
      extraction.leave();
      filenameEncodingRequest = null;
      dismissArchiveAddPreparation();
      create.dismiss();
    }
    firstRunDropFeedback = null;
    trackAppearanceSave(
      "mode",
      persistUiMode(next),
      tr("gui.mode.saved_preview_desktop_unavailable"),
      () => {
        savedModeChoice = next;
      },
    );
    create.syncContext();
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

  function onFirstRunKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    trapModalFocus(event, firstRunPanel);
  }

  function preventCreateSubmissionNavigation(next: Screen): boolean {
    return screen === "create" && next !== "create" && !create.leave();
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
    if (next !== screen) {
      filenameEncodingRequest = null;
      dismissArchiveAddPreparation();
      create.dismiss();
      tools.leave();
      extraction.leave();
      dismissArchivePicker();
      recoverySession.leave();
      if (archiveOpenStatus === "opening" && !archivePasswordPrompt && next !== "password") {
        archiveOpenGeneration += 1;
        archiveOpenStatus = "idle";
        cancelPendingArchiveOpen();
      }
    }
    if (screen !== "password" && next !== screen && (next !== "password" || !previewPasswordPrompt)) {
      clearEntryPreviewState();
    }
    if (screen === "password" && next !== "password") {
      dismissArchivePasswordRequest();
      if (previewPasswordPrompt) clearEntryPreviewState();
      workspacePasswordValue = "";
      workspacePasswordSubmissionAttempted = false;
    }
    cancelTaskReview();
    if (next !== "updateReview") {
      archiveUpdateReviewFocusPending = false;
      archiveUpdateReview.cancelSourceChoice();
    }
    if (next !== "nestedExtract") {
      nestedExtractReviewFocusPending = false;
    }
    if (next !== "batch") {
      batchReviewFocusPending = false;
    }
    if (next !== "create") createPrimaryFocusPending = null;
    if (next !== "extract") extractReviewFocusPending = false;
    if (next !== "convert") convertReviewFocusPending = false;
    if (next !== "settingsSecurity") securitySettingsFocusPending = false;
    if (next !== "checksum") checksumResultFocusPending = null;
    if (next !== "duplicates") duplicateReportFocusPending = false;
    if (next !== "password") pendingArchiveTaskReview = null;
    screen = next;
    create.syncContext();
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

  async function focusChecksumResultPanel(kind: ChecksumResultKind = "checksum"): Promise<void> {
    const request = { kind, origin: null as Element | null };
    checksumResultFocusPending = request;
    await tick();
    if (checksumResultFocusPending !== request) return;
    request.origin = document.activeElement;
    await focusPendingChecksumResult(request);
  }

  async function focusPendingChecksumResult(request: NonNullable<typeof checksumResultFocusPending>): Promise<void> {
    await tick();
    if (checksumResultFocusPending !== request || request.origin === null) return;
    if (screen !== "checksum" || modeSelectionBlocked || taskCenterOpen || blockingModalVisible()
      || (document.activeElement !== request.origin && document.activeElement !== document.body)) {
      checksumResultFocusPending = null;
      return;
    }
    const panel = request.kind === "checksum_check"
      ? checksumCheckResultPanel
      : checksumResultPanel;
    if (!panel?.isConnected) return;
    checksumResultFocusPending = null;
    panel.scrollIntoView({ block: "nearest", inline: "nearest" });
    panel.focus({ preventScroll: true });
  }

  async function focusDuplicateReportPanel(): Promise<void> {
    duplicateReportFocusPending = true;
    await tick();
    if (screen !== "duplicates" || !duplicateReportPanel || blockingModalVisible()) return;
    duplicateReportFocusPending = false;
    duplicateReportPanel.scrollIntoView({ block: "start", inline: "nearest" });
    duplicateReportPanel.focus({ preventScroll: true });
  }

  function registerDuplicateReportPanel(node: HTMLElement | null): void {
    duplicateReportPanel = node;
    if (node && duplicateReportFocusPending) void focusDuplicateReportPanel();
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

  function handleWorkflowEscape(): boolean {
    if (modeSelectionBlocked || blockingModalVisible() || document.activeElement?.closest("#squallz-task-center")) return false;
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
    return false;
  }

  function nextPreflightRequestId(): string {
    return globalThis.crypto?.randomUUID?.()
      ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
  }

  function setTheme(next: ThemeChoice) {
    activeThemeChoice = next;
    syncUrl();
    trackAppearanceSave(
      "theme",
      ipc.setTheme(next),
      tr("gui.theme.saved_preview_desktop_unavailable"),
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
      tr("gui.density.saved_preview_desktop_unavailable"),
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

  function navigationAction(next: Screen): AppAction | undefined {
    if (next === "extract") return "extract_all";
    if (next === "create") return "create_archive";
    if (next === "settingsGeneral") return "settings";
    return undefined;
  }

  function navigationDisabled(next: Screen): boolean {
    const action = navigationAction(next);
    return action ? !appActionEnabled(action) : false;
  }

  function navigationDisabledReason(next: Screen): string {
    if (next === "extract" && navigationDisabled(next)) {
      return archiveSelectionBusyReason() || archiveActionTitle(hasArchiveOpen());
    }
    return "";
  }

  function navigateToScreen(next: Screen) {
    const action = navigationAction(next);
    if (action) {
      void runAppAction(action);
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

  function formatStateLabel(format: FormatDto): string {
    if (format.id === "zip") return tr("gui.format.state.default");
    if (format.id === "sqz") return tr("gui.format.state.recovery_container");
    if (format.id === "wim") return tr("gui.format.state.external_writer");
    if (format.id === "rar") return tr("gui.format.state.open_only");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.format.state.7zz_bridge");
    if (format.id.startsWith("tar.")) return tr("gui.format.state.compound");
    if (format.kind === "compressor") return tr("gui.format.state.stream_codec");
    return tr("gui.format.state.built_in");
  }

  function formatCreateLabel(format: FormatDto): string {
    if (!format.can_create) return tr("common.no", "No");
    if (format.id === "wim") return tr("gui.format.create.if_wimlib");
    return tr("common.yes", "Yes");
  }

  function formatEncryptLabel(format: FormatDto): string {
    if (format.can_encrypt_names) return tr("gui.format.encrypt.names_data");
    if (format.can_encrypt_data) return tr("gui.format.encrypt.data");
    return tr("common.no", "No");
  }

  function formatNote(format: FormatDto): string {
    if (format.id === "rar") return tr(
      "gui.format.note.rar_read_only",
    );
    if (format.id === "wim") return tr("gui.format.note.wim_external");
    if (format.id === "sqz") return tr("gui.format.note.sqz_recovery");
    if (longTailBridgeFormatIds.has(format.id)) return tr("gui.format.note.7zz_bridge");
    if (format.id.startsWith("tar.")) return tr("gui.format.note.compound_tar");
    return format.extensions.length > 0
      ? tr("gui.format.note.extensions").replace("{extensions}", `.${format.extensions.slice(0, 3).join(", .")}`)
      : tr("gui.format.note.registry_capability");
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
      return tr("gui.format.volume.zip");
    }
    if (format.id === "rar") {
      return tr("gui.format.volume.rar");
    }
    if (format.id === "wim") {
      return tr("gui.format.volume.wim");
    }
    if (format.id === "sqz") {
      return tr("gui.format.volume.sqz");
    }
    if (format.can_split) {
      return tr("gui.format.volume.generic");
    }
    return tr("gui.format.volume.single");
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

  function applyExtractPreset(preset: Extract<NamedArchivePreset, { kind: "extract" }> | null, announce: boolean): boolean {
    extraction.applyPreset(preset?.options ?? null);
    if (preset) {
      if (announce) showNotice(tr("gui.presets.applied_notice").replace("{name}", archivePresetDisplayName(preset)));
    }
    return true;
  }

  function joinFolderPath(folder: string, name: string): string {
    return joinDesktopPath(folder, name, platformKind());
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

  function historyLastLabel(): string {
    return historyRows.length > 0
      ? tr("gui.history.local_only")
      : tr("gui.history.no_activity");
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
      onReady: () => { if (securitySettingsFocusPending) focusSecuritySettings(); },
      tr,
      settings: settingsSession,
      appearanceSaveState,
      modernModeSelected: selectedMode === "modern" || (selectedMode === null && mode === "modern"),
      classicModeSelected: selectedMode === "classic" || (selectedMode === null && mode === "classic"),
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
      passwordBookStatusState: passwordBookStatus.state,
      passwordSaveState,
      passwordBookArchiveId: currentArchive?.id ?? null,
      passwordBookCanSave: Boolean(currentArchive && !currentArchive.read_only && passwordBookStatus.state === "ready" && passwordBookStatus.available),
      passwordBookSaved: passwordBookStatus.saved === true,
      savePasswordBook: (password) => currentArchive
        ? rememberArchivePassword(currentArchive.path, password, archiveEncoding()) : Promise.resolve(false),
      cancelPasswordBookSave: cancelArchivePasswordSave,
      clearPasswordBookSave: clearArchivePasswordSave,
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

  function archiveBrowserSurface(rowHeight: number): ArchiveBrowserActions & ArchivePreviewActions & { view: ArchiveBrowserView } {
    const recovery = archiveBrowseRecoveryState();
    const rows = browseEntries(rowHeight).map((entry) => entry ? ({
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
    }) : null);
    return {
      view: {
        selection: archiveSelectionControl(),
        selectedSummary: selectedSummary(),
        editingStatus: archiveEditingStatus(),
        conflict: archiveEdit.reviewView,
        structureWarning: archiveStructureWarningText(),
        recovery,
        totalRows: currentArchive ? totalRows() : 0,
        rows,
        startIndex: browseVirtualWindow(rowHeight).start,
        rowsPending: !archiveBrowseError() && (filterPending() || rows.some((entry) => entry === null)),
        paddingTop: browsePaddingTop(rowHeight),
        paddingBottom: browsePaddingBottom(rowHeight),
      },
      tr,
      onOpenRoot: () => void openArchiveBreadcrumb(-1),
      onOpenRecovery: openCurrentArchiveRecoveryConfiguration,
      onRetryBrowse: () => void retryArchiveContents(),
      onRetryFormats: () => void retryArchiveFormats(),
      ...archivePreviewActions(),
      onCancelMoveConflict: () => archiveEdit.cancelReview(),
      onSubmitMoveReadyOnly: () => void archiveEdit.submitReviewedMove("ready-only"),
      onSubmitMoveKeepBoth: () => void archiveEdit.submitReviewedMove("keep-both"),
      onBrowseScroll: onBrowseVirtualScroll,
      onSelectEntry: (entry, event) => selectEntry(entry, event),
      onActivateEntry: (entry) => void activateEntry(entry),
      onEntryKeydown: (event, entry) => onEntryKeydown(event, entry),
      onOpenEntryContext: (event, entry) => openEntryContext(event, entry),
      onToggleEntrySelection: (entry, event) => toggleEntrySelection(entry, event),
      onToggleAllEntries: toggleAllArchiveEntries,
      onPreviewEntry: (entry) => previewDisplayEntry(entry),
    };
  }

  function classicArchiveBrowserSurface(): ClassicArchiveBrowserSurfaceProps {
    const surface = archiveBrowserSurface(CLASSIC_ROW_HEIGHT);
    return {
      ...surface,
      view: {
        ...surface.view,
        archiveTitle: archiveTitle(),
        archiveFormatSummary: currentArchive
          ? `${archiveFormat()} · ${archiveEntryCountLabel(currentArchive.entry_count)}`
          : openArchiveFirstLabel(),
        archiveOpen: Boolean(currentArchive),
        openArchiveFirst: openArchiveFirstLabel(),
        preview: archivePreviewView(),
        emptyName: currentArchive ? noEntriesLabel() : openArchiveFirstLabel(),
        emptyStatus: surface.view.recovery ? "" : currentArchive ? archiveFilterStatus() : noEntriesLabel(),
      },
    };
  }

  function modernArchiveBrowserSurface(): ModernArchiveBrowserSurfaceProps {
    const surface = archiveBrowserSurface(MODERN_ROW_HEIGHT);
    const archive = currentArchive;
    if (!archive) {
      throw new Error("Modern archive browser requires an open archive");
    }
    const mutationDisabledReason = archiveMutationDisabledReason();
    const previewLabel = previewActionLabel();
    const previewDisabledReason = previewSelectedDisabledReason();
    return {
      ...surface,
      view: {
        ...surface.view,
        archive: {
          title: archiveTitle(),
          format: archiveFormat(),
          summary: archiveSummary(),
          dirs: archiveDirs,
          canGoUp: appActionEnabled("go_up"),
        },
        actions: {
          mutationDisabledReason,
          testDisabledReason: testArchiveDisabledReason(),
          renameDisabledReason: renameSelectedDisabledReason(),
          deleteDisabledReason: deleteSelectedDisabledReason(),
          moveDisabledReason: moveSelectedDisabledReason(),
          copyOutDisabledReason: copyOutSelectedDisabledReason(),
          enabled: availableAppActions,
          previewBusy: previewBusy(),
          previewDisabledReason,
          previewLabel,
          previewIcon: previewActionIcon(),
          extractDestinationHint: archiveSelectionBusyReason() || extractView.extractDestinationHint(),
          extractAllLabel: extractAllToLabel(),
          extractSelectedLabel: extractSelectedToLabel(),
          nestedPreview: Boolean(nestedPreview),
        },
        encodingWarning: hasEncodingWarning() ? archiveWarningText() : null,
        filterText: filterText(),
        filterPending: filterPending(),
        filterStatus: surface.view.recovery ? "" : archiveFilterStatus(),
        emptyLabel: noEntriesLabel(),
      },
      onOpenBreadcrumb: (index) => void openArchiveBreadcrumb(index),
      onGoUp: () => void runAppAction("go_up"),
      onExtractAll: () => void runAppAction("extract_all"),
      onExtractSelection: () => void runAppAction("extract_selection"),
      onTestArchive: () => void runAppAction("test_archive"),
      onAddFiles: () => void runAppAction("add_files"),
      onConvert: () => void runAppAction("convert_archive"),
      onOpenInfo: () => void runAppAction("archive_info"),
      onRenameSelection: () => void runAppAction("rename_entry"),
      onDeleteSelection: () => void runAppAction("delete_entries"),
      onMoveSelection: () => void runAppAction("move_entries"),
      onCopyOutSelection: () => void runAppAction("copy_entries"),
      onCreateFolder: () => void runAppAction("new_folder"),
      onRepairEncoding: () => void repairFilenameEncoding("gbk"),
      onSearchInputMount: (input) => {
        archiveSearchInput = input;
      },
      onSearchInput: updateArchiveFilter,
      onSearchKeydown: onArchiveFilterKeydown,
      onClearSearch: clearArchiveFilter,
    };
  }

  function archivePreviewView(): ArchivePreviewView {
    return {
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
      canPreview: appActionEnabled("preview_entry"),
      actionLabel: previewActionLabel(),
      actionIcon: previewActionIcon(),
      disabledReason: previewSelectedDisabledReason(),
    };
  }

  function archivePreviewActions(): ArchivePreviewActions {
    return {
      onOpenNestedPreview: () => void openNestedPreviewArchive(),
      onExtractNestedPreview: () => void extractNestedPreviewArchive(),
      onClearPreview: (restoreEntryFocus) => clearEntryPreviewState(restoreEntryFocus),
      onRetryPreview: retryEntryPreview,
      onExtractPreviewFailure: () => void extractEntryPreviewFailure(),
      onOpenPreview: () => void openEntryPreview().then((opened) => {
        if (opened) clearEntryPreviewState();
      }),
      onRevealPreview: () => void revealEntryPreview(),
      onPreviewSelection: () => void runAppAction("preview_entry"),
    };
  }

  function modernInspectorSurface(): ModernInspectorSurfaceProps {
    let view: ModernInspectorSurfaceProps["view"];
    if (screen === "recovery") {
      view = {
        kind: "recovery",
        tone: recoverySession.resultTone(),
        title: recoverySession.resultTitle(),
        detail: recoverySession.resultDetail(),
        metricsAvailable: recoverySession.metricsAvailable(),
        explanation: recoverySession.resultExplanation(),
      };
    } else {
      view = {
        kind: "archive",
        preview: archivePreviewView(),
        canRename: appActionEnabled("rename_entry"),
        canMove: appActionEnabled("move_entries"),
        canTest: appActionEnabled("test_archive"),
        canCopyOut: appActionEnabled("copy_entries"),
        archive: currentArchive
          ? {
              format: archiveFormat(),
              entries: currentArchive.entry_count,
              encoding: extractView.extractEncodingLabel(),
              volumes: currentArchive.volumes?.length
                ? archiveVolumeCountLabel(currentArchive.volumes.length)
                : tr("gui.archive.single"),
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
      ...archivePreviewActions(),
      onRenameSelection: () => void runAppAction("rename_entry"),
      onMoveSelection: () => void runAppAction("move_entries"),
      onOpenRecovery: openRecoveryConfiguration,
      onTestArchive: () => void runAppAction("test_archive"),
      onCopyOutSelection: () => void runAppAction("copy_entries"),
    };
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
        title: tr("gui.update.automatic_available_title")
          .replace("{version}", `v${result.latestVersion}`),
        body: tr(
          "gui.update.automatic_available_body",
        ),
        action: {
          label: tr("gui.update.review"),
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

  function languageLabel(tag: string | null): string {
    if (!tag) return tr("gui.settings.language.follow_system");
    const language = availableLanguages.find((item) => item.tag === tag);
    return language ? `${language.name} · ${language.tag}` : tag;
  }

  function showSourceCleanupRecovery(notice: SourceCleanupRecoveryNotice) {
    const trash = platformTrashName();
    if (notice.status === "restored") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.restored",
        ),
        body: notice.path
          ? tr("gui.toast.source_recovery.restored_path").replace(
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
        ),
      });
      return;
    }
    if (notice.status === "preserved") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.preserved",
        ),
        body: notice.path
          ? tr("gui.toast.source_recovery.path").replace("{path}", notice.path)
          : undefined,
      });
      return;
    }
    if (notice.status === "changed") {
      pushToast({
        kind: "danger",
        title: tr(
          "gui.toast.source_recovery.changed",
        ),
        body: notice.path
          ? tr(
              "gui.toast.source_recovery.changed_path",
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
        ).replaceAll("{trash}", trash),
        body: notice.path
          ? tr(
              "gui.toast.source_recovery.original_path",
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
        ),
      });
      return;
    }
    pushToast({
      kind: "danger",
      title: tr(
        "gui.toast.source_recovery.needs_attention",
      ).replace("{trash}", trash),
      body: sourceCleanupRecoveryReason(notice.reason),
      action: {
        label: tr(
          "gui.toast.source_recovery.show_record",
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
      );
    }
    if (reason === "journal_permission_denied") {
      return tr(
        "gui.toast.source_recovery.reason_permission",
      );
    }
    if (reason === "journal_unavailable") {
      return tr(
        "gui.toast.source_recovery.reason_unavailable",
      );
    }
    return tr(
      "gui.toast.source_recovery.reason_failed",
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
      ),
      body: path
        ? tr(
            "gui.toast.source_recovery.show_record_failed_path",
          ).replace("{path}", path)
        : tr(
            "gui.toast.source_recovery.show_record_failed_detail",
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
    if (platform === "macos") return tr("gui.platform.macos");
    if (platform === "windows") return tr("gui.platform.windows");
    return tr("gui.platform.linux");
  }

  function fileManagerLabel(): string {
    const platform = platformKind();
    if (platform === "macos") return tr("gui.platform.file_manager.macos");
    if (platform === "windows") return tr("gui.platform.file_manager.windows");
    return tr("gui.platform.file_manager.linux");
  }

  function trashNameLabel(): string {
    return platformTrashName(platformKind());
  }

  function secretStoreLabel(): string {
    const platform = platformKind();
    if (platform === "macos") return tr("gui.platform.secret_store.macos");
    if (platform === "windows") return tr("gui.platform.secret_store.windows");
    return tr("gui.platform.secret_store.linux");
  }

  function openWithLabel(): string {
    return tr("gui.settings.integration.open_with");
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

  function noArchiveLabel(): string {
    return tr("gui.empty.no_archive_short");
  }

  function classicArchiveStartVisible(): boolean {
    return screen === "browse" && !currentArchive;
  }

  function openArchiveFirstLabel(): string {
    return tr("gui.empty.open_archive_first");
  }

  function noEntriesLabel(): string {
    if (filterPending()) return archiveFilterStatus();
    if (archiveBrowseError()) {
      return filterText().trim()
        ? tr("gui.empty.search_failed")
        : tr("gui.empty.browse_failed");
    }
    if (filterText().trim()) {
      return tr("gui.empty.no_search_matches");
    }
    return tr("gui.empty.no_entries");
  }

  function archiveFilterStatus(): string {
    const refreshStatus = archiveRefreshStatusLabel();
    if (refreshStatus) return refreshStatus;
    if (archiveBrowseError()) {
      return filterText().trim()
        ? tr("gui.list.search_failed")
        : tr("gui.list.browse_failed");
    }
    if (filterPending()) {
      return filterText().trim()
        ? tr("gui.list.searching")
        : tr("gui.list.loading_folder");
    }
    const count = totalRows().toLocaleString();
    if (filterText().trim()) {
      return tr("gui.list.search_result_count").replace("{count}", count);
    }
    return tr("gui.list.current_folder_count").replace("{count}", count);
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

  function applySettingsEffect(effect: SettingsEffect): void {
    switch (effect.kind) {
      case "notice":
        showNotice(effect.message);
        break;
      case "generalApplied":
        setRevealAfterExtractPreference(effect.revealAfterExtract);
        break;
      case "previewLanguage":
        storePreviewLanguage(effect.language);
        break;
      case "generalSaved": {
        const settings = effect.settings;
        recordOperation({
          status: "done",
          title: tr("gui.settings.general.saved"),
          detail: tr(
            "gui.settings.general.saved_detail",
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
        break;
      }
      case "automaticUpdates":
        startAutomaticUpdateCheck(effect.enabled);
        break;
      case "palettePreviewChanged":
        syncUrl();
        break;
    }
  }

  function normalizedFolderSetting(value: string): string | null {
    return normalizeDesktopFolder(value, platformKind());
  }

  function normalizedDefaultCreateDir(value: string = settingsSession.general.defaultCreateDir): string | null {
    return normalizedFolderSetting(value);
  }

  function normalizedDefaultExtractDir(value: string = settingsSession.general.defaultExtractDir): string | null {
    return normalizedFolderSetting(value);
  }

  function defaultCreateFolderLabel(value: string = settingsSession.general.defaultCreateDir): string {
    return normalizedDefaultCreateDir(value) ?? tr("gui.settings.folder.ask_when_creating");
  }

  function defaultExtractFolderLabel(value: string = settingsSession.general.defaultExtractDir): string {
    return normalizedDefaultExtractDir(value) ?? tr("gui.settings.folder.next_to_archive");
  }

  async function chooseDefaultExtractFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("settings.default-extract-folder", open, {
        title: tr("gui.settings.folder.choose_title"),
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        settingsSession.setGeneral("defaultExtractDir", selected);
      }
    } catch {
      showNotice(tr("gui.settings.folder.picker_requires_desktop_service"));
    }
  }

  async function chooseDefaultCreateFolder() {
    try {
      const { open } = await getDialogModule();
      const selected = await openNativeDialog("settings.default-create-folder", open, {
        title: tr("gui.settings.folder.choose_create_title"),
        multiple: false,
        directory: true,
      });
      if (typeof selected === "string") {
        settingsSession.setGeneral("defaultCreateDir", selected);
      }
    } catch {
      showNotice(tr("gui.settings.folder.picker_requires_desktop_service"));
    }
  }

  function openRecoveryConfiguration(source: "preserve" | "current" = "preserve") {
    if (preventCreateSubmissionNavigation("recovery")) return;
    recoverySession.open(source);
    setScreen("recovery");
  }

  function openCurrentArchiveRecoveryConfiguration() {
    openRecoveryConfiguration("current");
  }

  function dismissArchivePicker() {
    if (archivePickerRequest === null) return;
    if (archivePickerRequest === archiveOpenGeneration) {
      archiveOpenGeneration += 1;
      archiveOpenStatus = "idle";
    }
    archivePickerRequest = null;
  }

  async function openArchiveFromDialog() {
    if (archiveOpenStatus === "opening") return;
    if (preventCreateSubmissionNavigation("browse")) return;
    dismissArchiveAddPreparation();
    recoverySession.leave();
    dismissArchivePasswordRequest();
    clearEntryPreviewState();
    const requestGeneration = ++archiveOpenGeneration;
    archivePickerRequest = requestGeneration;
    const isCurrent = () => archivePickerRequest === requestGeneration && archiveOpenGeneration === requestGeneration;
    archiveOpenStatus = "opening";
    showNotice(tr("gui.archive.opening_picker"));
    let selected: string | string[] | null = null;
    try {
      const { open } = await getDialogModule();
      if (!isCurrent()) return;
      // An unfiltered macOS dialog keeps arbitrary numbered volumes selectable.
      const filters: OpenDialogOptions["filters"] = platformKind() === "macos"
        ? undefined
        : [
            {
              name: tr("gui.archive.filter_all_files"),
              extensions: ["*"],
            },
            {
              name: tr("gui.archive.filter_archives"),
              extensions: [
                ...registryFormatExtensions(),
                ...legacyRarVolumeExtensions,
                ...nativeSplitZipVolumeExtensions,
                "001",
              ],
            },
            { name: tr("gui.recovery.par2_files"), extensions: ["par2"] },
          ];
      selected = await openNativeDialog("archive.open", open, {
        title: tr("gui.archive.open_dialog_title"),
        multiple: false,
        directory: false,
        filters,
      });
      if (!isCurrent()) return;
    } catch {
      if (isCurrent()) showNotice(tr("gui.archive.open_requires_desktop_dialog"));
      return;
    } finally {
      if (isCurrent()) {
        archivePickerRequest = null;
        archiveOpenStatus = "idle";
      }
    }
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (typeof path === "string") {
      await openArchivePath(path, "dialog");
    } else {
      showNotice(tr("gui.archive.open_cancelled"));
    }
  }

  async function openFirstArchivePath(paths: string[], source: "dialog" | "open-file") {
    const validPaths = paths.filter((item) => typeof item === "string" && item.length > 0);
    const path = validPaths[0];
    if (!path) return;
    if (!extraction.review.replaceSources(validPaths)) return;
    await openArchivePath(path, source);
    if (validPaths.length > 1) {
      showNotice(tr("gui.archive.opened_first_more_batch").replace("{count}", String(validPaths.length - 1)));
    }
  }

  function captureIncomingOpenFilesRequest(): () => boolean {
    const generation = ++incomingOpenFilesGeneration;
    const navigation = taskReviewRequestGeneration;
    const archiveGeneration = archiveOpenGeneration;
    const batchRevision = extraction.review.batch.revision;
    return () => generation === incomingOpenFilesGeneration
      && navigation === taskReviewRequestGeneration
      && archiveGeneration === archiveOpenGeneration
      && batchRevision === extraction.review.batch.revision;
  }

  async function handleOpenFilesPayload(payload: OpenFilesPayload) {
    const action = externalOpenAction(payload.action);
    if (!action && !payload.paths.some((path) => typeof path === "string" && path.length > 0)) return;
    const isCurrent = captureIncomingOpenFilesRequest();
    if (action) {
      await submitExternalTaskWindow(action, payload.paths, payload.output ?? null);
      return;
    }
    if (preventCreateSubmissionNavigation("browse")) return;
    if (openRecoverySetFromPaths(payload.paths, "open-file")) return;
    await initialGeneralSettingsReady;
    if (!isCurrent() || preventCreateSubmissionNavigation("browse")) return;
    await openFirstArchivePath(payload.paths, "open-file");
  }

  async function submitExternalTaskWindow(action: ExternalOpenAction, paths: string[], output: string | null) {
    function applyTaskWindowSubmitTransition(transition: TaskWindowSubmitTransition) {
      taskWindowLaunchState = transition.state;
      if (transition.notice) showNotice(transition.notice);
    }

    applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "starting", tr));
    taskDialogTaskId = null;
    taskDialogDismissedId = null;
    rememberWindowRecovery({ taskWindow: true });
    let resolvedSpec: JobSpec | null;
    try {
      resolvedSpec = await ipc.resolveExternalTaskJob(
        action,
        paths,
        output,
        tools.draft.algorithm,
        tools.excludes("checksum"),
      );
    } catch (error) {
      if (isErrorDto(error)) {
        const status = error.key.startsWith("error.presets") ? "preset-error" : "resolve-error";
        applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, status, tr));
        showNotice(tError(error));
        return;
      }
      applyTaskWindowSubmitTransition(taskWindowSubmitTransition(action, "requires-desktop-service", tr));
      return;
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
    review: ArchiveTaskReview | null = null,
  ): Promise<boolean> {
    if (preventCreateSubmissionNavigation("browse")) return false;
    dismissArchiveAddPreparation();
    extraction.invalidatePicker();
    recoverySession.leave();
    dismissArchivePicker();
    dismissArchivePasswordRequest();
    clearEntryPreviewState();
    pendingArchiveTaskReview = review;
    if (isPar2Path(path)) {
      openRecoverySet(path, null, source);
      return true;
    }
    const requestGeneration = ++archiveOpenGeneration;
    archiveOpenStatus = "opening";
    const ok = await openArchiveStore(path, null, review?.encoding ?? null);
    if (requestGeneration !== archiveOpenGeneration) return true;
    archiveOpenStatus = "idle";
    if (ok) {
      finishOpenedArchive(path, source);
      return true;
    }
    const passwordPrompt = openPasswordPrompt();
    if (passwordPrompt?.path === path) {
      setScreen("password");
      showNotice(tr("gui.archive.password_needed"));
      return true;
    }
    pendingArchiveTaskReview = null;
    if (archiveOpenError(path)?.key === "error.corrupt_archive") {
      recoverySession.leave();
      recoverySession.selectSource(path);
      setScreen("recovery");
      showNotice(
        tr("gui.recovery.open_failed_routed")
          .replace("{name}", pathBaseName(path)),
      );
      return true;
    }
    if (!archiveOpenError(path)) return false;
    showNotice(archiveOpenFailureNotice(path));
    return false;
  }

  function finishOpenedArchive(path: string, source: "dialog" | "open-file" | "password") {
    const review = pendingArchiveTaskReview;
    pendingArchiveTaskReview = null;
    rememberRecent(path);
    recordOperation({
      status: "info",
      title: tr("gui.archive.opened_operation"),
      detail: pathBaseName(path),
    });
    recoverySession.leave();
    recoverySession.adoptArchive();
    clearEntryPreviewState();
    if (review && sameFilePath(review.path, path) && review.restore()) return;
    setScreen("browse");
    showNotice(
      source === "password"
        ? tr("gui.archive.unlocked")
        : source === "open-file"
          ? tr("gui.archive.open_file_loaded")
          : tr("gui.archive.open_loaded"),
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
      return tr("gui.archive.open_unsupported")
        .replace("{name}", pathBaseName(path));
    }
    if (error?.key === "error.io") {
      return tr("gui.archive.open_io")
        .replace("{name}", pathBaseName(path));
    }
    if (error?.key === "error.resource_limit") {
      return tr("gui.archive.open_resource_limit")
        .replace("{name}", pathBaseName(path));
    }
    return tr("gui.archive.open_failed")
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
    dismissArchiveAddPreparation();
    dismissArchivePicker();
    recoverySession.leave();
    clearEntryPreviewState();
    pendingArchiveTaskReview = null;
    archiveOpenGeneration += 1;
    archiveOpenStatus = "idle";
    cancelArchivePasswordPrompt();
    recoverySession.selectSet(sidecar, archivePath, sidecarCount, sidecarSetCount);
    setScreen("recovery");
    recordValidationRenderReady(`recovery-open:${source}`);
  }

  function entryType(row: EntryDto): string {
    if (row.entry_type === "dir") return "folder";
    if (row.encrypted) return "locked";
    if (row.encoding !== "utf-8" || row.display.includes("\uFFFD")) return "warning";
    return "file";
  }

  function entryAttributeLabel(row: EntryDto): string {
    const parts: string[] = [];
    if (row.entry_type === "dir") parts.push(tr("gui.attr.folder"));
    else if (row.entry_type === "symlink") parts.push(tr("gui.attr.symlink"));
    else if (row.entry_type === "hardlink") parts.push(tr("gui.attr.hardlink"));
    else if (row.entry_type === "other") parts.push(tr("gui.attr.other"));
    else parts.push(tr("gui.attr.file"));
    if (row.encrypted) parts.push(tr("gui.attr.encrypted"));
    if (row.encoding !== "utf-8" || row.display.includes("\uFFFD")) parts.push(tr("gui.attr.encoding_review"));
    return parts.join(" · ");
  }

  function toDisplayEntry(row: EntryDto): DisplayEntry {
    const ratio =
      row.compressed && row.size > 0 ? `${Math.round((row.compressed / row.size) * 100)}%` : "-";
    const normalizedPath = row.path.replaceAll("\\", "/").replace(/\/+$/g, "");
    const parentBoundary = normalizedPath.lastIndexOf("/");
    const location = filterText().trim()
      ? parentBoundary < 0
        ? tr("gui.list.archive_root")
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
      encoding: row.encoding.toUpperCase(),
      attr: entryAttributeLabel(row),
      source: row,
    };
  }

  function browseEntries(rowHeight = MODERN_ROW_HEIGHT): (DisplayEntry | null)[] {
    if (!currentArchive) return [];
    const window = browseVirtualWindow(rowHeight);
    const rows: (DisplayEntry | null)[] = [];
    prefetchAround(window.start);
    prefetchAround(Math.max(window.end - 1, 0));
    for (let index = window.start; index < window.end; index += 1) {
      const row = rowAt(index);
      rows.push(row ? { ...toDisplayEntry(row), virtualIndex: index } : null);
    }
    return rows;
  }

  function browseVirtualWindow(rowHeight = MODERN_ROW_HEIGHT) {
    const total = currentArchive ? totalRows() : 0;
    if (!currentArchive) return { start: 0, end: total, top: 0, bottom: 0 };
    const viewport = Math.max(browseViewportHeight || 360, rowHeight * 6);
    const visibleRows = Math.ceil(viewport / rowHeight);
    const rawStart = Math.min(
      Math.floor(browseScrollTop / rowHeight),
      Math.max(0, total - visibleRows),
    );
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
    return currentArchive?.format.toUpperCase() ?? tr("gui.archive.generic");
  }

  function archiveSummary(): string {
    if (!currentArchive) {
      return tr(
        "gui.empty.no_archive_summary",
      );
    }
    const diagnostics = currentArchive.garbled_count
      ? tr("gui.archive.names_review")
          .replace("{count}", currentArchive.garbled_count.toLocaleString())
      : currentArchive.non_utf8_name_count
        ? tr("gui.archive.non_utf8_names")
            .replace("{count}", currentArchive.non_utf8_name_count.toLocaleString())
        : tr("gui.archive.names_clean");
    return tr("gui.archive.summary")
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
    ).replace("{archive}", archiveTitle());
  }

  function returnToCurrentArchive() {
    const title = archiveTitle();
    setScreen("browse");
    showNotice(
      tr("gui.archive.returned_to_current").replace("{archive}", title),
    );
  }

  function archiveWarningText(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    const count = currentArchive.garbled_count || currentArchive.non_utf8_name_count;
    if (count === 0) {
      return tr("gui.archive.encoding_clean");
    }
    return tr("gui.archive.encoding_warning")
      .replace("{count}", count.toLocaleString());
  }

  function archiveStructureWarningText(): string | null {
    if (currentArchive?.structure !== "zip_local_headers_recovered") return null;
    return tr(
      "gui.archive.zip_local_headers_recovered",
    );
  }

  function hasArchiveStructureWarning(): boolean {
    return archiveStructureWarningText() !== null;
  }

  function hasEncodingWarning(): boolean {
    return Boolean(currentArchive && (currentArchive.garbled_count > 0 || currentArchive.non_utf8_name_count > 0));
  }

  function isEntrySelected(entry: DisplayEntry): boolean {
    if (!currentArchive) return false;
    if (entry.source) return selectedPaths().has(entry.source.path);
    if (!import.meta.env.DEV) return false;
    return entry.name === "Launch plan.pdf" || entry.name === "screenshots" || entry.name === "财务报表.xlsx";
  }

  function entrySelectionLabel(entry: DisplayEntry): string {
    const name = entry.name || entry.source?.path || tr("gui.selection.entry");
    const key = isEntrySelected(entry) ? "gui.selection.deselect_entry" : "gui.selection.select_entry";
    const fallback = isEntrySelected(entry) ? "Deselect {name}" : "Select {name}";
    return tr(key, fallback).replace("{name}", name);
  }

  function entryPreviewForPath(entryPath: string): EntryPreviewDto | null {
    return currentArchive
      ? previewSession.fileFor(currentArchive.source, currentArchive.path, entryPath) : null;
  }

  function hasPreparedPreviewForPath(entryPath: string): boolean {
    return Boolean(currentArchive && previewSession.hasPrepared(currentArchive.source, currentArchive.path, entryPath));
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

  function clearEntryPreviewState(restoreEntryFocus = false) {
    const origin = previewSession.clear();
    if (restoreEntryFocus && origin && origin.virtualIndex !== null) {
      const { virtualIndex, entryPath } = origin;
      queueMicrotask(() => void focusArchiveRow(virtualIndex, entryPath));
    }
  }

  function selectOnlyEntry(entry: DisplayEntry) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    selectRow(entry.source, entry.virtualIndex);
    if (!hasPreparedPreviewForPath(entry.source.path)) clearEntryPreviewState();
  }

  function previewDisplayEntry(entry: DisplayEntry): void {
    if (!entry.source) return;
    void submitPreviewEntry(
      entry.source.path,
      entry.source.entry_type,
      entry.virtualIndex ?? null,
    );
  }

  function toggleEntrySelection(entry: DisplayEntry, event?: MouseEvent) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    if (event?.shiftKey) {
      void selectEntry(entry, event);
      return;
    }
    toggleSelect(entry.source, entry.virtualIndex);
    if (!hasPreparedPreviewForPath(entry.source.path)) clearEntryPreviewState();
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
      ? tr("gui.selection.clear")
      : tr("gui.selection.select_all");
    return {
      checked,
      mixed,
      disabled: total === 0 || filterPending() || Boolean(busyLabel),
      label,
      busy: Boolean(busyLabel),
      busyLabel,
    };
  }

  function selectionProgressLabel(loaded: number, total: number): string {
    return tr("gui.selection.selecting")
      .replace("{loaded}", loaded.toLocaleString())
      .replace("{total}", total.toLocaleString());
  }

  function archiveSelectionBusyReason(): string {
    return archiveSelectionProgress
      ? selectionProgressLabel(archiveSelectionProgress.loaded, archiveSelectionProgress.total)
      : archiveRefreshStatusLabel();
  }

  function archiveRefreshStatusLabel(): string {
    const status = archiveRefreshStatus();
    if (status === "refreshing") return tr("gui.archive.refreshing");
    if (status === "error") return tr("gui.archive.refresh_failed");
    return "";
  }

  function archiveBrowseRecoveryState() {
    if (!currentArchive) return null;
    const status = archiveRefreshStatus();
    const retrying = retryingArchiveId === currentArchive.id;
    if (status === "idle" && !archiveBrowseError() && !retrying) return null;
    const message = retrying && status === "idle" && !archiveBrowseError()
      ? tr("gui.list.loading_rows")
      : archiveFilterStatus();
    return { message, busy: status === "refreshing" || retrying };
  }

  async function retryArchiveContents(): Promise<void> {
    const current = currentArchive;
    if (!current || retryingArchiveId === current.id
      || (archiveRefreshStatus() !== "error" && !archiveBrowseError())) return;
    const generation = archiveOpenGeneration;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    retryingArchiveId = current.id;
    try {
      await retryArchiveBrowse();
    } finally {
      if (retryingArchiveId === current.id) retryingArchiveId = null;
    }
    await tick();
    if (generation !== archiveOpenGeneration || currentArchive?.source !== current.source
      || retryingArchiveId !== null || screen !== "browse" || blockingModalVisible() || modeSelectionBlocked) return;
    if (document.activeElement !== document.body && document.activeElement !== trigger) return;
    if (trigger?.isConnected) trigger.focus({ preventScroll: true });
    else archiveSearchInput?.focus({ preventScroll: true });
  }

  function blockSelectionScopedAction(): boolean {
    const reason = archiveSelectionBusyReason();
    if (!reason) return false;
    showNotice(reason);
    return true;
  }

  async function runArchiveSelection(
    operation: (onProgress: (loaded: number, total: number) => void) => Promise<RowSelectionResult>,
    successLabel: () => string,
  ): Promise<boolean> {
    archiveSelectionProgress = { loaded: 0, total: totalRows() };
    const updateProgress = (loaded: number, total: number) => {
      archiveSelectionProgress = { loaded, total };
      showNotice(selectionProgressLabel(loaded, total));
    };
    const result = await operation(updateProgress);
    archiveSelectionProgress = null;
    if (result === "failed") {
      showNotice(tr("gui.selection.failed"));
      return false;
    }
    if (result === "stale") {
      showNotice(tr("gui.selection.stale"));
      return false;
    }
    showNotice(successLabel());
    return true;
  }

  async function selectAllArchiveEntries() {
    const control = archiveSelectionControl();
    if (control.disabled || control.checked) return;
    const selected = await runArchiveSelection(
      selectAllRows,
      () => tr("gui.selection.all_selected"),
    );
    if (!selected) return;
    clearEntryPreviewState();
    recordValidationEvent("frontend.entry.selection_all", {
      selected_count: selectedPaths().size,
      total_count: totalRows(),
    });
  }

  function toggleAllArchiveEntries() {
    const control = archiveSelectionControl();
    if (control.disabled) return;
    if (control.checked) {
      clearSelection();
      clearEntryPreviewState();
      recordValidationEvent("frontend.entry.selection_clear", {
        selected_count: 0,
      });
      showNotice(tr("gui.selection.cleared"));
      return;
    }
    void selectAllArchiveEntries();
  }

  async function selectEntry(entry: DisplayEntry, event?: MouseEvent | KeyboardEvent) {
    if (!entry.source) return;
    if (archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    const additive = Boolean(event?.metaKey || event?.ctrlKey);
    if (event?.shiftKey) {
      const row = entry.source;
      const selected = await runArchiveSelection(
        (onProgress) => selectRangeTo(row, entry.virtualIndex, additive, onProgress),
        () => tr("gui.selection.count_selected")
          .replace("{count}", selectedPaths().size.toLocaleString()),
      );
      if (!selected) return;
    } else if (additive) {
      toggleSelect(entry.source, entry.virtualIndex);
    } else {
      selectRow(entry.source, entry.virtualIndex);
    }
    if (!hasPreparedPreviewForPath(entry.source.path)) clearEntryPreviewState();
    recordValidationEvent("frontend.entry.select", {
      path: entry.source.path,
      selected_count: selectedPaths().size,
      multi: Boolean(event?.metaKey || event?.ctrlKey),
      range: Boolean(event?.shiftKey),
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
    if (blockSelectionScopedAction()) return;
    if (archiveLikePath(entry.source.path) && currentArchive) {
      await nestedArchiveAction(
        "open",
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
    await goUp();
    if (archiveBrowseError() || archiveDirs.join("/") !== targetDirectory) return;
    browseScrollTop = 0;
    recordValidationEvent("frontend.entry.go_up", {
      path: archiveDirs.join("/"),
    });
    showNotice(tr("gui.nav.opened_parent_folder"));
  }

  async function openArchiveBreadcrumb(level: number) {
    if (!currentArchive) return;
    const targetDirectory = archiveDirs.slice(0, level + 1).join("/");
    clearEntryPreviewState();
    await gotoBreadcrumb(level);
    if (archiveBrowseError() || archiveDirs.join("/") !== targetDirectory) return;
    browseScrollTop = 0;
    recordValidationEvent("frontend.entry.breadcrumb", {
      level,
      path: archiveDirs.slice(0, level + 1).join("/"),
    });
  }

  function showEntryContextAt(x: number, y: number, entry: DisplayEntry) {
    const busyReason = archiveSelectionBusyReason();
    if (busyReason) {
      closeEntryContext();
      showNotice(busyReason);
      return;
    }
    if (entry.source && !selectedPaths().has(entry.source.path)) {
      selectOnlyEntry(entry);
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
      canRename: Boolean(entry.source),
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
    const actions: Record<typeof action, AppAction> = {
      extract: "extract_selection", delete: "delete_entries", rename: "rename_entry",
      move: "move_entries", preview: "preview_entry", test: "test_archive",
    };
    await runAppAction(actions[action], action === "preview" && contextPath
      ? { path: contextPath, type: contextIsDir ? "dir" : "file" } : undefined);
  }

  function onEntryKeydown(event: KeyboardEvent, entry: DisplayEntry) {
    if (event.target !== event.currentTarget) return;
    if (event.metaKey || event.ctrlKey || event.altKey || event.isComposing) return;
    if (["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) {
      event.preventDefault();
      void moveArchiveRow(event, entry);
      return;
    }
    if (
      archiveSelectionBusyReason()
      && ["Delete", "Backspace", "e", "E", "m", "M", "F2"].includes(event.key)
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
      void runAppAction("go_up");
    } else if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      if (!event.repeat) void runAppAction("delete_entries");
    } else if (event.key === "e" || event.key === "E") {
      event.preventDefault();
      if (entry.source && !selectedPaths().has(entry.source.path)) {
        selectOnlyEntry(entry);
      }
      void runAppAction("extract_selection");
    } else if (event.key === "m" || event.key === "M" || event.key === "F2") {
      event.preventDefault();
      if (entry.source && !selectedPaths().has(entry.source.path)) selectOnlyEntry(entry);
      if (!event.repeat) void tick().then(() => runAppAction(event.key === "F2" ? "rename_entry" : "move_entries"));
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const target = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
      const rect = target?.getBoundingClientRect();
      showEntryContextAt(rect ? rect.left + 24 : window.innerWidth / 2, rect ? rect.bottom - 2 : window.innerHeight / 2, entry);
    }
  }

  async function moveArchiveRow(event: KeyboardEvent, entry: DisplayEntry): Promise<void> {
    if (!currentArchive || entry.virtualIndex === undefined || screen !== "browse") return;
    const busyReason = archiveSelectionBusyReason();
    if (busyReason) {
      if (!event.repeat) showNotice(busyReason);
      return;
    }
    const origin = event.currentTarget as HTMLElement;
    const list = origin.closest<HTMLElement>("[data-virtual-list]");
    if (!list) return;
    const rowHeight = mode === "classic" ? CLASSIC_ROW_HEIGHT : MODERN_ROW_HEIGHT;
    const index = archiveKeyboardTarget(event.key, entry.virtualIndex, totalRows(), list.clientHeight / rowHeight);
    if (index === null) return;
    const request = ++archiveKeyboardRequest;
    const archiveId = currentArchive.id;
    const listMode = mode;
    const navigation = taskReviewRequestGeneration;
    const previousSelection = selectedPaths();
    const contextIsCurrent = () => request === archiveKeyboardRequest && screen === "browse"
      && currentArchive?.id === archiveId && mode === listMode
      && taskReviewRequestGeneration === navigation
      && !blockingModalVisible() && !taskCenterOpen;
    const isCurrent = () => contextIsCurrent() && document.activeElement === origin;
    const row = await loadRowAt(index);
    if (!row || !isCurrent() || selectedPaths() !== previousSelection) return;
    // A keyboard range starts at the focused row even when Tab reached it without selecting it.
    if (event.shiftKey && previousSelection.size === 0 && entry.source) selectRow(entry.source, entry.virtualIndex);
    await selectEntry({ ...toDisplayEntry(row), virtualIndex: index }, event);
    if (!isCurrent() || !selectedPaths().has(row.path)) return;
    await focusArchiveRow(index, row.path, () => contextIsCurrent()
      && (document.activeElement === origin || (!origin.isConnected && document.activeElement === document.body)));
  }

  function selectedSummary(): string {
    const count = selectedPaths().size;
    if (!currentArchive || count === 0) {
      return tr("gui.selection.none");
    }
    return tr("gui.selection.selected_size")
      .replace("{count}", count.toLocaleString())
      .replace("{size}", formatBytes(selectedSize()));
  }

  function archiveEntryCountLabel(count: number): string {
    return tr("gui.archive.entry_count")
      .replace("{count}", count.toLocaleString());
  }

  function archiveVolumeCountLabel(count: number): string {
    return tr("gui.archive.volume_count")
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

  function removeBatchItem(index: number): void {
    extraction.review.editBatch({ kind: "remove", index });
    void tick().then(() => {
      if (screen !== "batch") return;
      (document.getElementById(`batch-remove-${Math.min(index, extraction.review.batch.state.items.length - 1)}`)
        ?? document.getElementById("batch-workspace-heading"))?.focus();
    });
  }

  function focusBatchReview(): void {
    batchReviewFocusPending = true;
    void tick().then(() => {
      const heading = document.getElementById("batch-workspace-heading");
      if (!batchReviewFocusPending || screen !== "batch" || blockingModalVisible() || !heading) return;
      batchReviewFocusPending = false;
      heading.focus();
    });
  }

  function dropStatusLabel(): string {
    if (dragActive) return tr("gui.drop.active");
    if (lastDropKind === "archives") {
      return tr("gui.drop.archives_ready").replace("{count}", String(extraction.review.batch.state.items.length));
    }
    if (lastDropKind === "create") {
      return tr("gui.drop.create_ready").replace("{count}", String(create.sources.length));
    }
    if (lastDropKind === "recovery") {
      return tr("gui.drop.recovery_ready");
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
        language: currentLang(),
        mode_selection_blocked: modeSelectionBlocked,
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
        firstRunDropFeedback = tr("gui.first_run.choose_before_drop");
      } else {
        showNotice(tr("gui.first_run.wait_before_drop"));
      }
      return;
    }
    if (create.preflight.state.picker) {
      showNotice(tr("gui.create.sources.finish_picker_before_drop"));
      return;
    }
    if (create.preflight.busy()) {
      showNotice(tr("gui.create.finish_preflight_before_drop"));
      return;
    }
    if (create.preflight.state.pending) {
      showNotice(tr("gui.create.finish_review_before_drop"));
      return;
    }
    if (screen === "create") {
      const result = create.appendSources(dropped, "unknown");
      lastDropKind = "create";
      recordValidationEvent("frontend.drop", {
        source,
        route: "create",
        paths: dropped,
        archive_count: dropped.filter(archiveLikePath).length,
        create_count: dropped.length,
      });
      if (source !== "preview" && source !== "validation") {
        create.view.sourcesAdded(result);
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
      if (!extraction.review.replaceSources(archivePaths)) return;
      if (archivePaths.length === 1) {
        await openArchivePath(archivePaths[0], "open-file");
      } else {
        setScreen("batch");
      }
      return;
    }

    lastDropKind = "create";
    const result = create.appendSources(dropped, "unknown");
    recordValidationEvent("frontend.drop", {
      source,
      route: "create",
      paths: dropped,
      archive_count: archivePaths.length,
      create_count: createInputs.length,
    });
    setScreen("create");
    if (source === "native" && create.options.state.postSuccess === "trash_source") {
      showNotice(
        tr(
          "gui.create.output.trash_drop_confirmation",
        ).replace("{trash}", trashNameLabel()),
      );
    } else if (source !== "preview" && source !== "validation") {
      create.view.sourcesAdded(result);
    }
  }

  function extractDestInDefaultFolder(fallbackParent: string, archiveName: string): string {
    const parent = normalizedDefaultExtractDir(settingsSession.appliedGeneral.defaultExtractDir) ?? fallbackParent;
    const name = archiveStemName(archiveName);
    if (parent === "/") return `/${name}`;
    return `${parent}/${name}`;
  }

  function defaultExtractDest(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return extractDestInDefaultFolder(pathDir(currentArchive.path), currentArchive.name);
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
    const allEntries = preparedNestedPreviewRows(params, currentArchive.source) ?? preview.previewRows ?? preview.rows;
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

  function extractDestForPath(path: string): string {
    return extractDestInDefaultFolder(pathDir(path), pathBaseName(path));
  }

  function sameFilePath(left: string, right: string): boolean {
    return sameDesktopPath(left, right, platformKind());
  }

  function labelWithDisabledReason(label: string, reason: string): string {
    return reason ? `${label} · ${reason}` : label;
  }

  function archiveEncodingForJob(): string | null {
    return currentArchive?.encoding_override ?? null;
  }

  function openExtractWorkspace(scope: ExtractScope) {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_extract"));
      return;
    }
    if (scope === "selection" && archiveSelectionBusyReason()) {
      showNotice(archiveSelectionBusyReason());
      return;
    }
    const selection = selectedJobPaths();
    if (scope === "selection" && !selection) {
      showNotice(tr("gui.precondition.select_before_extract"));
      return;
    }
    if (!extraction.open(scope, selection)) return;
    setScreen("extract");
  }

  function extractWorkspaceBridge(): ExtractWorkspaceBridge {
    return { archiveTitle: archiveTitle(), archiveLine: archiveLine(), password: extractPasswordLabel(),
      warning: archiveWarningText(), testEnabled: appActionEnabled("test_archive"),
      onReady: () => { if (extractReviewFocusPending) focusExtractReview(); },
      onBatch: () => setScreen("batch"), onTest: () => void runAppAction("test_archive") };
  }

  function extractionReviewSurface(mode: "batch" | "nested", variant: ToolsWorkspaceVariant): BatchWorkspaceSurface {
    return { kind: "batch", mode, variant, session: extraction.review, tr,
      title: mode === "batch" ? tr("gui.screen.batch") : tr("gui.nested_extract.title"),
      archiveReturn: toolsArchiveReturnSurface(variant), format: archiveFormatFromPath,
      onReady: () => { if (mode === "batch" ? batchReviewFocusPending : nestedExtractReviewFocusPending) {
        if (mode === "batch") focusBatchReview(); else focusNestedExtractReview();
      } },
      onRemove: removeBatchItem, onBrowse: () => setScreen("browse") };
  }

  function focusExtractReview(): void {
    extractReviewFocusPending = true;
    void tick().then(() => {
      const heading = document.getElementById("extract-workspace-heading");
      if (!extractReviewFocusPending || screen !== "extract" || blockingModalVisible() || !heading) return;
      extractReviewFocusPending = false;
      heading.focus();
    });
  }

  function selectedJobPaths(): string[] | null {
    if (archiveSelectionBusyReason()) return null;
    const selected = [...selectedPaths()];
    return selected.length > 0 ? selected : null;
  }

  function hasArchiveOpen(): boolean {
    return currentArchive !== null;
  }

  function archiveSourceMutationDisabledReason(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    const refreshing = archiveRefreshStatusLabel();
    if (refreshing) return refreshing;
    return currentArchive.read_only
      ? tr("gui.archive.nested_read_only")
      : "";
  }

  function archiveEditingStatus(): ArchiveEditingState | null {
    if (!currentArchive || archiveRefreshStatusLabel()) return null;
    if (currentArchive.read_only) {
      return { message: archiveSourceMutationDisabledReason(), pending: false, retryLabel: null };
    }
    const status = formatRegistryStatus();
    if (status === "loading") {
      return {
        message: tr("gui.archive.editing_loading"),
        pending: true,
        retryLabel: null,
      };
    }
    if (status === "error") {
      return {
        message: tr("gui.archive.editing_check_failed"),
        pending: false,
        retryLabel: tr("gui.archive.editing_retry"),
      };
    }
    const format = allFormats().find((item) => item.id === currentArchive?.format);
    if (format?.can_update === true) return null;
    return {
      message: format
        ? tr("gui.archive.editing_unsupported")
        : tr("gui.archive.editing_unavailable"),
      pending: false,
      retryLabel: null,
    };
  }

  async function retryArchiveFormats(): Promise<void> {
    const origin = document.activeElement;
    const focusScope = origin instanceof HTMLElement ? origin.closest(".archive-editing-status")?.parentElement : null;
    const archiveId = currentArchive?.id;
    const generation = archiveOpenGeneration;
    const requestScreen = screen;
    const requestMode = mode;
    try {
      await loadFormats();
    } catch {
      // The registry state keeps editing disabled and exposes the retry action.
    }
    if (!focusScope) return;
    await tick();
    if (!focusScope.isConnected || currentArchive?.id !== archiveId || archiveOpenGeneration !== generation || screen !== requestScreen
      || mode !== requestMode || blockingModalVisible() || modeSelectionBlocked
      || (document.activeElement !== document.body && document.activeElement !== origin)) return;
    const retryButton = document.querySelector<HTMLButtonElement>(".archive-editing-status button");
    (retryButton ?? archiveSearchInput)?.focus({ preventScroll: true });
  }

  function archiveMutationDisabledReason(): string {
    return archiveSourceMutationDisabledReason() || archiveEditingStatus()?.message || "";
  }

  function hasArchiveSelection(): boolean {
    return hasArchiveOpen() && !archiveSelectionBusyReason() && selectedPaths().size > 0;
  }

  function canRenameSelection(): boolean {
    return hasArchiveSelection() && !archiveMutationDisabledReason() && selectedRenameSource() !== null;
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
        label: tr("gui.preview.no_file"),
        code: "no_archive",
        disabledReason: tr("gui.preview.open_archive_first"),
      };
    }
    if (!entryPath) {
      return {
        kind: "none",
        label: tr("gui.preview.no_file"),
        code: "select_one",
        disabledReason: tr("gui.preview.select_one"),
      };
    }

    const resolvedType = entryType ?? entryTypeForPath(entryPath);
    if (resolvedType === "dir" || entryPath.endsWith("/")) {
      return {
        kind: "folder",
        label: tr("gui.preview.open_folder"),
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
      label: tr("gui.action.open_preview"),
      code: previewSystemCode(entryPath),
      disabledReason: "",
    };
  }

  function selectedPreviewPolicy(): PreviewPolicy {
    return previewPolicyFor(selectedPreviewPath());
  }

  function canPreviewEntrySelection(): boolean {
    return !archiveSelectionBusyReason() && selectedPreviewPolicy().kind !== "none" && !previewBusy();
  }

  function renameSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
    return canRenameSelection()
      ? ""
      : tr("gui.precondition.select_one_before_rename");
  }

  function deleteSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_delete");
  }

  function moveSelectedDisabledReason(): string {
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) return readOnly;
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_move");
  }

  function copyOutSelectedDisabledReason(): string {
    if (!currentArchive) return tr("gui.precondition.open_before_copy_out");
    return hasArchiveSelection()
      ? ""
      : tr("gui.precondition.select_entries_before_copy_out");
  }

  function previewSelectedDisabledReason(): string {
    const selectionBusy = archiveSelectionBusyReason();
    if (selectionBusy) return selectionBusy;
    if (previewBusy()) return tr("gui.preview.loading");
    return selectedPreviewPolicy().disabledReason;
  }

  function previewActionLabel(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
  ): string {
    if (previewBusy()) return tr("gui.preview.loading");
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
      ? tr("gui.preview.loading")
      : previewPolicyFor(entry.source.path, entry.source.entry_type).label;
  }

  function archiveActionTitle(enabled: boolean): string {
    return enabled ? "" : openArchiveFirstLabel();
  }

  function testArchiveDisabledReason(): string {
    if (appActionEnabled("test_archive")) return "";
    if (archiveOpenStatus === "opening") return toolbarLabel("Opening");
    return currentArchive ? "" : tr("gui.precondition.open_before_test");
  }

  function createPrimaryAction(): HTMLElement | null {
    return document.getElementById("create-primary-source-action");
  }

  async function focusCreatePrimaryAction(isCurrent: () => boolean = () => true): Promise<void> {
    const request = {};
    createPrimaryFocusPending = request;
    await tick();
    const action = createPrimaryAction();
    if (!isCurrent() || createPrimaryFocusPending !== request || screen !== "create" || blockingModalVisible() || !action) return;
    createPrimaryFocusPending = null;
    action.focus();
  }

  function selectedDeletePaths(): string[] {
    return archiveSelectionRoots(selectedPaths());
  }

  function selectedRenameSource(): string | null {
    const selected = [...selectedPaths()];
    return selected.length === 1 ? selected[0] : null;
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
    if (previewPhase === "nested") return tr("gui.preview.loading");
    if (!nestedPreview) return tr("gui.preview.no_nested");
    return `${pathBaseName(nestedPreview.entry_path)} · ${nestedPreview.archive.format.toUpperCase()}`;
  }

  function nestedPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase === "nested") {
      return tr("gui.preview.loading_target")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry"));
    }
    if (!nestedPreview) return tr("gui.preview.select_file");
    return tr("gui.preview.nested_entries")
      .replace("{count}", nestedPreview.archive.entry_count.toLocaleString())
      .replace("{suffix}", nestedPreview.truncated ? tr("gui.preview.first_200_shown_suffix") : "");
  }

  function nestedPreviewRows(): EntryDto[] {
    return nestedPreview?.items.slice(0, 5) ?? [];
  }

  function entryPreviewTitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") return tr("gui.preview.loading");
    if (entryPreviewFailure) return tr("gui.preview.failed_title");
    if (!entryPreview) return tr("gui.preview.no_file");
    return entryPreview.display_name;
  }

  function entryPreviewSubtitle(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (previewPhase !== "idle") {
      return tr("gui.preview.loading_target")
        .replace("{name}", previewTargetName || tr("gui.preview.selected_entry"));
    }
    if (entryPreviewFailure) {
      return entryPreviewFailure.message;
    }
    if (!entryPreview) {
      return tr("gui.preview.select_file");
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
      void nestedArchiveAction(failure.retryAction, failure.outerSource, failure.entryPath, previewOriginVirtualIndex);
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
      const prepared = prepareNestedExtract(
        failure.outerSource,
        failure.outerDisplayPath,
        failure.entryPath,
      );
      if (prepared) clearEntryPreviewState();
      return;
    }
    const selectionBusyReason = archiveSelectionBusyReason();
    if (selectionBusyReason) {
      showNotice(selectionBusyReason);
      return;
    }
    const row = previewEntryForPath(failure.entryPath);
    if (!row) {
      showNotice(tr("gui.preview.extract_unavailable"));
      return;
    }
    selectRow(row);
    clearEntryPreviewState();
    openExtractWorkspace("selection");
  }

  async function runPreviewWithPassword<T>(
    preparation: PreviewPreparation,
    outerDisplayPath: string,
    entryPath: string,
    operation: (passwords: NestedArchivePasswords, requestId: string) => Promise<T>,
    options = { serial: false },
  ): Promise<T | null> {
    try {
      return await previewSession.runPreparation(preparation, {
        outerName: pathBaseName(outerDisplayPath),
        innerName: pathBaseName(entryPath),
      }, operation, options);
    } finally {
      if (
        previewSession.isCurrent(preparation) && screen === "password" &&
        !jobPasswordPrompt && !jobConflictPrompt && !archivePasswordPrompt
      ) {
        setScreen("browse");
      }
    }
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
          ? tr("gui.preview.nested_resource_limit")
          : tr("gui.preview.resource_limit");
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

  function extractAllToLabel(): string {
    return tr("gui.action.extract_all_to");
  }

  function extractSelectedToLabel(): string {
    return tr("gui.action.extract_selected_to");
  }

  function extractPasswordLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (passwordBookStatus.state === "ready" && passwordBookStatus.saved) {
      return tr("gui.password.book_can_unlock");
    }
    return tr("gui.password.ask_only_if_required");
  }

  function extractPasswordStatusbarLabel(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    if (passwordBookStatus.state === "ready" && passwordBookStatus.saved) {
      return tr("gui.extract.password_book_ready_short");
    }
    return tr("gui.extract.password_if_needed_short");
  }

  function archiveLine(): string {
    if (!currentArchive) return openArchiveFirstLabel();
    return tr("gui.archive.line")
      .replace("{name}", currentArchive.name)
      .replace("{count}", currentArchive.entry_count.toLocaleString())
      .replace("{format}", archiveFormat());
  }

  function archiveInfoRows(): Array<[string, string]> {
    if (!currentArchive) {
      return [[tr("gui.inspector.archive"), openArchiveFirstLabel()]];
    }
    return [
      [tr("common.name"), currentArchive.name],
      [tr("common.path"), currentArchive.path],
      [tr("gui.archive.format"), archiveFormat()],
      [tr("gui.table.entries"), currentArchive.entry_count.toLocaleString()],
      [tr("gui.archive.encoding"), extractView.extractEncodingLabel()],
      [
        tr("gui.archive.volumes"),
        currentArchive.volumes?.length
          ? archiveVolumeCountLabel(currentArchive.volumes.length)
          : tr("gui.archive.single"),
      ],
      [tr("common.selection"), selectedSummary()],
      [extractView.extractDestinationFieldLabel(), extraction.destination],
    ];
  }

  async function submitCurrentArchiveJob(
    spec: JobSpec,
    success: string,
    missing: string,
    onFailure?: (message: string) => void,
  ): Promise<boolean> {
    if (!currentArchive) {
      if (onFailure) onFailure(missing);
      else showNotice(missing);
      return false;
    }
    const blocked = focusBlockingTaskIfAny();
    if (blocked) {
      onFailure?.(jobSubmitBlockedMessage(new JobSubmitBlockedError(blocked)));
      return false;
    }
    try {
      await submitJob(spec);
      showNotice(success);
      return true;
    } catch (error) {
      if (isJobSubmitBlocked(error)) {
        onFailure?.(jobSubmitBlockedMessage(error));
        return false;
      }
      const title = tr("gui.job.submit_failed");
      const body = isErrorDto(error)
        ? tError(error)
        : tr("gui.job.submit_unavailable");
      if (onFailure) onFailure(`${title} · ${body}`);
      else pushToast({ kind: "danger", title, body });
      return false;
    }
  }

  function toolsArchiveReturnSurface(variant: ToolsWorkspaceVariant) {
    return {
      visible: variant === "classic" && showArchiveReturnBar(),
      title: archiveTitle(),
      detail: archiveReturnDetail(),
      contextLabel: tr("gui.archive.current_context"),
      actionLabel: tr("gui.archive.back_to_current"),
      onReturn: returnToCurrentArchive,
    };
  }

  function localToolsSurface(kind: "checksum", variant: ToolsWorkspaceVariant): ChecksumWorkspaceSurface;
  function localToolsSurface(kind: "duplicates", variant: ToolsWorkspaceVariant): DuplicatesWorkspaceSurface;
  function localToolsSurface(kind: "checksum" | "duplicates", variant: ToolsWorkspaceVariant): ChecksumWorkspaceSurface | DuplicatesWorkspaceSurface {
    return {
      kind, variant, session: tools, tr,
      title: kind === "checksum" ? tr("gui.screen.checksum") : tr("gui.screen.duplicates"),
      archiveReturn: toolsArchiveReturnSurface(variant),
      onNavigate: (next: "duplicates" | "recovery" | "create" | "batch") => setScreen(next),
      onPanelMount: (kind: ChecksumResultKind | "duplicates", node: HTMLElement | null) => {
        if (kind === "duplicates") { registerDuplicateReportPanel(node); return; }
        if (kind === "checksum") checksumResultPanel = node;
        else checksumCheckResultPanel = node;
        const request = checksumResultFocusPending;
        if (node && request?.kind === kind) void focusPendingChecksumResult(request);
      },
    };
  }

  async function submitTestJob() {
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_test"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "test",
        path: currentArchive.source,
        encoding: archiveEncodingForJob(),
        password: null,
      },
      tr("gui.test.queued"),
      tr("gui.precondition.open_before_test"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.test.queued"),
        detail: archiveTitle(),
      });
    }
  }

  function isCurrentArchiveAddPreparation(request: NonNullable<typeof archiveAddPreparation>): boolean {
    return archiveAddPreparation === request && screen === request.screen && mode === request.mode
      && archiveOpenGeneration === request.generation && archiveOpenStatus !== "opening"
      && currentArchive?.id === request.id && currentArchive.source === request.source
      && currentArchive.encoding_override === request.encoding;
  }

  function dismissArchiveAddPreparation(): void {
    if (archiveAddPreparation?.phase === "preparing") archiveAddPreparation = null;
  }

  async function submitAddToArchiveJob() {
    if (archiveAddPending) return;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_add"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
      return;
    }
    if (focusBlockingTaskIfAny()) return;
    const { id, source: path, encoding_override: encoding } = currentArchive;
    const request: NonNullable<typeof archiveAddPreparation> = {
      screen, mode, generation: archiveOpenGeneration, id, source: path, encoding, phase: "preparing",
    };
    const content_policy = create.options.state.contentPolicy;
    const excludes = content_policy === "custom" ? [...create.options.excludeRules] : [];
    const level = create.options.compressionLevel;
    const profile = create.view.profileLabel(create.options.state.profile);
    const canAddToOriginalArchive = () => {
      if (!isCurrentArchiveAddPreparation(request)) return false;
      const reason = archiveMutationDisabledReason();
      if (reason) showNotice(reason);
      return !reason;
    };
    archiveAddPreparation = request;
    try {
      let selected: string | string[] | null;
      try {
        const { open } = await getDialogModule();
        if (!canAddToOriginalArchive()) return;
        selected = await openNativeDialog("archive.add-files", open, {
          title: tr("gui.add.choose_files_to_add"),
          multiple: true,
          directory: false,
        });
      } catch {
        if (!isCurrentArchiveAddPreparation(request)) return;
        showNotice(tr("gui.add.picker_failed"));
        return;
      }
      if (!canAddToOriginalArchive()) return;
      const add = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (add.length === 0) {
        showNotice(tr("gui.add.cancelled"));
        return;
      }
      request.phase = "submitting";
      const queued = await submitCurrentArchiveJob({
        kind: "update",
        path,
        expected_archive_id: id,
        encoding,
        add,
        delete: [],
        rename: [],
        mkdir: [],
        excludes,
        content_policy,
        password: null,
        level,
      },
      tr("gui.add.operations_queued").replace("{count}", add.length.toLocaleString()),
      tr("gui.precondition.open_before_add"));
      if (!queued) return;
      recordOperation({
        status: "queued",
        title: tr("gui.add.queued"),
        detail: tr("gui.add.items_profile")
          .replace("{count}", add.length.toLocaleString())
          .replace("{profile}", profile),
      });
    } finally {
      if (archiveAddPreparation === request) archiveAddPreparation = null;
    }
  }

  async function submitDeleteSelectedJob() {
    if (blockSelectionScopedAction()) return;
    if (!currentArchive) {
      showNotice(tr("gui.precondition.open_before_delete"));
      return;
    }
    const readOnly = archiveMutationDisabledReason();
    if (readOnly) {
      showNotice(readOnly);
      return;
    }
    const paths = selectedDeletePaths();
    if (paths.length === 0) {
      showNotice(tr("gui.precondition.select_entries_before_delete"));
      return;
    }
    const queued = await submitCurrentArchiveJob(
      {
        kind: "update",
        path: currentArchive.source,
        expected_archive_id: currentArchive.id,
        encoding: currentArchive.encoding_override,
        add: [],
        delete: paths,
        rename: [],
        mkdir: [],
        excludes: [],
        content_policy: "keep_all_files",
        password: null,
        level: 6,
      },
      (paths.length === 1
        ? tr("gui.delete.operation_queued")
        : tr("gui.delete.operations_queued").replace("{count}", paths.length.toLocaleString())),
      tr("gui.precondition.open_before_delete"),
    );
    if (queued) {
      recordOperation({
        status: "queued",
        title: tr("gui.delete.queued"),
        detail: tr("gui.delete.entries_from_archive")
          .replace("{count}", paths.length.toLocaleString())
          .replace("{archive}", archiveTitle()),
      });
    }
  }

  function openArchiveEditor(kind: ArchiveEditKind) {
    const reason = kind === "rename" ? renameSelectedDisabledReason()
      : kind === "move" ? moveSelectedDisabledReason() : archiveMutationDisabledReason();
    if (!currentArchive || reason) {
      showNotice(reason || openArchiveFirstLabel());
      return;
    }
    const owner = archiveEdit.open(kind);
    if (!owner) return;
    archiveEditReturnFocus = {
      owner, target: document.activeElement instanceof HTMLElement ? document.activeElement : null,
    };
    setScreen("browse");
  }

  function applyArchiveEditEffect(effect: ArchiveEditEffect) {
    if (effect.kind === "notice") {
      showNotice(effect.message);
    } else if (effect.kind === "queued") {
      recordOperation({ status: "queued", title: effect.title, detail: effect.detail });
    } else if (effect.kind === "reviewCreated") {
      const target = archiveEditReturnFocus?.owner === effect.editor ? archiveEditReturnFocus.target : null;
      moveConflictReturnFocus = { owner: effect.review, target };
    } else if (effect.kind === "editorClosed") {
      if (archiveEditReturnFocus?.owner !== effect.owner) return;
      const target = archiveEditReturnFocus.target;
      archiveEditReturnFocus = null;
      restoreArchiveEditFocus(target);
    } else if (moveConflictReturnFocus?.owner === effect.review) {
      const target = moveConflictReturnFocus.target;
      moveConflictReturnFocus = null;
      if (effect.restoreFocus) restoreArchiveEditFocus(target);
    }
  }

  function restoreArchiveEditFocus(target: HTMLElement | null) {
    void tick().then(() => {
      if (blockingModalVisible() || (document.activeElement !== document.body && document.activeElement !== target)) return;
      const focus = target?.isConnected && !target.matches(":disabled") && !target.closest("[inert]")
        ? target : archiveSearchInput;
      if (focus?.isConnected && !focus.closest("[inert]")) focus.focus({ preventScroll: true });
    });
  }

  function archiveEditorFields(kind: ArchiveEditKind) {
    const blocked = archiveEdit.blockedReason;
    const feedback = {
      cancelWhileSubmitting: archiveEdit.checking,
      error: archiveEdit.error || (archiveRefreshStatus() === "idle" ? blocked : null),
      disabled: Boolean(blocked),
      recovery: archiveEdit.source && currentArchive?.source === archiveEdit.source
        && archiveRefreshStatus() !== "idle" ? archiveBrowseRecoveryState() : null,
      retryLabel: tr("gui.error.retry"),
      onRetry: retryArchiveContents,
      submittingLabel: archiveEdit.checking
        ? kind === "move" ? tr("gui.move.checking_targets")
          : tr("gui.edit.checking_targets")
        : tr("gui.task_center.submitting"),
      value: archiveEdit.target,
      status: archiveEdit.status,
      onChange: (value: string) => archiveEdit.setTarget(value),
      onSubmit: () => archiveEdit.submit(),
    };
    if (kind === "rename") return {
      ...feedback,
      title: tr("gui.action.rename_selected"),
      label: tr("gui.rename.target_name"),
    };
    if (kind === "move") return {
      ...feedback,
      title: tr("gui.action.move_selected"),
      label: tr("gui.move.target_folder"),
      hint: tr("gui.move.path_hint"),
    };
    return {
      ...feedback,
      title: tr("gui.action.new_folder"),
      label: tr("gui.new_folder.name"),
      placeholder: tr("gui.new_folder.default_name"),
      hint: tr("gui.new_folder.path_hint"),
    };
  }

  async function openArchiveDirectoryEntry(entryPath: string) {
    const name = pathBaseName(entryPath.replace(/\/+$/g, ""));
    if (!name) {
      showNotice(tr("gui.preview.open_folder_failed"));
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
    showNotice(tr("gui.preview.folder_opened").replace("{name}", name));
  }

  async function submitPreviewEntry(
    entryPath: string | null = selectedPreviewPath(),
    entryType: EntryDto["entry_type"] | null = null,
    virtualIndex: number | null = null,
  ) {
    if (!currentArchive) {
      showNotice(tr("gui.preview.open_archive_first"));
      return;
    }
    if (!entryPath) {
      showNotice(tr("gui.preview.select_one"));
      return;
    }
    if (entryType === "dir" || entryPath.endsWith("/") || entryTypeForPath(entryPath) === "dir") {
      await openArchiveDirectoryEntry(entryPath);
      return;
    }
    if (blockSelectionScopedAction()) return;
    if (archiveLikePath(entryPath)) {
      await nestedArchiveAction("preview", currentArchive.source, entryPath, virtualIndex);
      return;
    }
    clearNotice();
    const archivePath = currentArchive.path;
    const archiveSource = currentArchive.source;
    const origin = { outerSource: archiveSource, outerDisplayPath: archivePath, entryPath, virtualIndex };
    const preparedEntry = entryPreviewForPath(entryPath);
    if (preparedEntry) {
      const preparation = previewSession.beginPreparedFileOpen(preparedEntry, origin);
      if (!preparation) return;
      const opened = await openEntryPreview(preparedEntry);
      previewSession.finishPreparation(preparation);
      if (opened) clearEntryPreviewState();
      return;
    }
    const preparation = previewSession.beginPreparation(origin, "entry", previewEntryDisplayName(entryPath));
    if (!preparation) return;
    recordValidationEvent("frontend.entry.preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      if (!previewSession.isCurrent(preparation)) return;
      const preparedPreview = await runPreviewWithPassword(
        preparation,
        archivePath,
        entryPath,
        async (passwords, requestId) => previewSampleForEntry(archivePath, entryPath)
          ?? ipc.previewArchiveEntry(archiveSource, entryPath, passwords.outer, archiveEncodingForJob(), requestId),
        { serial: true },
      );
      if (!preparedPreview) return;
      if (!previewSession.acceptFile(preparation, preparedPreview)) return;
      recordValidationEvent("frontend.entry.preview_loaded", {
        entry_path: entryPath,
        display_name: preparedPreview.display_name,
      });
      if (!await openEntryPreview(preparedPreview)) return;
      if (!previewSession.isCurrent(preparation)) return;
      recordOperation({
        status: "info",
        title: tr("gui.preview.operation_title"),
        detail: pathBaseName(entryPath),
      });
      clearEntryPreviewState();
    } catch (error) {
      if (!previewSession.isCurrent(preparation)) return;
      const failurePolicy = previewPolicyFor(entryPath, entryType ?? entryTypeForPath(entryPath));
      const message = previewFailureMessage(
        error,
        false,
        "gui.preview.failed",
        "Could not open this item",
      );
      previewSession.failPreparation(preparation, {
        entryPath,
        entryType: entryType ?? entryTypeForPath(entryPath),
        displayName: previewEntryDisplayName(entryPath),
        policyKind: failurePolicy.kind,
        outerSource: archiveSource,
        outerDisplayPath: archivePath,
        message,
        retryAction: "preview",
      });
      recordValidationEvent("frontend.entry.preview_failed", {
        entry_path: entryPath,
        policy_kind: failurePolicy.kind,
      });
    } finally {
      previewSession.finishPreparation(preparation);
    }
  }

  async function nestedArchiveAction(
    action: "preview" | "open" | "extract",
    outerPath: string | null,
    entryPath: string | null,
    virtualIndex: number | null = previewOriginVirtualIndex,
  ) {
    if (action === "preview") {
      if (!currentArchive) {
        showNotice(tr("gui.preview.open_archive_first", "Open an archive before previewing entries"));
        return;
      }
      if (!entryPath) {
        showNotice(tr("gui.preview.select_one", "Select one file entry to preview"));
        return;
      }
      if (!archiveLikePath(entryPath)) {
        showNotice(tr("gui.preview.select_archive"));
        return;
      }
      outerPath = currentArchive.source;
    }
    if (outerPath === null || entryPath === null) return;
    if (action === "extract" && (preventCreateSubmissionNavigation("extract")
      || preventConvertSubmissionNavigation("extract") || focusBlockingTaskIfAny())) return;
    clearNotice();
    if (action !== "preview") dismissArchiveAddPreparation();
    const initialScreen = screen;
    const initialScreenGeneration = taskReviewRequestGeneration;
    const prepared = action !== "preview" ? previewSession.takeNested(outerPath, entryPath) : null;
    const archiveDisplayPath = currentArchive?.path ?? outerPath;
    const encoding = action !== "preview" && currentArchive?.source === outerPath ? archiveEncodingForJob() : null;
    const preparation = previewSession.beginPreparation({
      outerSource: outerPath, outerDisplayPath: archiveDisplayPath, entryPath, virtualIndex,
    }, "nested", pathBaseName(entryPath));
    if (!preparation) return;
    const isCurrent = () => previewSession.isCurrent(preparation)
      && (action !== "extract" || (screen === initialScreen
        && (!prepared || initialScreenGeneration === taskReviewRequestGeneration)));
    if (action === "preview") recordValidationEvent("frontend.entry.nested_preview_requested", {
      entry_path: entryPath,
    });
    try {
      await waitForPreviewFeedbackFrame();
      if (!isCurrent()) {
        if (prepared) void ipc.closeArchive(prepared.id).catch(() => undefined);
        return;
      }
      const result = action === "preview" ? await runPreviewWithPassword(
          preparation,
          archiveDisplayPath,
          entryPath,
          async (passwords, requestId) => nestedPasswordPreviewSample(params, outerPath, entryPath, passwords)
            ?? ipc.previewNestedArchive(outerPath, entryPath, passwords, archiveEncodingForJob(), requestId),
        ) : prepared ?? await runPreviewWithPassword(
          preparation,
          currentArchive?.path ?? outerPath,
          entryPath,
          (passwords, requestId) => ipc.openNestedArchive(outerPath, entryPath, passwords, encoding, requestId),
        );
      if (!result) return;
      const info = "archive" in result ? result.archive : result;
      if ("archive" in result) {
        if (!previewSession.acceptNested(preparation, result)) return;
        recordValidationEvent("frontend.entry.nested_preview_loaded", {
          entry_path: entryPath,
          entry_count: info.entry_count,
          format: info.format,
        });
        recordOperation({
          status: "info",
          title: tr("gui.preview.nested_operation_title"),
          detail: `${pathBaseName(entryPath)} · ${info.format.toUpperCase()}`,
        });
      } else {
        if (!isCurrent()) {
          void ipc.closeArchive(info.id).catch(() => undefined);
          return;
        }
        if (!await adoptOpenedArchive(info, isCurrent)) return;
        recoverySession.leave();
        recoverySession.adoptArchive();
        clearEntryPreviewState();
        if (action === "extract") {
          openExtractWorkspace("all");
          focusExtractReview();
        }
        showNotice(tr("gui.preview.opened_nested_archive").replace("{name}", info.name));
        recordOperation({
          status: "done",
          title: tr("gui.preview.nested_opened_operation_title"),
          detail: `${pathBaseName(entryPath)} -> ${info.name}`,
        });
      }
    } catch (error) {
      if (!isCurrent()) return;
      const failurePolicy = previewPolicyFor(entryPath, entryTypeForPath(entryPath));
      const message = previewFailureMessage(
        error,
        true,
        action === "preview" ? "gui.preview.nested_failed" : "gui.preview.open_nested_requires_desktop_service",
        action === "preview" ? "Could not preview this nested archive"
          : "Could not open this inner archive. Preview it or extract it instead.",
      );
      previewSession.failPreparation(preparation, {
        entryPath,
        entryType: entryTypeForPath(entryPath),
        displayName: pathBaseName(entryPath),
        policyKind: failurePolicy.kind,
        outerSource: outerPath,
        outerDisplayPath: action === "preview" ? archiveDisplayPath
          : currentArchive?.source === outerPath ? currentArchive.path : outerPath,
        message,
        retryAction: action,
      });
    } finally {
      previewSession.finishPreparation(preparation);
    }
  }

  async function revealEntryPreview() {
    const preview = entryPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_first"));
      return;
    }
    const action = previewSession.beginFileAction(preview, currentArchive?.source ?? null);
    if (!action) return;
    const responseIsCurrent = () => previewSession.fileActionIsCurrent(action, currentArchive?.source ?? null);
    try {
      await ipc.revealPreviewSession(preview.preview_id);
      if (!responseIsCurrent()) return;
      showNotice(
        tr("gui.preview.revealed_system")
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
      showNotice(tr("gui.preview.preview_first"));
      return false;
    }
    // Preparation already cleared its previous feedback before waiting for the file.
    if (previewPhase === "idle") clearNotice();
    const action = previewSession.beginFileAction(preview, currentArchive?.source ?? null);
    if (!action) return false;
    const outerDisplayPath = currentArchive?.path ?? preview.outer_path;
    const responseIsCurrent = () => previewSession.fileActionIsCurrent(action, currentArchive?.source ?? null);
    if (previewSystemOpenRequiresConfirmation(preview.entry_path)) {
      let confirmed: boolean;
      try {
        const { confirm } = await getDialogModule();
        if (!responseIsCurrent()) return false;
        confirmed = await confirm(
          tr(
            "gui.preview.risky_open_body",
          ).replace("{name}", preview.display_name),
          {
            title: tr("gui.preview.risky_open_title"),
            kind: "warning",
            okLabel: tr("gui.preview.risky_open_action"),
            cancelLabel: tr("gui.preview.risky_open_cancel"),
          },
        );
      } catch {
        if (!responseIsCurrent()) return false;
        showNotice(tr(
          "gui.preview.risky_open_confirm_failed",
        ));
        return false;
      }
      if (!responseIsCurrent()) return false;
      if (!confirmed) {
        showNotice(
          tr(
            "gui.preview.risky_open_cancelled",
          ).replace("{name}", preview.display_name),
        );
        return false;
      }
    }
    try {
      await ipc.openPreviewSession(preview.preview_id);
      if (!responseIsCurrent()) return false;
      previewSession.completeFileAction(action, currentArchive?.source ?? null);
      showNotice(tr("gui.preview.opened_system").replace("{name}", preview.display_name));
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
      previewSession.failFileAction(action, currentArchive?.source ?? null, {
        entryPath: preview.entry_path,
        entryType,
        displayName: preview.display_name,
        policyKind: previewPolicyFor(preview.entry_path, entryType).kind,
        outerSource: preview.outer_path,
        outerDisplayPath,
        message,
        retryAction: "preview",
      });
      return false;
    }
  }

  async function focusArchiveRow(index: number, entryPath: string | null, canFocus: () => boolean = () => true): Promise<void> {
    if (screen !== "browse" || !currentArchive) return;
    const archiveId = currentArchive.id;
    const directoryKey = archiveDirs.join("\u0000");
    const filterKey = filterText();
    const listMode = mode;
    const listKind = listMode === "classic" ? "classic" : "modern";
    const selector = `[data-row-index="${index}"]`;
    const contextIsCurrent = () => (
      screen === "browse" &&
      currentArchive?.id === archiveId &&
      archiveDirs.join("\u0000") === directoryKey &&
      filterText() === filterKey &&
      mode === listMode && canFocus()
    );
    const rowMatches = () => !entryPath || rowAt(index)?.path === entryPath;

    await tick();
    if (!contextIsCurrent()) return;
    const list = document.querySelector<HTMLElement>(`[data-virtual-list="${listKind}"]`);
    if (!list) return;
    const rowHeight = listMode === "classic" ? CLASSIC_ROW_HEIGHT : MODERN_ROW_HEIGHT;
    const top = index * rowHeight;
    const nextScrollTop = Math.max(0, Math.min(top, Math.max(list.scrollTop, top + rowHeight - list.clientHeight)));
    if (nextScrollTop !== list.scrollTop) {
      list.scrollTop = nextScrollTop;
      browseScrollTop = nextScrollTop;
      browseViewportHeight = list.clientHeight;
    }
    if (!await loadRowAt(index)) return;
    await tick();
    if (!contextIsCurrent() || !rowMatches()) return;
    list.querySelector<HTMLElement>(selector)?.focus({ preventScroll: true });
  }

  async function openNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_open"));
      return;
    }
    await nestedArchiveAction("open", preview.outer_path, preview.entry_path);
  }

  function restoreNestedExtractDraft(draft: NestedExtractDraft): boolean {
    if (extraction.review.locked("nested")) {
      showNotice(tr("gui.nested_extract.wait_for_current"));
      return false;
    }
    if (preventCreateSubmissionNavigation("nestedExtract") || preventConvertSubmissionNavigation("nestedExtract") || focusBlockingTaskIfAny()) return false;
    if (!extraction.review.adoptNested(draft)) return false;
    setScreen("nestedExtract");
    focusNestedExtractReview();
    return true;
  }

  function focusNestedExtractReview(): void {
    nestedExtractReviewFocusPending = true;
    void tick().then(() => {
      const heading = document.getElementById("batch-workspace-heading");
      if (!nestedExtractReviewFocusPending || screen !== "nestedExtract" || blockingModalVisible() || !heading) return;
      nestedExtractReviewFocusPending = false;
      heading.focus();
    });
  }

  function prepareNestedExtract(outerSource: string, outerDisplayPath: string, entryPath: string): boolean {
    return restoreNestedExtractDraft(extraction.review.prepareNested(outerSource, outerDisplayPath, entryPath));
  }

  async function extractNestedPreviewArchive() {
    const preview = nestedPreview;
    if (!preview) {
      showNotice(tr("gui.preview.preview_nested_before_extract"));
      return;
    }
    await nestedArchiveAction("extract", preview.outer_path, preview.entry_path, previewOriginVirtualIndex);
  }

  async function repairFilenameEncoding(encoding = "gbk") {
    if (!currentArchive) {
      showNotice(tr("gui.encoding.open_before_repair"));
      return;
    }
    dismissArchiveAddPreparation();
    clearEntryPreviewState();
    const request = { generation: archiveOpenGeneration };
    filenameEncodingRequest = request;
    const result = await reopenWithEncoding(encoding);
    if (filenameEncodingRequest !== request) return;
    filenameEncodingRequest = null;
    if (request.generation !== archiveOpenGeneration || result.status === "superseded" || !result.isCurrent()) return;
    const ok = result.status === "applied";
    if (ok) {
      extraction.options.encodingReopened();
    }
    showNotice(
      ok
        ? tr("gui.encoding.reopened_with").replace("{encoding}", encoding.toUpperCase())
        : tr("gui.encoding.reopen_failed"),
    );
  }

  async function applyCreatePreflightEffect(effect: CreatePreflightEffect, isCurrent: () => boolean): Promise<void> {
    if (!isCurrent()) return;
    if (effect.kind === "prepareSubmit" && !taskWindowMode) {
      taskCenterReturnFocus = document.querySelector<HTMLElement>(".create-plan-review") ?? taskCenterReturnFocus;
    } else if (effect.kind === "focus") {
      if (effect.target === "primary") await focusCreatePrimaryAction(isCurrent);
      else {
        await tick();
        if (isCurrent() && screen === "create") document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
      }
    } else if (effect.kind === "queued" && !taskWindowMode && !taskCenterOpen
      && document.activeElement instanceof HTMLElement && document.activeElement.closest(".create-plan-review")) {
      await focusCreatePrimaryAction(isCurrent);
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
      outputPasswordRequired: outputPasswordRequired(submittingJobSpec),
      title: titleForJobSpec(submittingJobSpec),
      origin: "app",
      ownedByRequester: true,
      interaction: null,
      question: null,
      actionFailure: null,
      statusStale: false,
      answeredQuestionVersion: 0,
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
    if (!task) return tr("gui.state.ready");
    return `${titleForJobSpec(task.spec)} · ${taskOutcomeStateLabel(task)}`;
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
    const unconfirmed = jobRows.find(taskStatusUnavailable);
    if (unconfirmed) return taskOutcomeStateLabel(unconfirmed);
    const counts = taskCenterCounts(jobRows);
    if (counts.attention > 0) {
      return tr("gui.task_center.summary_attention")
        .replace("{count}", counts.attention.toLocaleString());
    }
    const active = counts.active + (submittingJobSpec ? 1 : 0);
    if (active > 0 || counts.waiting > 0) {
      return tr("gui.task_center.summary_active")
        .replace("{active}", active.toLocaleString())
        .replace("{waiting}", counts.waiting.toLocaleString());
    }
    if (counts.completed > 0) {
      return tr("gui.task_center.summary_completed")
        .replace("{count}", counts.completed.toLocaleString());
    }
    return tr("gui.task_center.summary_idle");
  }

  function taskCenterTriggerLabel(): string {
    return `${tr("gui.task_center.title")} · ${taskCenterSummaryLabel()}`;
  }

  function openTaskCenter(source: HTMLElement | null = null): void {
    if (!appActionEnabled("task_center") || taskCenterOpen) return;
    tools.leave();
    checksumResultFocusPending = null;
    rememberTaskWorkspaceFocus(source ?? document.activeElement);
    taskCenterFocusTaskId = null;
    taskCenterSelectedTaskId = null;
    taskCenterOpen = true;
    activePopover = null;
  }

  function closeTaskCenter(): void {
    tools.leave();
    cancelTaskReview();
    taskCenterOpen = false;
    taskCenterSelectedTaskId = null;
    taskCenterFocusTaskId = null;
    restoreTaskWorkspaceFocus();
  }

  function rememberTaskWorkspaceFocus(source: Element | null = document.activeElement): void {
    if (taskWindowMode || taskDialogVisible()) return;
    if (source instanceof HTMLElement && source !== document.body
      && !source.closest("#squallz-task-center, .task-modal-overlay")) {
      taskCenterReturnFocus = source;
    }
  }

  function restoreTaskWorkspaceFocus(): void {
    const returnFocus = taskCenterReturnFocus;
    const returnScreen = screen;
    taskCenterReturnFocus = null;
    void tick().then(() => {
      if (taskWindowMode || taskCenterOpen || blockingModalVisible() || modeSelectionBlocked || screen !== returnScreen) return;
      if (returnFocus?.isConnected && !returnFocus.closest("[inert]") && !returnFocus.matches(":disabled")) {
        returnFocus.focus({ preventScroll: true });
        if (document.activeElement === returnFocus) return;
      }
      const fallback = screen === "create" ? createPrimaryAction() : document.querySelector<HTMLElement>(".task-center-trigger");
      fallback?.focus({ preventScroll: true });
    });
  }

  function openTaskCenterDetails(task: Task): void {
    tools.leave();
    cancelTaskReview();
    taskCenterFocusTaskId = task.id;
    taskCenterSelectedTaskId = task.id;
  }

  function returnToTaskCenter(task: TaskDialogModel): void {
    tools.leave();
    cancelTaskReview();
    taskCenterSelectedTaskId = null;
    taskCenterFocusTaskId = task.id;
  }

  async function clearCompletedTasks(): Promise<void> {
    const ids = clearableTaskIds(jobRows);
    if (ids.length === 0) return;
    if (!await clearFinished(ids)) return;
    showNotice(
      tr("gui.task_center.cleared")
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
    if (taskWindowMode) {
      if (taskDialogTaskId === null && taskWindowLaunchState.status !== "waiting") return null;
      return taskWindowTask(jobRows, taskDialogTaskId);
    }
    if (taskDialogTaskId !== null) {
      const remembered = jobRows.find((task) => task.id === taskDialogTaskId);
      if (remembered) return remembered;
    }
    return null;
  }

  function taskDialogVisible(): boolean {
    const task = taskDialogTask();
    if (!task) return false;
    if (task.id === null) return true;
    return taskDialogDismissedId !== task.id;
  }

  function archiveEditorVisible(): boolean {
    return screen === "browse" && archiveEdit.kind !== null && !archivePasswordPrompt && !previewPasswordPrompt;
  }

  function blockingModalVisible(): boolean {
    return taskDialogVisible() || macosSfxPublisherTask !== null || archiveEditorVisible();
  }

  function taskCenterVisible(): boolean {
    return !taskWindowMode && taskCenterOpen && !blockingModalVisible() && !modeSelectionBlocked;
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
      title: tr("gui.sfx_publish.choose_output"),
      defaultPath: suggested,
      filters: [{
        name: tr("gui.sfx_publish.app_filter"),
        extensions: ["app"],
      }],
    });
  }

  function formatMacosSfxPublishSubmitError(error: unknown): string | null {
    if (isJobSubmitBlocked(error)) return jobSubmitBlockedMessage(error);
    return isErrorDto(error) ? tError(error) : null;
  }

  function taskProgressDialogProps(
    task: TaskDialogModel,
    presentation: "panel" | "dialog" | "window",
  ): TaskProgressDialogSurfaceProps {
    const passwordPrompt = jobPasswordPrompt?.id === task.id ? jobPasswordPrompt : null;
    const conflictPrompt = jobConflictPrompt?.id === task.id ? jobConflictPrompt : null;
    const rootClass = presentation === "panel" ? "task-center-detail"
      : presentation === "window" ? "task-window-surface" : "task-modal-overlay";
    return {
      task,
      presentation,
      rootClass: `${rootClass} design-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`,
      rootVariables: customPaletteVariables(),
      copyFeedback: tools.copyFeedback("task", task.id)?.message ?? null,
      copyFeedbackTone: tools.copyFeedback("task", task.id)?.tone ?? null,
      copyPending: tools.copyPending("task", task.id),
      ...(presentation === "panel" ? { rootId: "squallz-task-center" } : {
        passwordQuestion: taskPasswordQuestion(task),
        passwordValue: jobPasswordValue,
        passwordError: jobPasswordSubmissionError,
        conflictQuestion: taskConflictQuestion(task),
        conflictApplyAll,
      }),
      taskOutputPath,
      taskRevealOutputLabel,
      taskWindowMode: presentation === "window",
      reviewPending: pendingTaskReviewId === task.id,
      macosSfxPublishingAvailable: activePlatform === "macos",
      onPause: pauseCurrentTask,
      onResume: resumeCurrentTask,
      onCancel: cancelCurrentTask,
      onCopyChecksumResults: (task) => tools.copyTask(task),
      onOpenOutput: openTaskOutput,
      onPublishMacosSfx: openMacosSfxPublisher,
      onReviewTask: prepareTaskReview,
      onToggleDetails: toggleTaskDetails,
      onViewResults: viewTaskResults,
      onRevealOutput: revealTaskOutput,
      onDismiss: presentation === "panel" ? returnToTaskCenter : dismissTaskDialog,
      onPasswordValueChange: (value) => {
        if (isCurrentTaskPasswordPrompt(passwordPrompt)) jobPasswordValue = value;
      },
      onSubmitPassword: () => submitTaskPasswordRequest(passwordPrompt),
      onCancelPassword: () => cancelTaskPasswordRequest(passwordPrompt),
      onConflictApplyAllChange: (applyAll) => {
        if (isCurrentTaskConflictPrompt(conflictPrompt)) conflictApplyAll = applyAll;
      },
      onAnswerConflict: (decision, applyAll) => answerConflictDecision(conflictPrompt, decision, applyAll),
    };
  }

  function taskCenterSurface(): TaskCenterSurfaceProps {
    return {
      tasks: jobRows,
      submittingTask: submittingTaskModel(),
      rootClass: `task-center-panel design-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`,
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

  function passwordWorkspaceSurface(variant: PasswordWorkspaceVariant): PasswordWorkspaceSurface {
    return {
      variant,
      tr,
      active: Boolean(archivePasswordPrompt || previewPasswordPrompt),
      name: passwordPromptName(),
      detail: passwordPromptDetail(),
      sessionDetail: passwordSessionDetail(),
      failureDetail: passwordFailureDetail(),
      secretStoreLabel: secretStoreLabel(),
      value: workspacePasswordValue,
      busy: archiveOpenStatus === "opening" || Boolean(previewPasswordPrompt?.busy),
      rejected: Boolean(archivePasswordPrompt?.wrong || previewPasswordPrompt?.wrong),
      error: workspacePasswordSubmissionError,
      onInputMount: (input) => (standalonePasswordInput = input),
      onValueChange: (value) => (workspacePasswordValue = value),
      onSubmit: submitPasswordRequest,
      onCancel: cancelPasswordRequest,
      onBack: () => setScreen("browse"),
    };
  }

  function taskPasswordQuestion(task: TaskDialogModel) {
    if (
      task.id === null ||
      !isTaskActiveState(task.state) ||
      jobPasswordPrompt?.id !== task.id
    ) return null;
    return {
      name: jobPasswordPrompt.name,
      detail: jobPasswordPrompt.wrong
        ? tr("gui.password.previous_rejected")
        : tr("gui.password.task_paused"),
      sessionDetail: tr("gui.password.session_only_separate_book"),
    };
  }

  function taskConflictQuestion(task: TaskDialogModel) {
    if (
      task.id === null ||
      !isTaskActiveState(task.state) ||
      jobConflictPrompt?.id !== task.id
    ) return null;
    return {
      path: jobConflictPrompt.incoming_path,
      existing: `${formatBytes(jobConflictPrompt.existing_size)} · ${formatModified(jobConflictPrompt.existing_modified)}`,
      incoming: `${formatBytes(jobConflictPrompt.incoming_size)} · ${formatModified(jobConflictPrompt.incoming_modified)}`,
    };
  }

  function openTaskDialog(task: Task | null = blockingTask()): void {
    if (!task) return;
    rememberTaskWorkspaceFocus();
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
    cancelTaskReview();
    if (task.id === null) return;
    if (isTaskActiveState(task.state)) return;
    tools.leave();
    if (taskWindowMode && await closeNativeTaskWindow()) return;
    taskDialogDismissedId = task.id;
    taskDialogTaskId = null;
    if (!taskWindowMode && taskCenterSelectedTaskId === task.id) {
      closeTaskCenter();
      return;
    }
    if (!taskWindowMode && !taskCenterOpen) restoreTaskWorkspaceFocus();
  }

  function isJobSubmitBlocked(error: unknown): boolean {
    return error instanceof JobSubmitBlockedError;
  }

  function jobSubmitBlockedMessage(error: unknown): string {
    if (!(error instanceof JobSubmitBlockedError)) {
      return tr("gui.task.one_at_a_time_notice");
    }
    if (error.reason === "starting") {
      return tr(
        "gui.task.starting_notice",
      );
    }
    if (error.reason === "replace-existing") {
      return tr(
        "gui.task.replace_existing_queue_blocked",
      );
    }
    return tr("gui.task.one_at_a_time_notice");
  }

  async function submitJob(spec: JobSpec): Promise<number> {
    const blockReason = focusBlockingTaskIfAny(
      spec.kind === "compress" && spec.replace_existing === true,
    );
    if (blockReason) {
      throw new JobSubmitBlockedError(blockReason);
    }
    const previousTaskCenter = taskWindowMode ? null : {
      open: taskCenterOpen,
      selectedTaskId: taskCenterSelectedTaskId,
      focusTaskId: taskCenterFocusTaskId,
      navigationGeneration: taskReviewRequestGeneration,
    };
    jobSubmitInFlight = true;
    submittingJobSpec = spec;
    if (taskWindowMode) {
      taskDialogTaskId = null;
      taskDialogDismissedId = null;
    } else {
      if (taskCenterReturnFocus === null) {
        rememberTaskWorkspaceFocus();
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
        rememberWindowRecovery({ taskWindow: true, taskId: id });
      }
      return id;
    } catch (error) {
      if (previousTaskCenter
        && previousTaskCenter.navigationGeneration === taskReviewRequestGeneration
        && taskCenterOpen && taskCenterSelectedTaskId === null && taskCenterFocusTaskId === null) {
        taskCenterOpen = previousTaskCenter.open;
        taskCenterSelectedTaskId = previousTaskCenter.selectedTaskId;
        taskCenterFocusTaskId = previousTaskCenter.focusTaskId;
        if (!previousTaskCenter.open && !taskDialogVisible()) {
          const focused = document.activeElement;
          if (!focused || focused === document.body || focused.closest("#squallz-task-center")) {
            restoreTaskWorkspaceFocus();
          } else {
            taskCenterReturnFocus = null;
          }
        }
      }
      throw error;
    } finally {
      jobSubmitInFlight = false;
      submittingJobSpec = null;
    }
  }

  async function cancelCurrentTask(task: TaskDialogModel): Promise<void> {
    if (task.id === null) return;
    if (await cancelTask(task.id) && task.controlIntent === "cancel") {
      showNotice(tr("gui.task.cancel_requested"));
    }
  }

  async function pauseCurrentTask(task: TaskDialogModel): Promise<void> {
    if (task.id === null) return;
    if (await pauseTask(task.id) && task.controlIntent === "pause") {
      showNotice(tr("gui.task.pause_requested"));
    }
  }

  async function resumeCurrentTask(task: TaskDialogModel): Promise<void> {
    if (task.id === null) return;
    if (await resumeTask(task.id) && task.controlIntent === "resume") {
      showNotice(tr("gui.task.resume_requested"));
    }
  }

  function moveQueuedTaskEarlier(task: Task): void {
    moveTaskEarlier(task.id);
    showNotice(tr("gui.task_center.moving_earlier"));
  }

  function moveQueuedTaskLater(task: Task): void {
    moveTaskLater(task.id);
    showNotice(tr("gui.task_center.moving_later"));
  }

  function moveQueuedTaskBefore(task: Task, beforeTask: Task | null): void {
    moveTaskBefore(task.id, beforeTask?.id ?? null);
    showNotice(tr("gui.task_center.moving_position"));
  }

  function taskRevealOutputLabel(): string {
    return t("gui.task.show_in_file_manager", { fileManager: fileManagerLabel() });
  }

  function preventTaskWorkspaceNavigation(next: Screen): boolean {
    if (screen === "create" && next !== "create" && !create.preflight.canLeave()
      && preventCreateSubmissionNavigation(next)) return true;
    if (screen === "convert" && next !== "convert"
      && convertRouteHandle && !convertRouteHandle.canLeave()) return true;
    return false;
  }

  function viewTaskResults(task: TaskDialogModel): void {
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
    if (preventTaskWorkspaceNavigation(target)) return;
    if (target === "recovery" && !recoverySession.adoptTask(task)) return;
    tools.adoptTask(task);
    setScreen(target);
    void dismissTaskDialog(task);
    if (target === "checksum") {
      void focusChecksumResultPanel(task.spec.kind === "checksum_check" ? "checksum_check" : "checksum");
    }
    if (target === "duplicates") void focusDuplicateReportPanel();
  }

  function toggleTaskDetails(task: TaskDialogModel): void {
    if (task.id === null || task.state !== "failed") return;
    setTaskExpanded(task.id, !task.expanded);
  }

  function cancelTaskReview(): void {
    taskReviewRequestGeneration += 1;
    pendingTaskReviewId = null;
  }

  async function prepareTaskReview(task: TaskDialogModel): Promise<void> {
    if (taskWindowMode || task.id === null || pendingTaskReviewId === task.id || !taskReviewScreen(task)) return;
    if (taskReviewScreen(task) === "settingsSecurity") {
      await reviewTask(task);
      return;
    }
    const generation = ++taskReviewRequestGeneration;
    pendingTaskReviewId = task.id;
    const initialScreen = screen;
    const initialArchive = currentArchive?.id ?? null;
    const isCurrent = () => generation === taskReviewRequestGeneration
      && screen === initialScreen && (currentArchive?.id ?? null) === initialArchive;
    try {
      const spec = previewTaskSpecForReview(task.id) ?? await ipc.reviewJobSpec(task.id);
      if (!isCurrent()) return;
      if (spec.kind !== task.spec.kind) throw new Error("Task kind changed");
      await reviewTask({ ...task, spec }, task.spec);
    } catch {
      if (!isCurrent()) return;
      showNotice(tr("gui.task.review.source_unavailable"));
    } finally {
      if (generation === taskReviewRequestGeneration) pendingTaskReviewId = null;
    }
  }

  async function reviewTask(task: TaskDialogModel, displayedSpec: JobSpec = task.spec): Promise<void> {
    if (taskWindowMode) return;
    let target = taskReviewScreen(task);
    if (target === "archiveInfo" && recoverySession.usesTaskContext(task)) {
      target = "recovery";
    }
    if (target === "extract" && task.spec.kind === "extract" && task.spec.best_effort) {
      target = "recovery";
    }
    if (!target) return;
    if (target === "updateReview" && task.spec.kind === "update" && displayedSpec.kind === "update") {
      if (preventCreateSubmissionNavigation(target) || preventConvertSubmissionNavigation(target) || focusBlockingTaskIfAny()) return;
      if (!archiveUpdateReview.restore(task.spec, displayedSpec.path)) {
        showNotice(tr("gui.update_review.wait"));
        return;
      }
      setScreen(target);
      await dismissTaskDialog(task);
      focusArchiveUpdateReview();
      return;
    }
    if (target === "nestedExtract" && task.spec.kind === "extract_nested" && displayedSpec.kind === "extract_nested") {
      if (!restoreNestedExtractDraft(reviewNestedExtract(task.spec, displayedSpec.outer_path))) return;
      await dismissTaskDialog(task);
      focusNestedExtractReview();
      showNotice(tr("gui.nested_extract.review_restored"));
      return;
    }
    if (target === "batch" && task.spec.kind === "batch_extract") {
      if (extraction.review.locked("batch")) {
        showNotice(tr("gui.batch.wait_for_current"));
        return;
      }
      if (preventCreateSubmissionNavigation(target) || preventConvertSubmissionNavigation(target) || focusBlockingTaskIfAny()) return;
      if (!extraction.review.adoptBatch(task, displayedSpec)) return;
      setScreen(target);
      await dismissTaskDialog(task);
      focusBatchReview();
      showNotice(task.state === "done"
        ? tr("gui.batch.review_failed_restored")
        : tr("gui.batch.review_restored"));
      return;
    }
    if (target === "extract" && task.spec.kind === "extract") {
      await reviewExtractTask(task);
      return;
    }
    if (target === "convert" && task.spec.kind === "convert") {
      await reviewConvertTask(task);
      return;
    }
    if (preventTaskWorkspaceNavigation(target)) return;
    if (target === "create" && task.spec.kind === "compress" && !create.restoreTask(task.spec, task.outputPasswordRequired)) return;
    if (target === "recovery" && !recoverySession.adoptTask(task, displayedSpec)) return;
    if (target === "settingsSecurity") securitySettingsFocusPending = true;
    setScreen(target);
    await dismissTaskDialog(task);
    if (target === "create") focusCreatePrimaryAction();
    if (target === "settingsSecurity") focusSecuritySettings();
  }

  function focusSecuritySettings(): void {
    securitySettingsFocusPending = true;
    void tick().then(() => {
      const input = document.getElementById("settings-security-max-entries");
      if (!securitySettingsFocusPending || screen !== "settingsSecurity" || blockingModalVisible() || !input) return;
      securitySettingsFocusPending = false;
      input.focus();
    });
  }

  function focusArchiveUpdateReview(): void {
    archiveUpdateReviewFocusPending = true;
    void tick().then(() => {
      const heading = document.getElementById("update-review-heading");
      if (!archiveUpdateReviewFocusPending || screen !== "updateReview" || blockingModalVisible() || !heading) return;
      archiveUpdateReviewFocusPending = false;
      heading.focus();
    });
  }

  async function submitArchiveUpdateReview(): Promise<void> {
    if (focusBlockingTaskIfAny()) return;
    try {
      if (await archiveUpdateReview.submit(submitJob)) {
        showNotice(tr("gui.update_review.queued"));
      }
    } catch (error) {
      if (isJobSubmitBlocked(error)) return;
      pushToast({ kind: "danger", title: tr("gui.job.submit_failed"),
        body: isErrorDto(error) ? tError(error) : tr("gui.job.submit_unavailable") });
    }
  }

  async function chooseArchiveUpdateSource(id: number, kind: "file" | "folder"): Promise<void> {
    if (screen !== "updateReview") return;
    const path = archiveUpdateReview.draft?.operations.find((row) => row.id === id)?.value;
    await archiveUpdateReview.chooseSource(id, async (isCurrent) => {
      const { open } = await getDialogModule();
      if (!isCurrent() || screen !== "updateReview") return null;
      const selected = await openNativeDialog("update-review.source", open, {
        title: kind === "folder" ? tr("gui.update_review.choose_folder") : tr("gui.update_review.choose_file"),
        multiple: false, directory: kind === "folder", defaultPath: path || undefined,
      });
      return Array.isArray(selected) ? selected[0] ?? null : selected;
    });
  }

  function archiveUpdateWorkspaceSurface(variant: ToolsWorkspaceVariant): ArchiveUpdateWorkspaceSurface {
    return { kind: "update", variant, title: tr("gui.update_review.title"), tr,
      archiveReturn: { ...toolsArchiveReturnSurface(variant), visible: Boolean(currentArchive) }, review: archiveUpdateReview,
      policyLabel: createContentPolicyLabel,
      onReady: () => { if (archiveUpdateReviewFocusPending) focusArchiveUpdateReview(); },
      onChooseSource: chooseArchiveUpdateSource,
      onSubmit: submitArchiveUpdateReview, onOpenTasks: () => openTaskCenter() };
  }

  async function reviewExtractTask(task: TaskDialogModel): Promise<void> {
    if (task.spec.kind !== "extract") return;
    const spec = task.spec;
    const draft: ExtractTaskDraft = {
      path: spec.path, dest: spec.dest, selection: spec.selection === null ? null : [...spec.selection],
      overwrite: spec.overwrite, symlinks: spec.symlinks, smart: spec.smart,
      encoding: spec.encoding, verify_sfx: spec.verify_sfx,
    };
    await reviewArchiveTask(task, "extract", {
      path: draft.path, encoding: draft.encoding, restore: () => restoreExtractTaskDraft(draft),
    });
  }

  async function loadConvertRouteForReview(): Promise<ConvertRouteHandle> {
    const { convertSessionFor } = await import("./lib/convert-session.svelte");
    return convertSessionFor(convertRouteOwner, convertRouteBridge);
  }

  async function reviewConvertTask(task: TaskDialogModel): Promise<void> {
    if (task.spec.kind !== "convert") return;
    const generation = taskReviewRequestGeneration;
    const initialScreen = screen;
    const initialArchive = currentArchive?.id ?? null;
    const isCurrent = () => generation === taskReviewRequestGeneration
      && screen === initialScreen && (currentArchive?.id ?? null) === initialArchive;
    const spec = task.spec;
    const draft: ConvertTaskDraft = {
      src: spec.src, dest: spec.dest, level: spec.level, src_encoding: spec.src_encoding,
      encrypt_names: spec.encrypt_names, split_size: spec.split_size, split_mode: spec.split_mode,
      outputPasswordRequired: task.outputPasswordRequired,
    };
    let session: ConvertRouteHandle;
    try {
      session = await loadConvertRouteForReview();
    } catch {
      if (isCurrent()) showNotice(tr("gui.convert.review.load_failed"));
      return;
    }
    if (!isCurrent() || !session.canReviewTask(draft)) return;
    convertRouteHandle = session;
    session.syncArchive(currentArchive);
    await reviewArchiveTask(task, "convert", {
      path: draft.src,
      encoding: draft.src_encoding,
      restore: () => {
        if (!session.restoreTaskDraft(draft)) return false;
        setScreen("convert");
        focusConvertReview();
        showNotice(tr("gui.convert.review.restored"));
        return true;
      },
    });
  }

  function focusConvertReview(): void {
    convertReviewFocusPending = true;
    void tick().then(() => {
      const heading = document.getElementById("convert-workspace-heading");
      if (!convertReviewFocusPending || screen !== "convert" || blockingModalVisible() || !heading) return;
      convertReviewFocusPending = false;
      heading.focus();
    });
  }

  async function reviewArchiveTask(task: TaskDialogModel, target: "extract" | "convert", review: ArchiveTaskReview): Promise<void> {
    if (archiveOpenStatus === "opening") {
      showNotice(tr("gui.task.review.wait_for_open"));
      return;
    }
    if (preventCreateSubmissionNavigation(target) || preventConvertSubmissionNavigation(target) || focusBlockingTaskIfAny()) return;
    await dismissTaskDialog(task);
    if (currentArchive && sameFilePath(currentArchive.source, review.path)
      && (currentArchive.encoding_override ?? null) === review.encoding) {
      pendingArchiveTaskReview = null;
      review.restore();
      return;
    }
    await openArchivePath(review.path, "open-file", review);
  }

  function restoreExtractTaskDraft(draft: ExtractTaskDraft): boolean {
    if (!currentArchive) return false;
    const restored = extraction.restore(draft);
    if (!restored) return false;
    setScreen("extract");
    focusExtractReview();
    showNotice(tr("gui.extract.review.restored"));
    return true;
  }

  async function openTaskOutput(task: TaskDialogModel): Promise<void> {
    const outputPath = taskOutputPath(task);
    if (task.id === null || task.state !== "done" || !outputPath || !taskOutputCanOpen(task)) return;
    if (task.spec.kind === "compress") {
      if (await openArchivePath(outputPath, "open-file")) {
        await dismissTaskDialog(task);
      }
      return;
    }
    try {
      await ipc.openJobOutput(task.id);
      showNotice(
        taskOutputIsFolder(task)
          ? tr("gui.task.output_folder_opened")
          : tr("gui.task.output_opened"),
      );
    } catch {
      showNotice(tr("gui.task.open_output_failed"));
    }
  }

  async function revealTaskOutput(task: TaskDialogModel): Promise<void> {
    const outputPath = taskOutputPath(task);
    if (!outputPath) return;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(outputPath);
    } catch {
      showNotice(tr("gui.task.reveal_failed"));
    }
  }

  function passwordPromptName(): string {
    return (archivePasswordPrompt ? pathBaseName(archivePasswordPrompt.path) : null)
      ?? previewPasswordPrompt?.name
      ?? tr("gui.password.no_prompt", "No password prompt");
  }

  function passwordPromptDetail(): string {
    if (archivePasswordPrompt) {
      return archivePasswordPrompt.wrong
        ? tr("gui.password.open_previous_rejected")
        : tr("gui.password.archive_waiting");
    }
    if (previewPasswordPrompt) {
      if (previewPasswordPrompt.wrong) return tr("gui.preview.password_rejected");
      return previewPasswordPrompt.scope === "inner"
        ? tr("gui.preview.inner_password")
        : tr("gui.preview.outer_password");
    }
    return tr("gui.password.no_prompt_pending", "No password request is active.");
  }

  function passwordSessionDetail(): string {
    if (previewPasswordPrompt && !archivePasswordPrompt) {
      return tr("gui.preview.password_session");
    }
    return tr("gui.password.open_session_only");
  }

  function passwordFailureDetail(): string {
    return tr("gui.password.open_return_to_prompt");
  }

  function isCurrentTaskPasswordPrompt(prompt: typeof jobPasswordPrompt): boolean {
    return prompt !== null && jobPasswordPrompt?.id === prompt.id && jobPasswordPrompt.version === prompt.version;
  }

  async function submitTaskPasswordRequest(prompt: typeof jobPasswordPrompt) {
    if (!prompt || !isCurrentTaskPasswordPrompt(prompt)) {
      showNotice(tr("gui.password.no_prompt_pending", "No password request is active."));
      return;
    }
    jobPasswordSubmissionAttempted = true;
    if (!taskPasswordReady(jobPasswordValue)) return;
    jobPasswordSubmissionAttempted = false;
    const promptId = prompt.id;
    const answer = answerJobPassword(jobPasswordValue);
    jobPasswordValue = "";
    if (!await answer) return;
    showNotice(tr("gui.password.sent_to_task"));
    if (jobPasswordPrompt || jobConflictPrompt) return;
    returnTaskQuestionToCenter(promptId);
  }

  async function cancelTaskPasswordRequest(prompt: typeof jobPasswordPrompt) {
    if (!prompt || !isCurrentTaskPasswordPrompt(prompt)) return;
    jobPasswordSubmissionAttempted = false;
    jobPasswordValue = "";
    const promptId = prompt.id;
    if (!await answerJobPassword(null)) return;
    showNotice(tr("gui.password.prompt_cancelled"));
    if (jobPasswordPrompt || jobConflictPrompt) return;
    returnTaskQuestionToCenter(promptId);
  }

  async function submitPasswordRequest() {
    if (!archivePasswordPrompt && !previewPasswordPrompt) {
      showNotice(tr("gui.password.no_prompt_pending", "No password request is active."));
      return;
    }
    workspacePasswordSubmissionAttempted = true;
    if (!taskPasswordReady(workspacePasswordValue)) return;
    workspacePasswordSubmissionAttempted = false;
    if (!archivePasswordPrompt && previewPasswordPrompt) {
      if (previewSession.answerPassword(workspacePasswordValue)) workspacePasswordValue = "";
      return;
    }
    const prompt = archivePasswordPrompt;
    if (!prompt) return;
    if (archiveOpenStatus === "opening") return;
    const requestGeneration = prompt.refresh ? archiveOpenGeneration : ++archiveOpenGeneration;
    const attempt = ++archivePasswordAttempt;
    standalonePasswordFocusedInput = null;
    archiveOpenStatus = "opening";
    const ok = await openArchiveStore(prompt.path, workspacePasswordValue, prompt.encoding);
    if (requestGeneration !== archiveOpenGeneration || attempt !== archivePasswordAttempt) return;
    archiveOpenStatus = "idle";
    workspacePasswordValue = "";
    if (ok) {
      if (prompt.refresh) {
        setScreen("browse");
        showNotice(tr("gui.archive.unlocked"));
        return;
      }
      finishOpenedArchive(prompt.path, "password");
      return;
    }
    if (openPasswordPrompt()?.path === prompt.path) {
      showNotice(tr("gui.password.open_previous_rejected"));
      return;
    }
    if (prompt.refresh) {
      setScreen("browse");
      return;
    }
    pendingArchiveTaskReview = null;
    if (archiveOpenError(prompt.path)?.key === "error.corrupt_archive") {
      recoverySession.leave();
      recoverySession.selectSource(prompt.path);
      setScreen("recovery");
      showNotice(
        tr("gui.recovery.open_failed_routed")
          .replace("{name}", pathBaseName(prompt.path)),
      );
      return;
    }
    if (!archiveOpenError(prompt.path)) return;
    setScreen("browse");
    showNotice(archiveOpenFailureNotice(prompt.path));
  }

  function dismissArchivePasswordRequest() {
    const prompt = archivePasswordPrompt;
    if (!prompt) return;
    pendingArchiveTaskReview = null;
    archivePasswordAttempt += 1;
    if (!prompt.refresh) archiveOpenGeneration += 1;
    archiveOpenStatus = "idle";
    workspacePasswordValue = "";
    workspacePasswordSubmissionAttempted = false;
    cancelArchivePasswordPrompt();
  }

  async function cancelPasswordRequest() {
    workspacePasswordSubmissionAttempted = false;
    if (previewPasswordPrompt && !archivePasswordPrompt) {
      clearEntryPreviewState(true);
      showNotice(tr("gui.preview.password_cancelled"));
    }
    if (archivePasswordPrompt) {
      const refreshing = archivePasswordPrompt.refresh;
      dismissArchivePasswordRequest();
      showNotice(refreshing
        ? tr("gui.archive.refresh_cancelled")
        : tr("gui.password.archive_open_cancelled"));
    }
    workspacePasswordValue = "";
    setScreen("browse");
  }

  function isCurrentTaskConflictPrompt(prompt: typeof jobConflictPrompt): boolean {
    return prompt !== null && jobConflictPrompt?.id === prompt.id && jobConflictPrompt.version === prompt.version;
  }

  async function answerConflictDecision(prompt: typeof jobConflictPrompt, decision: TaskConflictDecision, applyAll: boolean) {
    if (!prompt || !isCurrentTaskConflictPrompt(prompt)) {
      showNotice(tr("gui.conflict.no_prompt_pending", "No conflict request is active"));
      return;
    }
    const answer = normalizeTaskConflictAnswer(decision, applyAll);
    const promptId = prompt.id;
    if (!await answerJobConflict(answer.decision, answer.applyAll)) return;
    if (answer.decision === "abort") {
      showNotice(tr("gui.task.cancel_requested"));
    } else {
      showNotice(
        answer.applyAll
          ? tr("gui.conflict.decision_applied_remaining")
          : tr("gui.conflict.decision_sent"),
      );
    }
    if (jobPasswordPrompt || jobConflictPrompt) return;
    conflictApplyAll = false;
    returnTaskQuestionToCenter(promptId);
  }

  function currentArchiveName(): string {
    return currentArchive?.name ?? noArchiveLabel();
  }

  function passwordBookSecretStoreLabel(): string {
    if (!currentArchive) return tr("gui.settings.password_book.not_checked");
    if (passwordBookStatus.state === "checking") return tr("gui.settings.password_book.checking");
    if (passwordBookStatus.state === "idle") return tr("gui.settings.password_book.not_checked");
    if (passwordBookStatus.state === "error") return tr("gui.settings.password_book.unavailable_status");
    return passwordBookStatus.available
      ? tr("gui.settings.password_book.available")
      : tr("gui.settings.password_book.unavailable_status");
  }

  function passwordBookCurrentLabel(): string {
    if (!currentArchive) return noArchiveLabel();
    if (currentArchive.read_only) return tr("gui.settings.password_book.unavailable_status");
    if (passwordBookStatus.state === "checking") return tr("gui.settings.password_book.checking");
    const saved = passwordBookStatus.saved === null
      ? tr("gui.settings.password_book.saved_unknown")
      : passwordBookStatus.saved
      ? tr("gui.settings.password_book.saved_status")
      : tr("gui.settings.password_book.not_saved_status");
    return passwordBookStatus.session
      ? `${tr("gui.settings.password_book.session_available")} · ${saved}`
      : saved;
  }

  function passwordBookDetailLabel(): string {
    if (!currentArchive) return tr("gui.settings.password_book.open_archive_to_check");
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable");
    }
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.checking_detail");
    }
    if (passwordBookStatus.state === "error") {
      return tr("gui.settings.password_book.check_failed_detail");
    }
    if (passwordBookStatus.state === "idle") {
      return tr("gui.settings.password_book.refresh_to_check");
    }
    if (!passwordBookStatus.available) {
      return tr("gui.settings.password_book.secret_store_unavailable_detail");
    }
    return passwordBookStatus.saved
      ? tr("gui.settings.password_book.current_has_saved_entry")
      : tr("gui.settings.password_book.prompt_or_save_after_unlock");
  }

  function passwordBookForgetDisabledReason(): string {
    if (archivePasswordSaveBusy()) return tr("gui.settings.password_book.wait_for_save");
    if (!currentArchive) return openArchiveFirstLabel();
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable");
    }
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.wait_for_status");
    }
    if (passwordBookStatus.session) return "";
    if (passwordBookStatus.state === "error") {
      return tr("gui.settings.password_book.refresh_after_failure");
    }
    if (passwordBookStatus.state !== "ready") {
      return tr("gui.settings.password_book.refresh_before_forgetting");
    }
    if (passwordBookStatus.saved === null) {
      return tr("gui.settings.password_book.refresh_after_failure");
    }
    if (!passwordBookStatus.saved) {
      return tr("gui.settings.password_book.no_saved_entry");
    }
    return "";
  }

  function passwordBookRefreshDisabledReason(): string {
    if (archivePasswordSaveBusy()) return tr("gui.settings.password_book.wait_for_save");
    if (!currentArchive) return openArchiveFirstLabel();
    if (currentArchive.read_only) {
      return tr("gui.settings.password_book.nested_unavailable");
    }
    if (passwordBookStatus.state === "checking") {
      return tr("gui.settings.password_book.wait_for_status");
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
      showNotice(tr("gui.settings.password_book.status_refreshed"));
    } catch {
      showNotice(tr("gui.settings.password_book.status_check_failed"));
    }
  }

  async function forgetPasswordBookPanel() {
    const disabledReason = passwordBookForgetDisabledReason();
    if (disabledReason) {
      showNotice(disabledReason);
      return;
    }

    await forgetCurrentArchivePassword();
  }

  function clearNotice() {
    if (noticeTimer) clearTimeout(noticeTimer);
    appNotice = null;
    noticeTimer = null;
  }

  function showNotice(message: string) {
    clearNotice();
    appNotice = message;
    noticeTimer = setTimeout(() => {
      appNotice = null;
      noticeTimer = null;
    }, 2600);
  }

  const convertRouteBridge: ConvertRouteBridge = {
    onReady: () => {
      if (convertReviewFocusPending) focusConvertReview();
    },
    getArchive: () => currentArchive,
    tr,
    tError: (error) => isErrorDto(error) ? tError(error) : String(error),
    showNotice,
    ensurePreflightListener: create.ensureEvents,
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

  function classicCommandAction(label: string): AppAction | undefined {
    const actions: Record<string, AppAction> = {
      Add: "add_files",
      "Extract To": selectedPaths().size > 0 ? "extract_selection" : "extract_all",
      Test: "test_archive",
      View: "preview_entry",
      Delete: "delete_entries",
      Rename: "rename_entry",
      Move: "move_entries",
      "New Folder": "new_folder",
      Convert: "convert_archive",
      Info: "archive_info",
    };
    return actions[label];
  }

  function handleClassicCommand(label: string) {
    const action = classicCommandAction(label);
    if (action) {
      void runAppAction(action);
      return;
    }
    if (label === "Protect") {
      openRecoveryConfiguration();
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
    setScreen(screenForCommand(label));
  }

  function classicCommandDisabled(label: string): boolean {
    const action = classicCommandAction(label);
    if (action) return !appActionEnabled(action);
    if (label === "Protect") return Boolean(currentArchive && archiveSourceMutationDisabledReason());
    if (label === "Checksum" || label === "Duplicates") return false;
    return !hasArchiveOpen();
  }

  function classicCommandDisabledTitle(label: string): string {
    if (!classicCommandDisabled(label)) return "";
    if (label === "View") return previewSelectedDisabledReason();
    const refreshStatus = archiveRefreshStatusLabel();
    if (refreshStatus) return refreshStatus;
    if (label === "Extract To") {
      const selectionBusyReason = archiveSelectionBusyReason();
      if (selectionBusyReason) return selectionBusyReason;
    }
    if (!currentArchive) {
      if (label === "Add") return tr("gui.precondition.open_before_add");
      if (label === "Extract To") return tr("gui.precondition.open_before_extract");
      if (label === "Test") return tr("gui.precondition.open_before_test");
      if (label === "Protect") return tr("gui.precondition.open_before_protect");
      if (label === "Delete") return tr("gui.precondition.open_before_delete");
      if (label === "Rename") return tr("gui.precondition.open_before_rename");
      if (label === "Move") return tr("gui.precondition.open_before_move");
      if (label === "New Folder") return tr("gui.precondition.open_before_new_folder");
      if (label === "Convert") {
        return currentArchive
          ? ""
          : tr("gui.precondition.open_before_convert");
      }
      return openArchiveFirstLabel();
    }
    if (label === "Protect") return archiveSourceMutationDisabledReason();
    if (label === "Rename") return renameSelectedDisabledReason();
    if (label === "Move") return moveSelectedDisabledReason();
    if (label === "Delete") return deleteSelectedDisabledReason();
    if (label === "Add" || label === "New Folder") return archiveMutationDisabledReason() || archiveSelectionBusyReason();
    return "";
  }

  function classicCommandTitle(label: string): string {
    const disabledTitle = classicCommandDisabledTitle(label);
    if (disabledTitle) return disabledTitle;
    if (label === "Extract To") return extractView.extractDestinationHint();
    if (label === "Info") return tr("gui.archive.info_title", "Show archive information");
    return "";
  }

  function classicCommandAriaLabel(label: string): string {
    return labelWithDisabledReason(classicCommandDisplayLabel(label), classicCommandDisabledTitle(label));
  }

  function classicCommandDisplayLabel(label: string): string {
    if (label === "View") {
      const entryPath = selectedPreviewPath();
      return entryPath ? previewActionLabel(entryPath) : tr("gui.action.open_preview");
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
    if (screen === "recent") return tr("gui.screen.recent");
    if (screen === "create") return tr("gui.screen.create");
    if (screen === "extract") return tr("gui.screen.extract");
    if (screen === "convert") return tr("gui.screen.convert");
    if (screen === "batch") return tr("gui.screen.batch");
    if (screen === "nestedExtract") return tr("gui.nested_extract.title");
    if (screen === "updateReview") return tr("gui.update_review.title");
    if (screen === "checksum") return tr("gui.screen.checksum");
    if (screen === "duplicates") return tr("gui.screen.duplicates");
    if (screen === "password") return tr("gui.screen.password");
    if (screen === "recovery") return tr("gui.screen.recovery");
    if (screen === "archiveInfo") return tr("gui.screen.archive_info");
    if (screen === "integration") return tr("gui.screen.integration");
    if (screen === "appearance") return tr("gui.screen.appearance");
    if (screen === "colors") return tr("gui.screen.colors");
    if (screen === "settingsGeneral") return tr("gui.screen.settings_general");
    if (screen === "settingsSecurity") return tr("gui.screen.settings_security");
    if (screen === "settingsPerformance") return tr("gui.screen.settings_performance");
    if (screen === "passwordBook") return tr("gui.screen.password_book");
    return archiveTitle();
  }
</script>

<div class:task-center-layout={taskCenterVisible()}>
  {#if !modeSelectionBlocked}
    <ToastHost
      rootClass={`themed-root palette-${settingsSession.colors.palette} theme-${activeTheme}`}
      rootVariables={customPaletteVariables()}
      blocked={blockingModalVisible()}
    />
  {/if}

  {#if taskCenterVisible()}
    {@const selectedTask = taskCenterSelectedTask()}
    {#if selectedTask}
      {@render taskProgressSurface(selectedTask, "panel")}
    {:else}
      <TaskCenterHost
        surface={taskCenterSurface()}
        loadingTitle={tr("gui.task_surface.center_loading")}
        loadingBody={tr("gui.task_surface.center_loading_body")}
        failureTitle={tr("gui.task_surface.center_load_failed")}
        failureBody={tr("gui.task_surface.center_load_failed_body")}
        retryLabel={tr("gui.task_surface.retry")}
        closeLabel={tr("gui.task_center.close")}
      />
    {/if}
  {/if}
</div>

{#if appNotice && (!firstRunRequired || taskWindowMode)}
  <div
    class={`app-notice mode-${mode} themed-root palette-${settingsSession.colors.palette} theme-${activeTheme}`}
    use:cssVariables={customPaletteVariables()}
    role="status"
    aria-hidden={blockingModalVisible() ? "true" : undefined}
  >{appNotice}</div>
{/if}

{#if macosSfxPublisherTask && LoadedMacosSfxPublisher}
  <LoadedMacosSfxPublisher
    task={macosSfxPublisherTask}
    rootClass={`sfx-publish-overlay design-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`}
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

{#snippet taskProgressSurface(task: TaskDialogModel, presentation: "panel" | "dialog" | "window")}
  <TaskProgressDialogHost
    surface={taskProgressDialogProps(task, presentation)}
    loadingTitle={tr("gui.task_surface.loading")}
    loadingBody={tr("gui.task_surface.loading_body")}
    failureTitle={tr("gui.task_surface.load_failed")}
    failureBody={tr("gui.task_surface.load_failed_body")}
    retryLabel={tr("gui.task_surface.retry")}
    backLabel={tr("gui.task.back_to_tasks")}
  />
{/snippet}

{#if !taskWindowMode && taskDialogVisible() && !macosSfxPublisherTask}
  {@const task = taskDialogTask()}
  {#if task}
    {@render taskProgressSurface(task, "dialog")}
  {/if}
{/if}

{#if (dragActive || lastDropKind !== "none") && !modeSelectionBlocked}
  <div
    class={`drop-status mode-${mode} themed-root palette-${settingsSession.colors.palette} theme-${activeTheme}`}
    use:cssVariables={customPaletteVariables()}
    class:active={dragActive}
    role="status"
    aria-hidden={blockingModalVisible() ? "true" : undefined}
  >{dropStatusLabel()}</div>
{/if}

{#if !taskWindowMode && archiveEdit.kind && archiveEditorVisible() && !taskDialogVisible() && !macosSfxPublisherTask}
  <ArchiveEntryEditor
    {...archiveEditorFields(archiveEdit.kind)}
    rootClass={`archive-editor-overlay design-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`}
    rootVariables={customPaletteVariables()}
    cancelLabel={tr("common.cancel")}
    onClose={() => archiveEdit.close()}
  />
{/if}

{#if entryContext}
  <div
    bind:this={entryContextMenu}
    class={`entry-context-menu themed-root palette-${settingsSession.colors.palette} theme-${activeTheme}`}
    use:cssVariables={entryContextCssVariables(entryContext)}
    role="menu"
    aria-label={tr("gui.context.actions_for").replace("{name}", entryContext.name)}
    aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined}
    inert={blockingModalVisible() || modeSelectionBlocked}
  >
    <div class="entry-context-head">
      <span>{tr("gui.context.selection_actions")}</span>
      <strong>{entryContext.name}</strong>
    </div>
    <button role="menuitem" disabled={!appActionEnabled("extract_selection")} title={archiveSelectionBusyReason() || (currentArchive ? "" : openArchiveFirstLabel())} onclick={() => void runEntryContextAction("extract")}><Icon name="archive" size={15} />{actionLabel("Extract selected")}</button>
    <button role="menuitem" disabled={!appActionEnabled("delete_entries")} title={deleteSelectedDisabledReason()} onclick={() => void runEntryContextAction("delete")}><Icon name="x-circle" size={15} />{actionLabel("Delete selected")}</button>
    <button role="menuitem" disabled={!entryContext.canRename || !appActionEnabled("rename_entry")} title={renameSelectedDisabledReason()} onclick={() => void runEntryContextAction("rename")}><Icon name="repeat" size={15} />{actionLabel("Rename selected")}</button>
    <button role="menuitem" disabled={!appActionEnabled("move_entries")} title={moveSelectedDisabledReason()} onclick={() => void runEntryContextAction("move")}><Icon name="repeat" size={15} />{actionLabel("Move selected")}</button>
    <button role="menuitem" disabled={!entryContext.path || !contextualAppActions({ path: entryContext.path, type: entryContext.isDir ? "dir" : "file" }).preview_entry} title={entryContext.path ? previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file") : tr("gui.preview.select_one")} onclick={() => void runEntryContextAction("preview")}><Icon name={previewActionIcon(entryContext.path, entryContext.isDir ? "dir" : "file")} size={15} />{previewActionLabel(entryContext.path, entryContext.isDir ? "dir" : "file")}</button>
    <button role="menuitem" disabled={!appActionEnabled("test_archive")} title={currentArchive ? "" : openArchiveFirstLabel()} onclick={() => void runEntryContextAction("test")}><Icon name="shield-alert" size={15} />{actionLabel("Test archive")}</button>
  </div>
{/if}

{#if firstRunRequired && !taskWindowMode}
  <div
    class={`first-run-overlay themed-root palette-${settingsSession.colors.palette} theme-${activeTheme}`}
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
          <h1 id="squallz-first-run-title">{tr("gui.first_run.title")}</h1>
          <p id="squallz-first-run-description">{tr("gui.first_run.body")}</p>
        </div>
      </div>

      <div class="first-run-choices">
        <section class="first-run-modern-card" aria-labelledby="squallz-first-run-modern-title">
          <div class="first-run-mode-head">
            <span class="first-run-mode-mark" aria-hidden="true"><Icon name="sparkles" size={20} /></span>
            <div class="first-run-mode-copy">
              <span class="mode-kicker">{tr("gui.first_run.recommended")}</span>
              <strong id="squallz-first-run-modern-title">{tr("gui.mode.modern")}</strong>
              <small>{tr("gui.first_run.modern_body")}</small>
            </div>
          </div>
          <span class="first-run-mode-features">
            <span><Icon name="sparkles" size={14} />{tr("gui.first_run.modern_focus")}</span>
            <span><Icon name="check-circle" size={14} />{tr("gui.first_run.modern_guided")}</span>
          </span>
          <div class="first-run-primary-actions">
            <button bind:this={firstRunRecommendedButton} class="primary" onclick={() => void startFirstRun("create")}>
              <Icon name="sparkles" size={16} />{tr("gui.classic.create_archive")}
            </button>
            <button aria-busy={archiveOpenStatus === "opening"} onclick={() => void startFirstRun("open")}>
              <Icon name="folder-open" size={16} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            </button>
          </div>
        </section>

        <button class="first-run-classic-card" onclick={() => setMode("classic")}>
          <span class="first-run-mode-mark" aria-hidden="true"><Icon name="list" size={20} /></span>
          <span class="first-run-mode-copy">
            <span class="mode-kicker">{tr("gui.first_run.power_workflow")}</span>
            <strong>{tr("gui.mode.classic")}</strong>
            <small>{tr("gui.first_run.classic_body")}</small>
          </span>
          <span class="first-run-classic-features">
            <span><Icon name="list" size={14} />{tr("gui.first_run.classic_table")}</span>
            <span><Icon name="search" size={14} />{tr("gui.first_run.classic_keyboard")}</span>
          </span>
          <span class="first-run-choice-action">
            {tr("gui.first_run.choose_mode").replace("{mode}", tr("gui.mode.classic"))}
            <Icon name="chevron-right" size={14} />
          </span>
        </button>
      </div>

      <footer class="first-run-footer">
        <span role="status" aria-live="polite" aria-atomic="true">{firstRunDropFeedback ?? tr("gui.first_run.mode_changed_settings")}</span>
        <button onclick={reviewFirstRunSettings}>{tr("gui.first_run.review_settings_action")}</button>
      </footer>
    </div>
  </div>
{/if}

{#if taskWindowMode}
  <main
    class={`design-root task-window-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`}
    use:cssVariables={customPaletteVariables()}
    aria-label={tr("gui.external_task.window_label")}
    aria-hidden={macosSfxPublisherTask ? "true" : undefined}
    inert={macosSfxPublisherTask !== null}
  >
    {#if taskDialogVisible()}
      {@const task = taskDialogTask()}
      {#if task}
        {@render taskProgressSurface(task, "window")}
      {/if}
    {:else}
      <section class="task-window-empty" role="status">
        <AppIcon size={42} title="Squallz" />
        <div>
          <h1>{taskWindowShellTitleCopy}</h1>
          <p>{taskWindowShellCopy}</p>
        </div>
        {#if displayedTaskWindowState.status === "reconnect-error" || displayedTaskWindowState.status === "task-unavailable"}
          <div class="task-window-empty-actions">
            <button class="primary-lite" type="button" onclick={() => reloadWindow({ taskWindow: true, taskId: taskDialogTaskId })}>
              <Icon name="rotate-cw" size={15} />{tr("gui.external_task.reconnect_action")}
            </button>
          </div>
        {/if}
      </section>
    {/if}
  </main>
{:else if mode === "modern" || isSettingsScreen()}
  <main class={`design-root modern-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`} use:cssVariables={customPaletteVariables()} class:drop-active={dragActive} aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined} inert={blockingModalVisible() || modeSelectionBlocked}>
    <section class="window modern-window" aria-label={isSettingsScreen() ? tr("gui.aria.squallz_settings") : tr("gui.aria.modern_archive_browser")}>
      <header class="modern-titlebar" data-tauri-drag-region>
        <div class="brand-lockup">
          <div class="brand-glyph"><AppIcon size={36} title="Squallz" /></div>
          <div>
            <strong>Squallz</strong>
            <span>{titleForScreen()}</span>
          </div>
        </div>
        <div class="modern-toolbar" aria-label={tr("gui.aria.primary_actions")}>
          {#if isSettingsScreen()}
            <button onclick={() => setScreen("browse")}><Icon name="archive" size={16} />{tr("gui.settings.back_to_archives", "Archives")}</button>
          {:else}
            <button disabled={!appActionEnabled("open_archive")} aria-busy={archiveOpenStatus === "opening"} onclick={() => void runAppAction("open_archive")}><Icon name="folder-open" size={16} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
            <button disabled={!appActionEnabled("create_archive")} onclick={() => void runAppAction("create_archive")}><Icon name="sparkles" size={16} />{toolbarLabel("Create")}</button>
            <button onclick={() => openRecoveryConfiguration()}><Icon name="shield-alert" size={16} />{tr("gui.recovery.title")}</button>
            <button class="primary" disabled={!appActionEnabled("extract_all")} title={archiveSelectionBusyReason() || archiveActionTitle(hasArchiveOpen())} onclick={() => void runAppAction("extract_all")}><Icon name="archive" size={16} />{toolbarLabel("Extract")}</button>
            <button
              bind:this={quickActionButton}
              class="icon-only"
              aria-label={tr("gui.quick.title")}
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
        <div bind:this={quickActionPopover} class="quick-popover modern-quick-popover" role="dialog" aria-label={tr("gui.quick.title")}>
          <div class="quick-popover-head">
            <strong>{tr("gui.quick.title")}</strong>
            <span>{tr("gui.quick.subtitle")}</span>
          </div>
          {#each quickActions as action}
            <button disabled={navigationDisabled(action.screen)} title={navigationDisabledReason(action.screen)} onclick={() => chooseQuickAction(action.screen)}>
              <Icon name={action.icon} size={15} />
              <span><strong>{quickActionLabel(action.label)}</strong><small>{quickActionDetail(action.label, action.detail)}</small></span>
            </button>
          {/each}
        </div>
      {/if}

      <div
        class="modern-shell"
        class:browse-shell={screen === "browse"}
        class:settings-shell={isSettingsScreen()}
        class:no-archive-shell={screen === "browse" && !currentArchive}
        class:no-inspector-shell={screen === "recent" || screen === "convert" || screen === "create" || screen === "extract" || screen === "duplicates" || screen === "batch" || screen === "nestedExtract" || screen === "updateReview" || screen === "password"}
      >
        <aside class="modern-sidebar" aria-label={tr("gui.aria.navigation")}>
          <div class="sidebar-section">
            {#each nav as item}
              <button
                disabled={navigationDisabled(screenForNav(item[1]))}
                title={navigationDisabledReason(screenForNav(item[1]))}
                class:current={(screen === "recent" && item[1] === "Recent") || ((screen === "browse" || screen === "updateReview") && item[1] === "Archives") || (screen === "create" && item[1] === "Create") || ((screen === "extract" || screen === "batch" || screen === "nestedExtract" || screen === "password") && item[1] === "Extract") || (screen === "convert" && item[1] === "Convert") || (screen === "checksum" && item[1] === "Checksum") || (screen === "duplicates" && item[1] === "Duplicates") || (screen === "recovery" && item[1] === "Recovery") || (isSettingsScreen() && item[1] === "Settings")}
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
              <span>{tr("gui.task_center.title")}</span>
              <strong>{taskCenterSummaryLabel()}</strong>
              <small>{historyLastLabel()}</small>
            </button>
          {/if}
        </aside>

        <section
          class="modern-content"
          class:settings-workspace={isSettingsScreen()}
          class:browse-workspace={screen === "browse"}
          aria-label={isSettingsScreen() ? tr("gui.settings.workspace") : tr("gui.aria.archive_contents")}
        >
          {#if isSettingsScreen()}
            <aside class="settings-workspace-rail" aria-label={tr("gui.settings.sections")}>
              <div class="panel-title"><Icon name="settings" size={16} />{tr("gui.settings.title")}</div>
              <SettingsRouteList sections={settingsSections} active={screen} labelFor={settingsSectionLabel} detailFor={settingsSectionDetail} onChoose={setScreen} />
            </aside>
          {/if}
          {#if showArchiveReturnBar()}
            <ArchiveReturnStrip
              title={archiveTitle()}
              detail={archiveReturnDetail()}
              contextLabel={tr("gui.archive.current_context")}
              actionLabel={tr("gui.archive.back_to_current")}
              onReturn={returnToCurrentArchive}
            />
          {/if}
          {#if screen === "recent"}
            <div class="create-sheet modern-recent">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.recent.eyebrow")}</span>
                  <h1>{tr("gui.recent.title", "Recent archives")}</h1>
                  <p>{tr("gui.recent.subtitle")}</p>
                </div>
                <button class="primary sheet-action" disabled={!appActionEnabled("open_archive")} onclick={() => void runAppAction("open_archive")}><Icon name="folder-open" size={17} />{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</button>
              </div>

              <div class="create-grid">
                <section class="create-main-panel">
                  <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.recent.list_title")}</div>
                  <div class="limits-table recent-table">
                    <div><b>{tr("common.name")}</b><b>{tr("common.path")}</b><b>{tr("common.action")}</b></div>
                    {#each recentFiles() as path}
                      <div><span>{pathBaseName(path) || path}</span><span>{path}</span><button onclick={() => void openArchivePath(path, "dialog")}>{tr("gui.recent.reopen")}</button></div>
                    {:else}
                      <div><span>{tr("gui.recent.none")}</span><span>{tr("gui.recent.open_to_start")}</span><button disabled={!appActionEnabled("open_archive")} onclick={() => void runAppAction("open_archive")}>{toolbarLabel("Open")}</button></div>
                    {/each}
                  </div>
                </section>
                <aside class="create-side-panel">
                  <section>
                    <div class="panel-title"><Icon name="check-circle" size={16} />{tr("gui.recent.current_title")}</div>
                    <strong>{currentArchive ? currentArchive.name : noArchiveLabel()}</strong>
                    <p>{currentArchive ? currentArchive.path : openArchiveFirstLabel()}</p>
                  </section>
                  <section>
                    <div class="panel-title"><Icon name="shield-alert" size={16} />{tr("gui.recent.privacy_title")}</div>
                    <p>{tr("gui.recent.privacy_body")}</p>
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
              loadingTitle={tr("gui.convert.workspace_loading")}
              loadingBody={tr("gui.convert.workspace_loading_body")}
              failureTitle={tr("gui.convert.workspace_load_failed")}
              failureBody={tr("gui.convert.workspace_load_failed_body")}
              retryLabel={tr("gui.convert.workspace_retry")}
            />
          {:else if screen === "create"}
            <ArchiveOperationWorkspaceHost
              kind="create"
              variant="modern"
              session={create}
              loadingTitle={tr("gui.create.workspace_loading")}
              loadingBody={tr("gui.create.workspace_loading_body")}
              failureTitle={tr("gui.create.workspace_load_failed")}
              failureBody={tr("gui.create.workspace_load_failed_body")}
              retryLabel={tr("gui.create.workspace_retry")}
            />
          {:else if screen === "extract"}
            <ArchiveOperationWorkspaceHost
              kind="extract"
              variant="modern"
              session={extraction} view={extractView} {presets} bridge={extractWorkspaceBridge()}
              loadingTitle={tr("gui.extract.workspace_loading")}
              loadingBody={tr("gui.extract.workspace_loading_body")}
              failureTitle={tr("gui.extract.workspace_load_failed")}
              failureBody={tr("gui.extract.workspace_load_failed_body")}
              retryLabel={tr("gui.extract.workspace_retry")}
            />
          {:else if screen === "batch"}
            <ToolsWorkspaceHost surface={extractionReviewSurface("batch", "modern")} />
          {:else if screen === "nestedExtract"}
            <ToolsWorkspaceHost surface={extractionReviewSurface("nested", "modern")} />
          {:else if screen === "updateReview"}
            <ToolsWorkspaceHost surface={archiveUpdateWorkspaceSurface("modern")} />
          {:else if screen === "checksum"}
            <ToolsWorkspaceHost surface={localToolsSurface("checksum", "modern")} />
          {:else if screen === "duplicates"}
            <ToolsWorkspaceHost surface={localToolsSurface("duplicates", "modern")} />
          {:else if screen === "password"}
            <PasswordWorkspaceHost
              surface={passwordWorkspaceSurface("modern")}
            />
          {:else if screen === "recovery"}
            <RecoveryWorkspaceHost
              variant="modern"
              session={recoverySession}
              {tr}
            />
          {:else if screen === "archiveInfo"}
            <div class="settings-view modern-archive-info">
              <div class="sheet-head">
                <div>
                  <span class="eyebrow">{tr("gui.archive.info_eyebrow")}</span>
                  <h1>{tr("gui.archive.info_title")}</h1>
                  <p>{tr("gui.archive.info_subtitle")}</p>
                </div>
                <div class="sheet-action-row">
                  <button class="sheet-action" onclick={() => setScreen("browse")}><Icon name="archive" size={17} />{tr("gui.nav.back_to_archive")}</button>
                  <button
                    class="sheet-action"
                    disabled={!appActionEnabled("test_archive")}
                    title={testArchiveDisabledReason()}
                    aria-label={labelWithDisabledReason(tr("gui.action.test_archive"), testArchiveDisabledReason())}
                    onclick={() => void runAppAction("test_archive")}
                  ><Icon name="check-circle" size={17} />{tr("gui.action.test_archive")}</button>
                </div>
              </div>

              <div class="settings-layout">
                <section class="settings-main-panel">
                  <div class="limits-table archive-info-table">
                    <div><b>{tr("common.field")}</b><b>{tr("common.value")}</b></div>
                    {#each archiveInfoRows() as row}
                      <div><span>{row[0]}</span><strong>{row[1]}</strong></div>
                    {/each}
                  </div>
                </section>
                <aside class="settings-side-panel">
                  <div class="panel-title"><Icon name="archive" size={16} />{extractView.extractDestinationFieldLabel()}</div>
                  <div class="setting-callout">
                    <strong>{extractView.extractDestinationTitle(extraction.state.destinationMode)}</strong>
                    <span>{extraction.destination}</span>
                  </div>
                  <div class="settings-route-list">
                    <button class="settings-route-card" onclick={() => openExtractWorkspace("all")}>
                      <Icon name="archive" size={16} />
                      <span><strong>{tr("gui.screen.extract")}</strong><small>{extractView.extractDestinationHint()}</small></span>
                    </button>
                    <button class="settings-route-card" onclick={() => setScreen("checksum")}>
                      <Icon name="check-circle" size={16} />
                      <span><strong>{tr("gui.screen.checksum")}</strong><small>{tr("gui.checksum.route_from_info")}</small></span>
                    </button>
                  </div>
                </aside>
              </div>
            </div>
          {:else if isSettingsWorkspaceScreen(screen)}
            <SettingsWorkspaceHost
              workspace={settingsWorkspaceProps(screen)}
              loadingTitle={tr("gui.settings.workspace_loading")}
              loadingBody={tr("gui.settings.workspace_loading_body")}
              failureTitle={tr("gui.settings.workspace_load_failed")}
              failureBody={tr("gui.settings.workspace_load_failed_body")}
              retryLabel={tr("gui.settings.workspace_retry")}
            />
	          {:else}
	            <div class="archive-workspace" class:no-archive={!currentArchive}>
	              {#if currentArchive}
	                <ModernArchiveBrowserHost
	                  surface={modernArchiveBrowserSurface()}
	                  loadingTitle={tr("gui.archive.browser_loading")}
	                  loadingBody={tr("gui.archive.browser_loading_body")}
	                  failureTitle={tr("gui.archive.browser_load_failed")}
	                  failureBody={tr("gui.archive.browser_load_failed_body")}
	                  retryLabel={tr("gui.archive.browser_retry")}
	                />
		            {:else}
	              <ArchiveStartState
	                variant="modern"
	                eyebrow={tr("gui.archive.secure_archive")}
	                title={noArchiveLabel()}
	                body={tr("gui.empty.no_archive_summary")}
	                openLabel={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
	                createLabel={toolbarLabel("Create")}
	                openBusy={archiveOpenStatus === "opening"}
	                onOpen={() => void runAppAction("open_archive")}
	                onCreate={() => void runAppAction("create_archive")}
		              />
		            {/if}
	            </div>
	          {/if}
        </section>

        {#if !isSettingsScreen() && screen !== "recent" && screen !== "convert" && screen !== "create" && screen !== "extract" && screen !== "duplicates" && screen !== "batch" && screen !== "nestedExtract" && screen !== "updateReview" && screen !== "password" && (screen !== "browse" || currentArchive)}
          <ModernInspectorHost
            surface={modernInspectorSurface()}
            ariaLabel={tr("gui.aria.archive_inspector")}
            loadingTitle={tr("gui.inspector.workspace_loading")}
            loadingBody={tr("gui.inspector.workspace_loading_body")}
            failureTitle={tr("gui.inspector.workspace_load_failed")}
            failureBody={tr("gui.inspector.workspace_load_failed_body")}
            retryLabel={tr("gui.inspector.workspace_retry")}
          />
        {/if}
      </div>
    </section>
  </main>
{:else}
  <main class={`design-root classic-root platform-${activePlatform} palette-${settingsSession.colors.palette} theme-${activeTheme} density-${activeDensityChoice}`} use:cssVariables={customPaletteVariables()} class:drop-active={dragActive} aria-hidden={blockingModalVisible() || modeSelectionBlocked ? "true" : undefined} inert={blockingModalVisible() || modeSelectionBlocked}>
    <section class="window classic-window" aria-label={tr("gui.aria.classic_archive_browser")}>
      <header class="classic-titlebar" data-tauri-drag-region>
        <div class="classic-title">
          <AppIcon size={19} title="Squallz" />
          <strong>{titleForScreen()}</strong>
          <span>Squallz Classic</span>
        </div>
        <div class="classic-top-actions">
          <button
            disabled={!appActionEnabled("open_archive")}
            aria-busy={archiveOpenStatus === "opening"}
            aria-label={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            title={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
            onclick={() => void runAppAction("open_archive")}
          >
            <Icon name="folder-open" size={15} />
            <span class="classic-action-label">{archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}</span>
          </button>
          <button
            aria-label={tr("gui.classic.new_archive")}
            title={tr("gui.classic.new_archive")}
            disabled={!appActionEnabled("create_archive")}
            onclick={() => void runAppAction("create_archive")}
          >
            <Icon name="archive" size={15} />
            <span class="classic-action-label">{tr("gui.classic.new_archive")}</span>
          </button>
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
            <span class="classic-action-label">{tr("gui.task_center.short_title")}</span>
            {#if taskCenterBadgeCount() > 0}
              <span class="task-center-trigger-badge">{Math.min(taskCenterBadgeCount(), 99)}</span>
            {/if}
          </button>
          <button
            aria-label={navLabel("Settings")}
            title={navLabel("Settings")}
            disabled={!appActionEnabled("settings")}
            onclick={() => void runAppAction("settings")}
          >
            <Icon name="settings" size={15} />
            <span class="classic-action-label">{navLabel("Settings")}</span>
          </button>
        </div>
      </header>

      <div class="classic-commandbar" aria-label={tr("gui.aria.archive_commands")}>
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
            class:empty-address={!currentArchive}
          >
          <div class="classic-path-navigation" aria-label={tr("gui.nav.archive_navigation")}>
            <button
              class="path-button"
              disabled={!appActionEnabled("go_up")}
              aria-label={tr("gui.nav.up")}
              title={tr("gui.nav.up")}
              onclick={() => void runAppAction("go_up")}
            ><Icon name="chevron-up" size={14} /><span>{tr("gui.nav.up_short")}</span></button>
            <button
              class="path-button"
              disabled={!canGoUpArchive()}
              aria-label={tr("gui.nav.root")}
              title={tr("gui.nav.root")}
              onclick={() => void openArchiveBreadcrumb(-1)}
            ><Icon name="archive" size={14} /><span>{tr("gui.nav.root_short")}</span></button>
          </div>
          <div class="address" bind:this={classicArchiveAddress}>
            <button type="button" disabled={!currentArchive} title={archiveTitle()} onclick={() => void openArchiveBreadcrumb(-1)}>{archiveTitle()}</button>
            {#each archiveDirs as dir, index}
              <i>/</i><button type="button" title={dir} onclick={() => void openArchiveBreadcrumb(index)}>{dir}</button>
            {/each}
          </div>
          {#if screen === "extract" || screen === "batch" || screen === "nestedExtract"}
            <div class="encoding-chip accent"><Icon name="archive" size={14} />{screen === "nestedExtract" ? tr("gui.nested_extract.title") : screen === "batch" ? tr("gui.batch.title") : extractView.extractDestinationTitle(extraction.state.destinationMode)}</div>
          {:else if screen === "password"}
            <div class="encoding-chip warning"><Icon name="lock" size={14} />{tr("gui.password.required")}</div>
          {:else if screen === "recovery"}
            <div class="encoding-chip accent"><Icon name="shield-alert" size={14} />{recoverySession.sourceName() ?? tr("gui.recovery.no_archive_selected")}</div>
          {:else if screen === "checksum"}
            <div class="encoding-chip accent"><Icon name="check-circle" size={14} />{tools.algorithmLabel()}</div>
          {:else if screen === "duplicates"}
            <div class="encoding-chip accent"><Icon name="search" size={14} />{tr("gui.duplicates.blake3_scan")}</div>
          {:else if currentArchive && hasArchiveStructureWarning()}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{tr("gui.archive.zip_index_damaged")}</div>
          {:else if currentArchive && hasEncodingWarning()}
            <div class="encoding-chip warning"><Icon name="alert-triangle" size={14} />{tr("gui.encoding.gbk_suggested")}</div>
          {:else if currentArchive}
            <div
              class="encoding-chip accent archive-state-chip"
              aria-label={tr("gui.archive.open")}
              title={tr("gui.archive.open")}
            ><Icon name="archive" size={14} /><span>{tr("gui.archive.open")}</span></div>
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
                aria-label={tr("gui.list.search_aria")}
                aria-busy={filterPending()}
                title={tr("gui.list.search_shortcut")}
                placeholder={tr("gui.list.search_placeholder")}
                oninput={(event) => updateArchiveFilter(event.currentTarget.value)}
                onkeydown={onArchiveFilterKeydown}
              />
              {#if filterText()}
                <button type="button" aria-label={tr("gui.list.search_clear")} title={tr("gui.list.search_clear")} onclick={clearArchiveFilter}><Icon name="x-circle" size={13} /></button>
              {/if}
            </div>
            <button
              bind:this={quickActionButton}
              class="classic-quick-trigger"
              aria-label={tr("gui.quick.title")}
              title={tr("gui.quick.title")}
              aria-haspopup="dialog"
              aria-expanded={activePopover === "quickActions"}
              onclick={toggleQuickActions}
            ><Icon name="sparkles" size={14} /></button>
          </div>
          </div>
        {/if}

      {#if activePopover === "quickActions"}
        <div bind:this={quickActionPopover} class="quick-popover classic-quick-popover" role="dialog" aria-label={tr("gui.quick.title")}>
          <div class="quick-popover-head">
            <strong>{tr("gui.quick.title")}</strong>
            <span>{tr("gui.quick.close_hint")}</span>
          </div>
          {#each quickActions as action}
            <button disabled={navigationDisabled(action.screen)} title={navigationDisabledReason(action.screen)} onclick={() => chooseQuickAction(action.screen)}>
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
                <h1>{tr("gui.archive.info_title")}</h1>
                <p>{tr("gui.archive.info_subtitle")}</p>
              </div>
              <div class="classic-button-row">
                <button onclick={() => setScreen("browse")}>{tr("gui.nav.back_to_archive")}</button>
                <button
                  disabled={!appActionEnabled("test_archive")}
                  title={testArchiveDisabledReason()}
                  aria-label={labelWithDisabledReason(tr("gui.action.test_archive"), testArchiveDisabledReason())}
                  onclick={() => void runAppAction("test_archive")}
                >{tr("gui.action.test_archive")}</button>
                <button
                  class="classic-primary"
                  disabled={Boolean(extraction.archiveRequiredReason)}
                  title={extraction.archiveRequiredReason}
                  aria-label={labelWithDisabledReason(tr("gui.screen.extract"), extraction.archiveRequiredReason)}
                  onclick={() => openExtractWorkspace("all")}
                >{tr("gui.screen.extract")}</button>
              </div>
            </header>
            <div class="classic-batch-grid">
              <section>
                <h2>{tr("gui.inspector.archive")}</h2>
                <div class="classic-form-grid compact">
                  {#each archiveInfoRows() as row}
                    <div class="classic-label">{row[0]}</div>
                    <div class="classic-input classic-copy-wrap">{row[1]}</div>
                  {/each}
                </div>
              </section>
              <aside>
                <h2>{extractView.extractDestinationFieldLabel()}</h2>
                <div class="classic-form-grid compact no-pad">
                  <div class="classic-label">{tr("common.mode")}</div>
                  <div class="classic-input accent">{extractView.extractDestinationTitle(extraction.state.destinationMode)}</div>
                  <div class="classic-label">{tr("common.destination")}</div>
                  <div class="classic-input classic-copy-wrap">{extraction.destination}</div>
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
          loadingTitle={tr("gui.convert.workspace_loading")}
          loadingBody={tr("gui.convert.workspace_loading_body")}
          failureTitle={tr("gui.convert.workspace_load_failed")}
          failureBody={tr("gui.convert.workspace_load_failed_body")}
          retryLabel={tr("gui.convert.workspace_retry")}
        />
      {:else if screen === "create"}
        <ArchiveOperationWorkspaceHost
          kind="create"
          variant="classic"
          session={create}
          loadingTitle={tr("gui.create.workspace_loading")}
          loadingBody={tr("gui.create.workspace_loading_body")}
          failureTitle={tr("gui.create.workspace_load_failed")}
          failureBody={tr("gui.create.workspace_load_failed_body")}
          retryLabel={tr("gui.create.workspace_retry")}
        />
      {:else if screen === "extract"}
        <ArchiveOperationWorkspaceHost
          kind="extract"
          variant="classic"
          session={extraction} view={extractView} {presets} bridge={extractWorkspaceBridge()}
          loadingTitle={tr("gui.extract.workspace_loading")}
          loadingBody={tr("gui.extract.workspace_loading_body")}
          failureTitle={tr("gui.extract.workspace_load_failed")}
          failureBody={tr("gui.extract.workspace_load_failed_body")}
          retryLabel={tr("gui.extract.workspace_retry")}
        />
      {:else if screen === "batch"}
        <ToolsWorkspaceHost surface={extractionReviewSurface("batch", "classic")} />
      {:else if screen === "nestedExtract"}
        <ToolsWorkspaceHost surface={extractionReviewSurface("nested", "classic")} />
      {:else if screen === "updateReview"}
        <ToolsWorkspaceHost surface={archiveUpdateWorkspaceSurface("classic")} />
      {:else if screen === "checksum"}
        <ToolsWorkspaceHost surface={localToolsSurface("checksum", "classic")} />
      {:else if screen === "duplicates"}
        <ToolsWorkspaceHost surface={localToolsSurface("duplicates", "classic")} />
      {:else if screen === "password"}
        <PasswordWorkspaceHost
          surface={passwordWorkspaceSurface("classic")}
        />
      {:else if screen === "recovery"}
        <div class="classic-dialog-body" class:with-archive-return={showArchiveReturnBar()}>
          {#if showArchiveReturnBar()}
            <ArchiveReturnStrip
              title={archiveTitle()}
              detail={archiveReturnDetail()}
              contextLabel={tr("gui.archive.current_context")}
              actionLabel={tr("gui.archive.back_to_current")}
              buttonClass="classic-primary"
              iconSize={15}
              onReturn={returnToCurrentArchive}
            />
          {/if}
          <RecoveryWorkspaceHost
            variant="classic"
            session={recoverySession}
            {tr}
          />
        </div>
      {:else}
        {#if classicArchiveStartVisible()}
          <div class="classic-start-body">
            <ArchiveStartState
              variant="classic"
              eyebrow={tr("gui.archive.secure_archive")}
              title={noArchiveLabel()}
              body={tr("gui.empty.no_archive_summary")}
              openLabel={archiveOpenStatus === "opening" ? toolbarLabel("Opening") : toolbarLabel("Open")}
              createLabel={tr("gui.classic.create_archive")}
              openBusy={archiveOpenStatus === "opening"}
              onOpen={() => void runAppAction("open_archive")}
              onCreate={() => void runAppAction("create_archive")}
            />
          </div>
        {:else}
          <ClassicArchiveBrowserHost
            surface={classicArchiveBrowserSurface()}
            loadingTitle={tr("gui.archive.browser_loading")}
            loadingBody={tr("gui.archive.browser_loading_body")}
            failureTitle={tr("gui.archive.browser_load_failed")}
            failureBody={tr("gui.archive.browser_load_failed_body")}
            retryLabel={tr("gui.archive.browser_retry")}
          />
        {/if}
      {/if}

      <footer class="classic-statusbar">
        {#if screen === "create"}
          <span>{create.preflight.state.plan ? tr("gui.create.source_files_count").replace("{count}", create.preflight.state.plan.files.toLocaleString()) : tr("gui.create.source_files_pending")}</span>
          <span>{create.options.state.sfxEnabled ? create.view.sfxOutputLabel() : create.formatData.label} · {create.view.methodLabel()}</span>
          <span>{create.view.splitCapability()} · {create.view.formatRecovery()}</span>
          <strong>{create.view.diskPreflightStatusbar()}</strong>
        {:else if screen === "convert"}
          <span>{tr("gui.convert.title", "Convert archive")}</span>
          <span>{tr("gui.convert.status_source_target")
            .replace("{source}", convertRouteStatus.sourceFormat)
            .replace("{target}", convertRouteStatus.targetLabel)}</span>
          <span>{convertRouteStatus.profileLabel} · {convertRouteStatus.methodLabel}</span>
          <strong>{tr("gui.convert.status_destination").replace("{destination}", convertRouteStatus.destination)}</strong>
        {:else if screen === "extract"}
          {@const extractActionStatus = extractView.extractActionLabel()}
          {@const extractScopeStatus = `${extractView.extractSelectionLabel()} · ${extractView.extractDestinationTitle(extraction.state.destinationMode)}`}
          {@const extractConflictStatus = tr("gui.extract.status_conflicts").replace("{mode}", extractOverwriteLabel(extraction.state.overwrite)).replace("{password}", extractPasswordLabel())}
          {@const extractConflictCompactStatus = tr("gui.extract.status_conflicts").replace("{mode}", extractOverwriteLabel(extraction.state.overwrite)).replace("{password}", extractPasswordStatusbarLabel())}
          {@const extractDestinationStatus = tr("gui.extract.status_destination").replace("{destination}", extraction.effectiveDestination)}
          <span title={extractActionStatus}>{extractActionStatus}</span>
          <span title={extractScopeStatus}>{extractScopeStatus}</span>
          <span title={extractConflictStatus}>{extractConflictCompactStatus}</span>
          <strong title={extractDestinationStatus}>{extractDestinationStatus}</strong>
        {:else if screen === "updateReview"}
          <span>{tr("gui.update_review.title")}</span>
          <strong>{tr("gui.update_review.count").replace("{count}", String(archiveUpdateReview.selectedCount()))}</strong>
        {:else if screen === "nestedExtract"}
          <span>{tr("gui.nested_extract.title")}</span>
          <span>{extraction.review.nested ? pathBaseName(extraction.review.nested.entry_path) : tr("gui.nested_extract.empty")}</span>
          <span>{extractOverwriteLabel(extraction.review.nested?.overwrite ?? "ask")}</span>
          <strong>{tr("gui.batch.check_at_start")}</strong>
        {:else if screen === "batch"}
          <span>{tr("gui.batch.title")}</span>
          <span>{tr("gui.batch.archive_count").replace("{count}", String(extraction.review.batch.state.items.length))}</span>
          <span>{extractOverwriteLabel(extraction.review.batch.state.overwrite)}</span>
          <strong>{tr("gui.batch.check_at_start")}</strong>
        {:else if screen === "checksum"}
          <span>{tr("gui.screen.checksum")}</span>
          <span>{tr("gui.status.target").replace("{target}", tools.targetName("checksum"))}</span>
          <span>{tr("gui.checksum.status_algorithm_excludes").replace("{algorithm}", tools.algorithmLabel()).replace("{count}", String(tools.excludes("checksum").length))}</span>
          <strong>{tr("gui.checksum.status_failed_report").replace("{count}", (tools.result("checksum_check")?.failed ?? 0).toLocaleString())}</strong>
        {:else if screen === "duplicates"}
          <span>{tr("gui.screen.duplicates")}</span>
          <span>{tr("gui.status.target").replace("{target}", tools.targetName("duplicate_scan"))}</span>
          <span>{tr("gui.duplicates.status_min_excludes").replace("{min}", formatBytes(tools.draft.minimumSize)).replace("{count}", String(tools.excludes("duplicate_scan").length))}</span>
          <strong>{tr("gui.duplicates.status_groups_reclaimable").replace("{groups}", tools.resultLabel(tools.result("duplicate_scan")?.duplicate_groups)).replace("{size}", tools.resultLabel(tools.result("duplicate_scan")?.reclaimable_bytes, true))}</strong>
        {:else if screen === "password"}
          <span>{tr("gui.screen.password")}</span>
          <span>{passwordPromptName()}</span>
          <span>{tr("gui.password.keychain_opt_in_short").replace("{secretStore}", secretStoreLabel())}</span>
          <strong>{tr("gui.password.no_plaintext_storage")}</strong>
	        {:else if screen === "recovery"}
	          <span>{tr("gui.recovery.status_par2_sidecar")}</span>
	          <span>{recoverySession.sourceName() ?? tr("gui.recovery.no_archive_selected")}</span>
	          <span>{recoverySession.resultAvailable() ? recoverySession.resultDetail() : tr("gui.recovery.verify_first")}</span>
	          <strong>{recoverySession.resultFooter()}</strong>
        {:else if screen === "archiveInfo"}
          <span>{tr("gui.screen.archive_info")}</span>
          <span>{currentArchive ? archiveTitle() : openArchiveFirstLabel()}</span>
          <span>{currentArchive ? archiveFormat() : "-"}</span>
          <strong>{extractView.extractDestinationHint()}</strong>
        {:else if classicArchiveStartVisible()}
          <span role="status" aria-live="polite">{navLabel("Archives")}</span>
          <span></span>
          <span></span>
          <strong>{currentTaskStatusLabel()}</strong>
        {:else}
          <span role="status" aria-live="polite">{currentArchive ? archiveFilterStatus() : noArchiveLabel()}</span>
          <span>{selectedSummary()}</span>
          <span>{currentArchive ? `${archiveFormat()} · ${currentArchive.volumes?.length ? archiveVolumeCountLabel(currentArchive.volumes.length) : tr("gui.archive.single_file")}` : openArchiveFirstLabel()}</span>
          <strong>{currentTaskStatusLabel()}</strong>
        {/if}
      </footer>
    </section>
  </main>
{/if}
