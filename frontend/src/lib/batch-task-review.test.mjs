import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions, selectMountCallbacks, selectVariableStatements } from "../../tests/source.mjs";
import { settingsDto } from "../../tests/settings.mjs";
import { installBatchExtractDraft } from "../../tests/options-drafts.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { taskReviewScreen } = await server.ssrLoadModule("/src/lib/task-model.ts");
const { BatchExtractDraft } = await server.ssrLoadModule("/src/lib/batch-extract.svelte.ts");
const { readBatchExtractResult } = await server.ssrLoadModule("/src/lib/batch-extract-result.ts");
const { desktopBasename, normalizeDesktopFolder } = await server.ssrLoadModule("/src/lib/desktop-path.ts");
const { SettingsSession } = await server.ssrLoadModule("/src/lib/settings-session.svelte.ts");
const archiveStore = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
const taskWindow = await server.ssrLoadModule("/src/lib/task-window.ts");
const { externalOpenAction } = await server.ssrLoadModule("/src/lib/external-tasks.ts");
const { sameDesktopPath } = await server.ssrLoadModule("/src/lib/desktop-path.ts");
const { archiveVolumeFamilyKeys } = await server.ssrLoadModule("/src/lib/archive-names.ts");

function spec() {
  return { kind: "batch_extract", overwrite: "rename", symlinks: "skip", smart: false,
    items: [
      { path: "/original/季度归档与设计资料/完整文件名.zip", dest: "/output/zip", encoding: "gbk", best_effort: true, password: "old-password" },
      { path: "/original/photos.7z", dest: "/output/photos", encoding: null, best_effort: false, password: null },
      { path: "/original/logs.tar", dest: "/another/logs", encoding: null, best_effort: false, password: null },
    ] };
}

const plain = (value) => JSON.parse(JSON.stringify(value));

function harness() {
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const names = ["cancelTaskReview", "reviewTask", "batchDraftLocked", "setBatchArchivePaths", "editBatchDraft",
    "removeBatchItem", "chooseBatchPaths", "startBatchExtract", "batchWorkspaceSurface", "setScreen", "dismissRecoveryPreparation",
    "normalizedDefaultExtractDir", "normalizedFolderSetting"];
  const declarations = selectFunctions(source, names);
  const calls = [];
  const context = {
    BatchExtractDraft, normalizeDesktopFolder, batchSubmissionPending: false, batchPickerBusy: false,
    batchReviewFocusPending: false, batchPickerRequest: 0,
    nestedExtractPickerBusy: false, nestedExtractPickerRequest: 0, nestedExtractDraftGeneration: 0,
    archiveUpdateReview: { cancelSourceChoice() {} }, taskReviewRequestGeneration: 0,
    dismissArchivePicker() {}, clearEntryPreviewState() {}, syncUrl() {},
    screen: "browse", archiveOpenStatus: "idle", taskWindowMode: false, runtimePreviews: {batchPaths:[]},
    recoveryPickerStatus: "idle", recoveryPickerRequest: 0, recoveryOutputPreparation: null,
    dismissCreatePreparation() {}, syncCreatePreflightContext() {},
    dismissArchiveAddPreparation() {},
    currentArchive: {source:"/unrelated/current.zip",path:"/unrelated/current.zip",encoding_override:"shift_jis"},
    pathBaseName: (path) => desktopBasename(path,"macos"),
    archiveFormatFromPath: (path) => path.split(".").at(-1).toUpperCase(),
    platformKind: () => "macos",
    tr: (_key, fallback) => fallback, taskReviewScreen, readBatchExtractResult,
    preventCreateSubmissionNavigation: () => false, preventConvertSubmissionNavigation: () => false,
    focusBlockingTaskIfAny: () => false,
    dismissTaskDialog: async () => { calls.push(["dismiss"]); },
    focusBatchReview: () => { calls.push(["focus"]); },
    showNotice: (message) => calls.push(["notice",message]), recordOperation() {},
    document: {documentElement:{},body:{},querySelectorAll:()=>[],getElementById:(id) => id.endsWith("--1") ? null : ({focus:() => calls.push(["focus-field",id])})},
    tick: async () => {},
    submitJob: async (job) => { calls.push(["submit",plain(job)]); return 99; },
    isJobSubmitBlocked: () => false, isErrorDto: () => false, tError: () => "error",
    toolsArchiveReturnSurface: () => ({visible:false}),
    extractOverwriteModes: ["ask","skip","overwrite","rename"], extractOverwriteLabel: (mode) => mode,
    extractSymlinkModes: ["preserve","skip","follow"], extractSymlinkLabel: (mode) => mode,
    getDialogModule: async () => ({open:async () => null}),
    openNativeDialog: async (_key,open,options) => open(options),
  };
  context.settingsSession = new SettingsSession({ platform: context.platformKind, tr: context.tr, emit() {} });
  context.settingsSession.applySnapshot(settingsDto(), context.settingsSession.captureGenerations());
  installBatchExtractDraft(source, context);
  const outputText = compileTestScript(declarations.map((node) => node.getText(source)).join("\n"));
  return {...vm.runInNewContext(`${outputText}\n({${names.join(",")}})`,context),context,calls};
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const settle = () => new Promise((resolve) => setImmediate(resolve));
const incoming = (paths, action = null) => ({ paths, action, output: null });

function startupHarness({ forceFirstRun = false, taskWindowMode = false, batchPaths = null } = {}) {
  const run = harness();
  const { context, calls } = run;
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const names = ["captureIncomingOpenFilesRequest", "handleOpenFilesPayload", "openFirstArchivePath",
    "openArchivePath", "finishOpenedArchive", "dismissArchivePicker", "dismissArchivePasswordRequest",
    "isPar2Path", "preferredRecoverySidecar", "recoverySidecarSetKey", "openRecoverySetFromPaths", "openRecoverySet",
    "sameFilePath", "submitExternalTaskWindow", "submitJob", "isThemeChoice", "isDensityChoice", "applyAppearanceSettingsSnapshot"];
  const declarations = selectFunctions(source, names);
  const variables = selectVariableStatements(source, ["resolveInitialGeneralSettingsReady", "initialGeneralSettingsReady", "incomingOpenFilesGeneration"]);
  const mounts = selectMountCallbacks(source, ["ipc.takeOpenFiles", "ipc.getSettings"]);
  const modeDeclaration = selectVariableStatements(source, ["taskWindowMode"])[0].declarationList.declarations
    .find((node) => node.name.getText(source) === "taskWindowMode");
  const originalIpc = { ...ipc };
  const take = deferred(), settings = deferred(), locale = deferred(), localeEntered = deferred();
  const listenerRegistered = deferred(), queueEntered = deferred(), queue = deferred();
  let listener;
  let archiveId = 0;
  Object.assign(ipc, {
    takeOpenFiles: () => { calls.push(["take"]); return take.promise; },
    getSettings: () => { calls.push(["settings"]); return settings.promise; },
    getLocaleTable: (language) => { calls.push(["locale", language]); localeEntered.resolve(language); return locale.promise; },
    openArchive: async (path) => {
      calls.push(["open-native", path]);
      return { id: ++archiveId, path, source: path, name: desktopBasename(path, "macos"), format: "zip",
        entry_count: 0, volumes: null, non_utf8_name_count: 0, garbled_count: 0,
        suggested_encoding: null, encoding_override: null, read_only: true };
    },
    listEntries: async (_id, page) => ({ page, total: 0, items: [] }),
    closeArchive: async () => {}, cancelArchiveOpen: async () => {},
    openFileListenerReady: () => { calls.push(["queue-ready"]); queueEntered.resolve(); return queue.promise; },
    resolveExternalTaskJob: async (action, paths, output) => {
      calls.push(["resolve-external", action, Array.from(paths), output]);
      return paths.length ? { kind: "extract", path: paths[0], dest: "/native/external" } : null;
    },
  });
  archiveStore.closeArchive();
  context.settingsSession = new SettingsSession({ platform: context.platformKind, tr: context.tr, emit() {} });
  Object.defineProperty(context, "currentArchive", { get: archiveStore.archive });
  Object.defineProperty(context, "archivePasswordPrompt", { get: archiveStore.openPasswordPrompt });
  Object.assign(context, {
    ...taskWindow, ipc, loadLocale, externalOpenAction, sameDesktopPath, archiveVolumeFamilyKeys,
    taskWindowLaunchState: taskWindow.taskWindowLaunchStateFromParams(new URLSearchParams(taskWindowMode ? "taskWindow=1" : "")),
    forceFirstRun, initialMode: "modern", initialThemeChoice: "system", activePlatform: "macos",
    appearanceSaveGenerations: { mode: 0, theme: 0, density: 0 }, hasDensityOverride: false,
    activeDensityChoice: "standard", settingsStatus: "loading", sourceCleanupRecoveryReady: false,
    storedPreviewLanguage: () => "en-US", startAutomaticUpdateCheck: (enabled) => calls.push(["updates", enabled]),
    archiveOpenGeneration: 0, archivePasswordAttempt: 0, archivePickerRequest: null,
    previewPasswordPrompt: null, openPasswordPrompt: archiveStore.openPasswordPrompt,
    archiveOpenError: archiveStore.archiveOpenError, openArchiveStore: archiveStore.openArchive,
    cancelPendingArchiveOpen: archiveStore.cancelPendingArchiveOpen,
    cancelArchivePasswordPrompt: archiveStore.cancelPasswordPrompt,
    rememberRecent: (path) => calls.push(["recent", path]), recordValidationRenderReady() {},
    taskDialogTaskId: null, taskDialogDismissedId: null, jobSubmitInFlight: false, submittingJobSpec: null,
    checksumAlgorithm: "sha256", checksumExcludeRules: () => [],
    rememberWindowRecovery: (recovery) => calls.push(["window-recovery", plain(recovery)]),
    submitArchiveJob: async (job) => { calls.push(["submit", plain(job)]); return 99; },
    setTimeout, clearTimeout,
    currentWebviewWindowListener: async () => async (event, receive) => {
      calls.push(["listen", event]); listener = receive; listenerRegistered.resolve();
      return () => { calls.push(["unlisten"]); };
    },
  });
  const vmContext = vm.createContext(context);
  Object.defineProperty(context, "taskWindowMode", {
    get: vm.runInContext(compileTestScript(`() => (${modeDeclaration.initializer.arguments[0].getText(source)})`), vmContext),
  });
  const code = [...variables, ...declarations].map((node) => node.getText(source)).join("\n");
  vm.runInContext(compileTestScript(code.replaceAll("import.meta.env.DEV", "false")), vmContext);
  const callbacks = vm.runInContext(compileTestScript(`[${mounts.map((node) => node.getText(source)).join(",")}];`), vmContext);
  if (batchPaths) run.setBatchArchivePaths(batchPaths);
  const cleanups = callbacks.map((mount) => mount());
  let disposed = false;
  const cleanup = () => { if (!disposed) { disposed = true; for (const release of cleanups) release?.(); } };
  return { ...run, take, settings, locale, localeEntered, listenerRegistered, queueEntered, queue,
    handle: context.handleOpenFilesPayload, open: context.openArchivePath,
    emit: (payload) => { assert.ok(listener, "the actual mount registered its listener"); listener({ payload }); },
    cleanup,
    dispose: async () => {
      cleanup();
      take.resolve(incoming([])); settings.resolve(settingsDto()); queue.resolve(incoming([]));
      locale.resolve({ lang: "en-US", table: {} });
      await settle(); archiveStore.closeArchive(); Object.assign(ipc, originalIpc);
    },
  };
}

async function withStartup(options, body) {
  const run = startupHarness(options);
  try { await body(run); } finally { await run.dispose(); }
}

test("partially successful batches expose a review action for failed archives", () => {
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 1, extracted: 2 } }), "batch");
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 0, extracted: 3 } }), null);
  assert.equal(taskReviewScreen({ state: "done", spec: { kind: "batch_extract" },
    result: { failed: 0, extracted: 3, outputs: [{ archive: "damaged.zip", counts: { failed: 1 } }] } }), "batch");
});

for (const state of ["failed", "cancelled"]) {
test(`${state} batch review restores original nonsecret settings; edits are exactly what is submitted`, async () => {
  const run = harness();
  const original = spec();
  await run.reviewTask({state,spec:original,result:null});
  original.items[0].dest = "/changed/task-input";
  assert.equal(run.context.batchDraft.items[0].dest, spec().items[0].dest, "review retains its own nonsecret item snapshot");
  assert.equal(run.context.screen,"batch");
  assert.equal(run.calls.some(([name]) => name === "submit"),false);
  let surface = run.batchWorkspaceSurface("modern");
  assert.deepEqual(Array.from(surface.rows,(row)=>row.target),spec().items.map((item)=>item.dest));
  assert.equal(surface.rows[0].encoding,"gbk");
  assert.equal(surface.rows[0].bestEffort,true);
  assert.equal(surface.overwrite,"rename");
  assert.equal(surface.symlinks,"skip");
  assert.equal(surface.smart,false);
  surface.rows[0].onTargetInput("/edited/完整目标");
  surface.onSmartChange(true);
  surface.onOverwriteChange("ask");
  surface.onSymlinksChange("preserve");
  surface = run.batchWorkspaceSurface("classic");
  const saved = plain(run.context.batchDraft);
  const snapshot = run.context.batchExtract.snapshotRun();
  snapshot.items[0].dest = "/changed/run-snapshot";
  snapshot.items.push({ ...snapshot.items[0], path: "/changed/extra.zip" });
  assert.deepEqual(plain(run.context.batchDraft), saved, "a run snapshot cannot change draft items or destinations");
  await run.startBatchExtract();
  const submitted = run.calls.find(([name])=>name==="submit")[1];
  assert.deepEqual(submitted.items.map((item)=>item.dest),Array.from(surface.rows,(row)=>row.target));
  assert.equal(submitted.items[0].dest,"/edited/完整目标");
  assert.equal(submitted.items[0].encoding,"gbk");
  assert.equal(submitted.items[0].best_effort,true);
  assert.equal(submitted.smart,true);
  assert.equal(submitted.overwrite,"ask");
  assert.equal(submitted.symlinks,"preserve");
  assert.ok(submitted.items.every((item)=>item.password===null));
  assert.equal("password" in run.context.batchDraft.items[0],false);
});
}

test("partial success restores only identified failures and ambiguous results preserve the current draft", async () => {
  const run = harness();
  const job=spec();
  await run.reviewTask({state:"done",spec:job,result:{failed:2,failures:[{archive:job.items[2].path},{archive:job.items[0].path}]}});
  assert.deepEqual(Array.from(run.context.batchDraft.items,(item)=>item.path),[job.items[0].path,job.items[2].path]);
  const saved=plain(run.context.batchDraft);
  const revision = run.context.batchExtract.revision;
  for (const failures of [[],[{archive:"/unknown.zip"}],[{archive:job.items[0].path},{archive:job.items[0].path}]]) {
    await run.reviewTask({state:"done",spec:job,result:{failed:2,failures}});
    assert.deepEqual(plain(run.context.batchDraft),saved);
    assert.match(run.calls.at(-1)[1],/cannot identify/);
    assert.equal(run.context.batchExtract.revision, revision, "ambiguous reports do not invalidate an unchanged draft");
  }
  for (const [task, displayed] of [
    [{ state: "done", spec: job, result: { failed: 1, failures: [{ archive: "/unknown.zip" }] } }, job],
    [{ state: "failed", spec: job, result: null }, { ...job, items: job.items.slice(1) }],
  ]) {
    await run.reviewTask(task, displayed);
    assert.deepEqual(plain(run.context.batchDraft), saved);
    assert.equal(run.context.batchExtract.revision, revision);
  }
  const duplicate={...job,items:[job.items[0],{...job.items[0],dest:"/different"}]};
  assert.equal(run.context.batchExtract.restoreTask(duplicate, [{ archive: job.items[0].path }], 1, duplicate), false);
  assert.deepEqual(plain(run.context.batchDraft), saved);
  assert.equal(run.context.batchExtract.revision, revision);
  const windows={...job,items:[{...job.items[0],path:"C:\\Sources\\A.zip"}]};
  const windowsReview = harness();
  windowsReview.context.platformKind = () => "windows";
  assert.equal(windowsReview.context.batchExtract.restoreTask(windows, [{ archive: "c:/sources/a.zip" }], 1, windows), true);
  assert.equal(windowsReview.context.batchDraft.items.length, 1);
  const windowsDraft = plain(windowsReview.context.batchDraft);
  const windowsRevision = windowsReview.context.batchExtract.revision;
  windowsReview.context.platformKind = () => "linux";
  assert.equal(windowsReview.context.batchExtract.restoreTask(windows, [{ archive: "c:/sources/a.zip" }], 1, windows), false);
  assert.deepEqual(plain(windowsReview.context.batchDraft), windowsDraft);
  assert.equal(windowsReview.context.batchExtract.revision, windowsRevision);
  assert.equal(run.calls.some(([name])=>name==="submit"),false);
});

test("best-effort batches restore only archives with failed entries or archive failures", async () => {
  const run = harness();
  const job = spec();
  const result = { failed: 1, failures: [{ archive: job.items[2].path }], outputs: [
    { archive: job.items[0].path, counts: { failed: 2, skipped: 1 } },
    { archive: job.items[1].path, counts: { failed: 0, skipped: 1 } },
  ] };
  await run.reviewTask({ state: "done", spec: job, result });
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => item.path), [job.items[0].path, job.items[2].path]);
  assert.equal(run.calls.some(([name]) => name === "submit"), false);
  assert.match(run.calls.at(-1)[1], /from the beginning/);
  await run.startBatchExtract();
  const submitted = run.calls.find(([name]) => name === "submit")[1];
  assert.deepEqual(submitted.items.map((item) => item.path), [job.items[0].path, job.items[2].path]);
  assert.ok(submitted.items.every((item) => item.password === null));
  const saved = plain(run.context.batchDraft);
  await run.reviewTask({ state: "done", spec: job, result: { failed: 0, outputs: [
    { archive: job.items[0].path, counts: { failed: 1 } },
    { archive: job.items[0].path, counts: { failed: 1 } },
  ] } });
  assert.deepEqual(plain(run.context.batchDraft), saved);
  assert.match(run.calls.at(-1)[1], /cannot identify/);
});

test("new batches use the displayed smart base and an explicitly empty list never falls back to the current archive", async () => {
  const run=harness();
  run.context.screen="batch";
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => [item.path, item.dest, item.encoding]),
    [["/unrelated/current.zip", "/unrelated", "shift_jis"]]);
  run.context.runtimePreviews.batchPaths = ["/preview/backup.zip"];
  run.context.settingsSession.applySnapshot(settingsDto({ default_extract_dir: "/applied/fallback" }),
    run.context.settingsSession.captureGenerations());
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => [item.path, item.dest, item.encoding]),
    [["/preview/backup.zip", "/applied/fallback", null]], "fallback remains live until a business edit is accepted");
  assert.equal(run.context.batchExtract.revision, 0, "reading fallback does not initialize a saved draft");
  run.context.getDialogModule = async () => ({ open: async () => [] });
  await run.chooseBatchPaths(0);
  run.context.runtimePreviews.batchPaths = [];
  run.context.currentArchive = { source: "/later/current.zip", path: "/later/current.zip", encoding_override: "gbk" };
  run.context.settingsSession.applySnapshot(settingsDto(), run.context.settingsSession.captureGenerations());
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => [item.path, item.dest, item.encoding]),
    [["/later/current.zip", "/later", "gbk"]]);
  run.setBatchArchivePaths(["/inbox/backup.zip","/other/photos.7z"]);
  assert.deepEqual(Array.from(run.context.batchDraft.items,(item)=>item.dest),["/inbox","/other"]);
  run.context.settingsSession.applySnapshot(settingsDto({ default_extract_dir: "/preferred" }),
    run.context.settingsSession.captureGenerations());
  run.context.settingsSession.setGeneral("defaultExtractDir", "/not-applied");
  run.setBatchArchivePaths(["/inbox/backup.zip"]);
  assert.equal(run.context.batchDraft.items[0].dest,"/preferred");
  run.removeBatchItem(0);
  run.context.runtimePreviews.batchPaths = ["/preview/must-not-return.zip"];
  await run.startBatchExtract();
  assert.equal(run.context.batchDraft.items.length,0);
  assert.equal(run.calls.some(([name])=>name==="submit"),false);
  assert.ok(run.calls.some(([name,id])=>name==="focus-field"&&id==="batch-workspace-heading"));
  const paths = ["/inbox/backup.zip", "/other/photos.zip"];
  for (const order of ["take-first", "settings-first"]) {
    await withStartup({}, async (startup) => {
      const { context, calls } = startup;
      if (order === "take-first") {
        startup.take.resolve(incoming(paths)); await settle();
        assert.equal(context.batchExtract.revision, 0, "initial sources wait before materializing a batch");
        assert.equal(calls.some(([kind]) => kind === "open-native"), false);
        assert.equal(context.currentArchive, null);
      }
      context.settingsSession.setGeneral("defaultExtractDir", "/newer/settings-draft");
      startup.settings.resolve(settingsDto({ default_extract_dir: "/Exports/Default", language: "zh-CN" }));
      assert.equal(await startup.localeEntered.promise, "zh-CN");
      if (order === "settings-first") {
        assert.equal(context.batchExtract.revision, 0);
        assert.equal(calls.some(([kind]) => kind === "open-native"), false);
        startup.take.resolve(incoming(paths));
      }
      await settle();
      assert.equal(context.settingsSession.appliedGeneral.defaultExtractDir, "/Exports/Default");
      assert.equal(context.settingsSession.general.defaultExtractDir, "/newer/settings-draft");
      assert.equal(context.currentArchive.source, paths[0]);
      assert.deepEqual(Array.from(context.batchDraft.items, (item) => [item.path, item.dest]),
        paths.map((path) => [path, "/Exports/Default"]));
      assert.equal(context.batchExtract.revision, 1);
      assert.equal(calls.filter(([kind]) => kind === "settings").length, 1);
      assert.equal(context.settingsStatus, "loading", "real locale loading is still pending when the archive is admitted");
      assert.equal(context.sourceCleanupRecoveryReady, false);
      assert.equal(calls.some(([kind]) => kind === "updates"), false);
      startup.locale.resolve({ lang: "zh-CN", table: {} }); await settle();
      assert.equal(context.settingsStatus, "preview");
      assert.equal(context.sourceCleanupRecoveryReady, true);
    });
  }
  for (const forceFirstRun of [false, true]) {
    await withStartup({ forceFirstRun }, async (startup) => {
      startup.take.resolve(incoming(paths)); await settle();
      if (!forceFirstRun) {
        assert.equal(startup.context.batchExtract.revision, 0);
        assert.equal(startup.calls.some(([kind]) => kind === "open-native"), false);
        startup.settings.reject(new Error("settings unavailable"));
      }
      assert.equal(await startup.localeEntered.promise, forceFirstRun ? null : "en-US");
      await settle();
      assert.equal(startup.context.currentArchive.source, paths[0]);
      assert.equal(startup.context.settingsSession.appliedGeneral.defaultExtractDir, "");
      assert.deepEqual(Array.from(startup.context.batchDraft.items, (item) => item.dest), ["/inbox", "/other"]);
      assert.equal(startup.context.batchExtract.revision, 1);
      assert.equal(startup.calls.filter(([kind]) => kind === "settings").length, forceFirstRun ? 0 : 1);
      assert.equal(startup.context.sourceCleanupRecoveryReady, false, "fallback admission also precedes locale completion");
    });
  }
});

test("batch review matches displayed failures while submitting the original opaque source", async () => {
  const run=harness();
  const displayed=spec();
  const actual={...displayed,items:displayed.items.map((item,index)=>({...item,path:`squallz-archive://${index+1}`}))};
  await run.reviewTask({state:"done",spec:actual,result:{failed:1,failures:[{archive:displayed.items[0].path}]}},displayed);
  const surface=run.batchWorkspaceSurface("modern");
  assert.equal(surface.rows.length,1);
  assert.equal(surface.rows[0].path,displayed.items[0].path);
  assert.equal(surface.rows[0].name,"完整文件名.zip");
  await run.startBatchExtract();
  const submitted=run.calls.find(([name])=>name==="submit")[1];
  assert.equal(submitted.items[0].path,"squallz-archive://1");
  assert.equal("displayPath" in submitted.items[0],false);
  run.context.currentArchive={source:"squallz-archive://8",path:"/Archives/outer.zip › inner.zip"};
  run.context.settingsSession.applySnapshot(settingsDto(), run.context.settingsSession.captureGenerations());
  run.setBatchArchivePaths([run.context.currentArchive.source]);
  assert.equal(run.context.batchDraft.items[0].dest,"/Archives");
  assert.equal(run.batchWorkspaceSurface("classic").rows[0].path,run.context.currentArchive.path);
  run.context.platformKind = () => "windows";
  run.context.currentArchive = { source: "C:\\Archives\\A.zip", path: "C:\\Displayed\\A.zip", encoding_override: "shift_jis" };
  run.setBatchArchivePaths(["c:/archives/a.zip"]);
  assert.deepEqual(Array.from(run.context.batchDraft.items, (item) => [item.path, item.displayPath, item.dest, item.encoding]),
    [["c:/archives/a.zip", "c:/archives/a.zip", "c:/archives", "shift_jis"]],
    "display identity remains exact while encoding uses platform path equivalence");
});

test("submission locks a batch against edits, duplicate submits and incoming selections; errors preserve its draft", async () => {
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  const saved=plain(run.context.batchDraft);
  const revision = run.context.batchExtract.revision;
  let reject;
  run.context.submitJob=(job)=>{run.calls.push(["submit",plain(job)]);return new Promise((_,fail)=>{reject=fail;});};
  const pending=run.startBatchExtract();
  assert.equal(run.batchWorkspaceSurface("modern").locked,true);
  run.batchWorkspaceSurface("classic").rows[0].onTargetInput("/ignored");
  run.batchWorkspaceSurface("modern").onSmartChange(true);
  run.removeBatchItem(1);
  assert.equal(run.setBatchArchivePaths(["/ignored.zip"]),false);
  await run.startBatchExtract();
  await run.reviewTask({state:"failed",spec:{...spec(),items:[spec().items[1]]}});
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.equal(run.context.batchExtract.revision, revision, "busy rejection leaves the admission revision intact");
  assert.equal(run.calls.filter(([name])=>name==="submit").length,1);
  reject(new Error("not connected"));
  await pending;
  assert.equal(run.batchWorkspaceSurface("classic").locked,false);
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.match(run.calls.at(-1)[1],/desktop service/);
  run.batchWorkspaceSurface("modern").rows[1].onTargetInput("  ");
  await run.startBatchExtract();
  assert.deepEqual(run.calls.at(-1),["focus-field","batch-destination-1"]);
  assert.equal(run.calls.filter(([name])=>name==="submit").length,1);
});

test("native selection adds without duplication, changes the chosen destination and ignores stale responses", async () => {
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  run.context.getDialogModule=async()=>({open:async(options)=>options.directory?"/picked": [spec().items[0].path,"/new/new.zip"]});
  await run.chooseBatchPaths();
  assert.equal(run.context.batchDraft.items.length,4);
  await run.chooseBatchPaths(1);
  assert.equal(run.context.batchDraft.items[1].dest,"/picked");
  const saved=plain(run.context.batchDraft);
  let revision = run.context.batchExtract.revision;
  const surface = run.batchWorkspaceSurface("modern");
  surface.onOverwriteChange(surface.overwrite);
  assert.equal(run.context.batchExtract.revision, ++revision, "accepted same-value edits invalidate earlier draft captures");
  run.context.getDialogModule = async () => ({ open: async () => [spec().items[0].path] });
  await run.chooseBatchPaths();
  assert.equal(run.context.batchExtract.revision, ++revision, "accepted duplicate-only selection still advances revision");
  assert.deepEqual(plain(run.context.batchDraft), saved);
  run.context.getDialogModule = async () => ({ open: async () => [] });
  for (const index of [null, 1]) {
    await run.chooseBatchPaths(index);
    assert.equal(run.context.batchExtract.revision, ++revision, "an accepted empty native selection advances revision");
    assert.deepEqual(plain(run.context.batchDraft), saved);
  }
  run.context.getDialogModule=async()=>({open:async()=>null});
  await run.chooseBatchPaths();
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.match(run.calls.at(-1)[1],/cancelled/);
  assert.equal(run.context.batchExtract.revision, revision, "cancellation preserves the draft revision");
  let finish;
  run.context.getDialogModule=async()=>({open:()=>new Promise((resolve)=>{finish=resolve;})});
  const pending=run.chooseBatchPaths();
  await new Promise((resolve)=>setImmediate(resolve));
  run.context.screen="browse";
  finish(["/stale.zip"]);
  await pending;
  assert.deepEqual(plain(run.context.batchDraft),saved);
  assert.equal(run.context.batchPickerBusy,false);
});

test("leaving batch review abandons loading and open choosers without unlocking a newer choice", async () => {
  for (const index of [null, 1]) {
    for (const phase of ["loading", "choosing"]) {
      for (const outcome of ["selected", "cancelled", "failed"]) {
        const run = harness(); await run.reviewTask({state:"failed",spec:spec()});
        const saved = plain(run.context.batchDraft);
        let finish, fail, started;
        const ready = new Promise(resolve => { started = resolve; });
        const oldResult = new Promise((resolve, reject) => { finish = resolve; fail = reject; });
        let opened = 0;
        run.context.getDialogModule = phase === "loading" ? () => { started(); return oldResult; }
          : async () => ({open:async()=>null});
        run.context.openNativeDialog = () => { opened++; started(); return oldResult; };
        const oldChoice = run.chooseBatchPaths(index); await ready;
        assert.equal(run.context.batchPickerBusy, true);
        run.setScreen("settingsGeneral"); run.setScreen("batch");
        assert.equal(run.context.batchPickerBusy, false, "navigation must release the abandoned chooser");
        let completeNew;
        run.context.getDialogModule = async () => ({open:async()=>null});
        run.context.openNativeDialog = () => { opened++; return new Promise(resolve => { completeNew = resolve; }); };
        const newChoice = run.chooseBatchPaths(index);
        await new Promise(resolve => setImmediate(resolve));
        const noticeCount = run.calls.filter(([kind]) => kind === "notice").length;
        if (outcome === "failed") fail(new Error("late dialog failure"));
        else finish(phase === "loading" ? {open:async()=>"/stale"} : outcome === "cancelled" ? null : ["/stale"]);
        await oldChoice;
        assert.equal(opened, phase === "loading" ? 1 : 2, "abandoned module loads must not open a dialog");
        assert.deepEqual(plain(run.context.batchDraft), saved);
        assert.equal(run.context.batchPickerBusy, true, "old completion must not unlock the new chooser");
        assert.equal(run.calls.filter(([kind]) => kind === "notice").length, noticeCount);
        run.context.settingsSession.applySnapshot(settingsDto({ default_extract_dir: "/applied/while-choosing" }),
          run.context.settingsSession.captureGenerations());
        completeNew(index === null ? ["/new/archive.zip"] : "/new/destination"); await newChoice;
        assert.equal(run.context.batchPickerBusy, false);
        if (index === null) {
          assert.equal(run.context.batchDraft.items.at(-1).path, "/new/archive.zip");
          assert.equal(run.context.batchDraft.items.at(-1).dest, "/applied/while-choosing", "new source defaults are read at native acceptance");
        } else assert.equal(run.context.batchDraft.items[index].dest, "/new/destination");
      }
    }
  }
  for (const phase of ["take-pending", "general-pending"]) {
    for (const change of ["navigation", "navigation-aba", "source", "archive", "destination", "empty"]) {
      await withStartup({ batchPaths: ["/original/keep.zip"] }, async (startup) => {
        if (phase === "general-pending") {
          startup.take.resolve(incoming(["/stale/input.zip"])); await settle();
          assert.equal(startup.calls.some(([kind]) => kind === "open-native"), false);
        }
        if (change === "navigation" || change === "navigation-aba") {
          startup.setScreen("settingsGeneral");
          if (change === "navigation-aba") startup.setScreen("browse");
        } else if (change === "source") startup.setBatchArchivePaths(["/newer/source.zip"]);
        else if (change === "archive") await startup.open("/newer/current.zip", "dialog");
        else if (change === "destination") startup.batchWorkspaceSurface("modern").rows[0].onTargetInput("/manual/keep");
        else startup.removeBatchItem(0);
        const saved = plain(startup.context.batchDraft);
        const revision = startup.context.batchExtract.revision;
        const screen = startup.context.screen;
        const notices = startup.calls.filter(([kind]) => kind === "notice").length;
        if (phase === "take-pending") startup.take.resolve(incoming(["/stale/input.zip"]));
        startup.settings.resolve(settingsDto({ default_extract_dir: "/Exports/Default" }));
        await startup.localeEntered.promise; await settle();
        assert.deepEqual(plain(startup.context.batchDraft), saved, `${phase}/${change} keeps newer draft state`);
        assert.equal(startup.context.batchExtract.revision, revision);
        assert.equal(startup.context.screen, screen);
        assert.equal(startup.context.currentArchive?.source ?? null, change === "archive" ? "/newer/current.zip" : null);
        assert.deepEqual(startup.calls.filter(([kind]) => kind === "open-native").map(([, path]) => path),
          change === "archive" ? ["/newer/current.zip"] : []);
        assert.equal(startup.calls.filter(([kind]) => kind === "notice").length, notices);
      });
    }
  }
  await withStartup({}, async (startup) => {
    startup.take.resolve(incoming([]));
    await startup.listenerRegistered.promise;
    await startup.queueEntered.promise;
    assert.equal(startup.calls.filter(([kind]) => kind === "queue-ready").length, 1);
    startup.emit(incoming(["/older/first.zip"]));
    startup.emit(incoming(["/newer/last.zip", "/newer/second.zip"]));
    startup.queue.resolve(incoming([])); await settle();
    assert.equal(startup.context.batchExtract.revision, 0);
    assert.equal(startup.calls.some(([kind]) => kind === "open-native"), false);
    startup.settings.resolve(settingsDto({ default_extract_dir: "/Exports/Default" }));
    await startup.localeEntered.promise; await settle();
    assert.deepEqual(startup.calls.filter(([kind]) => kind === "open-native").map(([, path]) => path), ["/newer/last.zip"]);
    assert.deepEqual(Array.from(startup.context.batchDraft.items, (item) => [item.path, item.dest]),
      [["/newer/last.zip", "/Exports/Default"], ["/newer/second.zip", "/Exports/Default"]],
      "an empty native queue drain cannot supersede the newest waiting event");
    startup.cleanup();
    assert.equal(startup.calls.filter(([kind]) => kind === "unlisten").length, 1);
  });
  for (const route of ["external", "external-empty", "recovery"]) {
    await withStartup({ batchPaths: ["/original/keep.zip"] }, async (startup) => {
      startup.take.resolve(incoming(["/stale/input.zip"])); await settle();
      const saved = plain(startup.context.batchDraft);
      const latest = route === "recovery" ? incoming(["/recovery/keep.par2"])
        : incoming(route === "external-empty" ? [] : ["/external/new.zip"], "extract-here");
      await startup.handle(latest);
      assert.equal(startup.context.settingsSession.appliedGeneral.defaultExtractDir, "");
      if (route === "recovery") {
        assert.equal(startup.context.screen, "recovery");
        assert.equal(startup.context.recoveryPar2Override, "/recovery/keep.par2");
      } else {
        assert.equal(startup.context.taskWindowMode, true);
        assert.equal(startup.context.taskWindowLaunchState.status, route === "external-empty" ? "no-selection" : "started");
        assert.equal(startup.calls.filter(([kind]) => kind === "submit").length, route === "external-empty" ? 0 : 1);
        assert.equal(startup.context.taskDialogTaskId, route === "external-empty" ? null : 99);
      }
      startup.settings.resolve(settingsDto({ default_extract_dir: "/Exports/Default" }));
      await startup.localeEntered.promise; await settle();
      assert.equal(startup.calls.some(([kind]) => kind === "open-native"), false);
      assert.deepEqual(plain(startup.context.batchDraft), saved, "immediate routes supersede an older ordinary source wait");
    });
  }
  await withStartup({ taskWindowMode: true }, async (startup) => {
    assert.equal(startup.calls.some(([kind]) => kind === "take"), false);
    await startup.handle(incoming(["/external/task.zip"], "extract-here"));
    assert.equal(startup.context.taskDialogTaskId, 99);
    assert.deepEqual(startup.calls.filter(([kind]) => kind === "submit").map(([, job]) => job),
      [{ kind: "extract", path: "/external/task.zip", dest: "/native/external" }]);
    assert.equal(startup.calls.some(([kind]) => kind === "queue-ready" || kind === "listen"), false);
    assert.equal(startup.calls.filter(([kind]) => kind === "settings").length, 1);
  });
  for (const change of ["navigation", "destination"]) {
    await withStartup({ batchPaths: ["/original/keep.zip"] }, async (startup) => {
      if (change === "navigation") startup.setScreen("settingsGeneral");
      else startup.batchWorkspaceSurface("classic").rows[0].onTargetInput("/manual/keep");
      const saved = plain(startup.context.batchDraft);
      startup.take.resolve(incoming(["/external/take.zip"], "extract-here")); await settle();
      assert.equal(startup.context.taskWindowLaunchState.status, "started", "late external take retains its independent task route");
      assert.equal(startup.context.taskDialogTaskId, 99);
      assert.equal(startup.calls.filter(([kind]) => kind === "submit").length, 1);
      assert.equal(startup.context.settingsSession.appliedGeneral.defaultExtractDir, "");
      assert.deepEqual(plain(startup.context.batchDraft), saved);
    });
  }
  for (const phase of ["take-pending", "general-pending", "queue-pending"]) {
    await withStartup({}, async (startup) => {
      if (phase === "general-pending") {
        startup.take.resolve(incoming(["/disposed/input.zip"])); await settle();
      } else if (phase === "queue-pending") {
        startup.take.resolve(incoming([])); await startup.listenerRegistered.promise; await startup.queueEntered.promise;
      }
      startup.cleanup();
      startup.take.resolve(incoming(["/disposed/input.zip"]));
      startup.settings.resolve(settingsDto({ default_extract_dir: "/disposed/settings" }));
      startup.queue.resolve(incoming(["/disposed/queued.zip"]));
      if (phase === "queue-pending") startup.emit(incoming(["/disposed/event.zip"]));
      await settle();
      assert.equal(startup.context.currentArchive, null);
      assert.equal(startup.context.batchExtract.revision, 0);
      assert.equal(startup.context.settingsSession.appliedGeneral.defaultExtractDir, "");
      assert.equal(startup.calls.some(([kind]) => kind === "open-native" || kind === "locale" || kind === "updates"), false);
      assert.equal(startup.calls.filter(([kind]) => kind === "unlisten").length, phase === "queue-pending" ? 1 : 0);
    });
  }
});

test("both layouts show editable targets and honest checks in both languages without dropping full source paths", async () => {
  const {render}=await server.ssrLoadModule("svelte/server");
  const {default:ToolsWorkspace}=await server.ssrLoadModule("/src/components/ToolsWorkspace.svelte");
  const {loadLocale,tFallback}=await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
  const run=harness();
  await run.reviewTask({state:"failed",spec:spec()});
  for(const locale of ["en-US","zh-CN"]) {
    await loadLocale(locale);run.context.tr=tFallback;
    for(const variant of ["modern","classic"]) {
      const surface=run.batchWorkspaceSurface(variant);
      const {body}=render(ToolsWorkspace,{props:{surface}});
      assert.ok(body.includes(spec().items[0].path));
      assert.ok(body.includes('value="/output/zip"'));
      assert.ok(body.includes(tFallback("gui.batch.execution_hint")));
      assert.ok(body.includes(tFallback("gui.batch.add_archives")));
      assert.doesNotMatch(body,/gui\.batch\.|old-password|100%|Ready to start|batch-readiness/);
    }
  }
});
