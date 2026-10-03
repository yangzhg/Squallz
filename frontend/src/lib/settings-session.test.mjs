import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";
import { settingsDto as settings } from "../../tests/settings.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { SettingsSession } = await server.ssrLoadModule("/src/lib/settings-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { loadLocale, currentLang, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
const originalIpc = { ...ipc };
test.beforeEach(async () => {
  Object.assign(ipc, originalIpc);
  ipc.getLocaleTable = async (lang) => ({ lang: lang ?? "en-US", table: {} });
  await loadLocale("en-US");
});
test.afterEach(() => Object.assign(ipc, originalIpc));

function deferred() {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
}

function harness(initial = {}) {
  const effects = [];
  const session = new SettingsSession({
    platform: () => "macos", tr: tFallback, emit: (effect) => effects.push(effect),
  }, initial);
  return {
    session, effects,
    seed: (snapshot = settings({ language: "en-US" })) => {
      session.applySnapshot(snapshot, session.captureGenerations());
      effects.length = 0;
    },
  };
}

const persistenceError = { key: "error.settings_write", params: {}, detail: "write rejected" };

test("a late settings load updates saved and applied values without replacing edited drafts", async () => {
  const { session, effects } = harness();
  const response = deferred();
  const requested = session.captureGenerations();
  ipc.getSettings = () => response.promise;
  const loading = ipc.getSettings().then((snapshot) => session.applySnapshot(snapshot, requested));
  session.setGeneral("defaultCreateDir", " /Draft Create/ ");
  session.setSecurity("maxEntries", 17);
  session.setPerformance("threads", 4);
  session.updateCustomAccent("#aabbcc", "hex");
  response.resolve(settings({ default_create_dir: "/Saved Create", default_extract_dir: "/Saved Extract",
    safety_max_entries: 901, performance_threads: 8, accent_palette: "custom", custom_accent: "#112233" }));
  await loading;

  assert.equal(session.general.defaultCreateDir, " /Draft Create/ ");
  assert.equal(session.savedGeneral.defaultCreateDir, "/Saved Create");
  assert.equal(session.appliedGeneral.defaultCreateDir, "/Saved Create");
  assert.equal(session.appliedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.security.maxEntries, 17);
  assert.equal(session.performance.threads, 4);
  assert.equal(session.colors.input, "#AABBCC");
  assert.equal(session.generalSaveState, "dirty");
  assert.match(session.snapshotLabel, /8 encoder threads/);
  session.setSecurity("maxEntries", 901);
  session.setPerformance("threads", 8);
  session.updateCustomAccent("#112233", "hex");
  assert.equal(session.securityDirty, false);
  assert.equal(session.performanceDirty, false);
  assert.equal(session.colorsDirty, false);
  assert.ok(effects.some((effect) => effect.kind === "generalApplied"));
  const previewRequested = session.captureGenerations();
  session.setGeneral("language", "fr-FR");
  const effectsBeforePreview = effects.length;
  session.applyPreviewLanguage("zh-CN", previewRequested);
  assert.equal(session.general.language, "fr-FR");
  assert.equal(session.savedGeneral.language, "zh-CN");
  assert.equal(session.appliedGeneral.language, "zh-CN");
  assert.equal(session.appliedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.security.maxEntries, 901);
  assert.equal(session.performance.threads, 8);
  assert.equal(effects.length, effectsBeforePreview);

  const other = harness({ paletteOverride: "copper", defaultExtractDir: "/Launch Extract" });
  other.session.applyPreviewLanguage("en-US", other.session.captureGenerations());
  assert.equal(other.session.general.language, "en-US");
  assert.equal(other.session.appliedGeneral.defaultExtractDir, "/Launch Extract");
  assert.equal(other.session.performance.threads, null);
  other.seed(settings({ accent_palette: "custom" }));
  assert.equal(other.session.colors.palette, "copper");
  other.session.setPalette("sage");
  other.seed(settings({ accent_palette: "custom" }));
  assert.equal(other.session.colors.palette, "sage");
  other.session.setPalette("custom");
  assert.equal(other.session.colorsDirty, false);
  assert.equal(session.colors.palette, "custom");
  assert.equal(session.colors.accent, "#112233");
});

test("one save owns admission and a late response advances only its section baseline", async () => {
  const { session, seed, effects } = harness();
  seed();
  const response = deferred();
  const entered = deferred();
  const locale = deferred();
  const localeEntered = deferred();
  const calls = [];
  ipc.setGeneralOptions = (...args) => { calls.push(["general", ...args]); entered.resolve(); return response.promise; };
  ipc.getLocaleTable = (lang) => { localeEntered.resolve(lang); return locale.promise; };
  for (const [method, section] of [["setSafetyLimits", "security"], ["setPerformanceOptions", "performance"], ["setAccentPalette", "colors"]]) {
    ipc[method] = async (...args) => { calls.push([section, ...args]); return settings(); };
  }
  session.setGeneral("defaultCreateDir", "/Sending");
  const saving = session.saveGeneral();
  await entered.promise;
  assert.equal(session.saveTarget, "general");
  assert.equal(session.generalSaveState, "saving");
  session.setSecurity("maxEntries", 23);
  session.setPerformance("threads", 5);
  session.updateCustomAccent("#ABCDEF", "hex");
  await session.saveSecurity();
  await session.savePerformance();
  await session.saveColors();
  assert.equal(calls.length, 1);
  session.setGeneral("defaultCreateDir", "/Newer Draft");
  const saved = settings({ language: "en-US", default_create_dir: "/Sending", safety_max_entries: 999,
    performance_threads: 63, accent_palette: "custom", custom_accent: "#FF0000" });
  response.resolve(saved);
  assert.equal(await localeEntered.promise, "en-US");
  assert.equal(session.saveTarget, "general");
  assert.equal(effects.some((effect) => effect.kind === "generalSaved"), false);
  session.setGeneral("defaultExtractDir", "/Edited During Successful Locale");
  locale.resolve({ lang: "en-US", table: { "gui.settings.general.saved": "Saved after locale" } });
  await saving;

  assert.equal(session.saveTarget, null);
  assert.equal(session.general.defaultCreateDir, "/Newer Draft");
  assert.equal(session.general.defaultExtractDir, "/Edited During Successful Locale");
  assert.equal(session.savedGeneral.defaultCreateDir, "/Sending");
  assert.equal(session.appliedGeneral.defaultCreateDir, "/Sending");
  assert.equal(session.generalSaveState, "dirty");
  assert.equal(session.security.maxEntries, 23);
  assert.equal(session.performance.threads, 5);
  assert.equal(session.colors.input, "#ABCDEF");
  session.resetSecurity();
  session.resetPerformance();
  session.updateCustomAccent("#6E506F", "hex");
  session.setPalette("aqua");
  assert.equal(session.securityDirty, false);
  assert.equal(session.performanceDirty, false);
  assert.equal(session.colorsDirty, false);
  const savedEffects = effects.filter((effect) => effect.kind === "generalSaved");
  assert.equal(savedEffects.length, 1);
  assert.equal(savedEffects[0].settings, saved);
  assert.deepEqual(effects.filter((effect) => effect.kind !== "palettePreviewChanged").slice(-3)
    .map((effect) => effect.kind), ["generalSaved", "notice", "automaticUpdates"]);
  assert.ok(effects.some((effect) => effect.kind === "notice" && effect.message === "Saved after locale"));

  const performanceResponse = deferred();
  const performanceEntered = deferred();
  ipc.setPerformanceOptions = () => { performanceEntered.resolve(); return performanceResponse.promise; };
  session.setPerformance("threads", 8);
  const performanceSave = session.savePerformance();
  await performanceEntered.promise;
  session.setPerformance("threads", 16);
  performanceResponse.resolve(settings({ performance_threads: 8 }));
  await performanceSave;
  assert.equal(session.performance.threads, 16);
  assert.equal(session.performanceSaveState, "dirty");
  assert.match(session.snapshotLabel, /8 encoder threads/);
  session.setPerformance("threads", 8);
  assert.equal(session.performanceDirty, false);
  assert.equal(session.appliedGeneral.defaultCreateDir, "/Sending");
});

test("persistence errors retain saved values while General and Colors desktop failures stay session only", async () => {
  const { session, seed, effects } = harness();
  seed(settings({ language: "en-US", default_extract_dir: "/Saved Extract" }));
  session.setGeneral("language", "zh-CN");
  session.setGeneral("defaultExtractDir", "/Draft Extract");
  session.setGeneral("revealAfterExtract", true);
  session.setGeneral("automaticUpdateChecks", false);
  effects.length = 0;
  ipc.setGeneralOptions = async () => { throw persistenceError; };
  await session.saveGeneral();
  assert.equal(session.generalSaveState, "error");
  assert.equal(session.savedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.appliedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.general.defaultExtractDir, "/Draft Extract");
  assert.deepEqual(effects.map((effect) => effect.kind), ["notice"]);

  effects.length = 0;
  ipc.setGeneralOptions = async () => { throw new Error("desktop unavailable"); };
  await session.saveGeneral();
  assert.equal(session.generalSaveState, "session");
  assert.equal(session.savedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.appliedGeneral.defaultExtractDir, "/Draft Extract");
  assert.equal(session.appliedGeneral.revealAfterExtract, true);
  assert.equal(session.appliedGeneral.automaticUpdateChecks, false);
  assert.equal(session.savedGeneral.automaticUpdateChecks, true);
  assert.equal(currentLang(), "zh-CN");
  assert.equal(effects.some((effect) => effect.kind === "generalSaved"), false);
  assert.equal(effects.filter((effect) => effect.kind === "notice").length, 1);
  assert.deepEqual(effects.filter((effect) => effect.kind === "previewLanguage"),
    [{ kind: "previewLanguage", language: "zh-CN" }]);
  assert.deepEqual(effects.filter((effect) => effect.kind === "generalApplied"),
    [{ kind: "generalApplied", revealAfterExtract: true }]);
  assert.deepEqual(effects.filter((effect) => effect.kind === "automaticUpdates"),
    [{ kind: "automaticUpdates", enabled: false }]);

  session.updateCustomAccent("#336699", "hex");
  effects.length = 0;
  ipc.setAccentPalette = async () => { throw persistenceError; };
  await session.saveColors();
  assert.equal(session.colorsSaveState, "error");
  assert.equal(session.colors.accent, "#336699");
  assert.equal(effects.filter((effect) => effect.kind === "notice").length, 1);
  ipc.setAccentPalette = async () => { throw new Error("desktop unavailable"); };
  effects.length = 0;
  await session.saveColors();
  assert.equal(session.colorsSaveState, "session");
  assert.equal(session.colors.accent, "#336699");
  assert.equal(effects.filter((effect) => effect.kind === "notice").length, 1);
  session.updateCustomAccent("#6E506F", "hex");
  session.setPalette("aqua");
  assert.equal(session.colorsDirty, false);

  session.setSecurity("maxEntries", 23);
  session.setPerformance("threads", 4);
  ipc.setSafetyLimits = async () => { throw new Error("desktop unavailable"); };
  ipc.setPerformanceOptions = async () => { throw new Error("desktop unavailable"); };
  effects.length = 0;
  await session.saveSecurity();
  await session.savePerformance();
  assert.equal(session.securitySaveState, "error");
  assert.equal(session.performanceSaveState, "error");
  assert.deepEqual(effects.map((effect) => effect.kind), ["notice", "notice"]);
  assert.ok(effects.every((effect) => effect.message.includes("Could not apply")));
});

test("editing during General fallback locale loading restores the previous applied language", async () => {
  const { session, seed, effects } = harness();
  seed(settings({ language: "en-US", default_extract_dir: "/Saved Extract" }));
  session.setGeneral("language", "zh-CN");
  session.setGeneral("defaultExtractDir", "/Pending Extract");
  const response = deferred();
  const entered = deferred();
  const locales = [];
  ipc.setGeneralOptions = async () => { throw new Error("desktop unavailable"); };
  ipc.getLocaleTable = (lang) => {
    locales.push(lang);
    if (lang === "zh-CN") { entered.resolve(); return response.promise; }
    return Promise.resolve({ lang, table: {} });
  };
  const saving = session.saveGeneral();
  await entered.promise;
  assert.equal(session.saveTarget, "general");
  session.setGeneral("defaultExtractDir", "/Edited During Locale");
  response.resolve({ lang: "zh-CN", table: {} });
  await saving;

  assert.deepEqual(locales, ["zh-CN", "en-US"]);
  assert.equal(currentLang(), "en-US");
  assert.equal(session.general.defaultExtractDir, "/Edited During Locale");
  assert.equal(session.savedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.appliedGeneral.defaultExtractDir, "/Saved Extract");
  assert.equal(session.appliedGeneral.language, "en-US");
  assert.equal(session.saveTarget, null);
  assert.equal(session.generalSaveState, "error");
  assert.deepEqual(effects.map((effect) => effect.kind), ["notice"]);
});

test("valid saves use byte units, normalized folders and nulls for automatic defaults", async () => {
  const { session, seed, effects } = harness();
  seed();
  const calls = [];
  ipc.setSafetyLimits = async (...args) => {
    calls.push(["security", ...args]);
    return settings({ safety_max_output_bytes: args[0], safety_max_entries: args[1], safety_max_compression_ratio: args[2] });
  };
  ipc.setPerformanceOptions = async (...args) => {
    calls.push(["performance", ...args]);
    return settings({ performance_threads: args[0], performance_memory_limit_bytes: args[1], performance_parallel_jobs: args[2] });
  };
  ipc.setGeneralOptions = async (...args) => {
    calls.push(["general", ...args]);
    return settings({ language: args[0], default_create_dir: args[1], default_extract_dir: args[2],
      reveal_after_extract: args[3], check_updates_automatically: args[4] });
  };
  ipc.setAccentPalette = async (...args) => {
    calls.push(["colors", ...args]);
    return settings({ accent_palette: args[0], custom_accent: args[1], accent_contrast_guard: args[2] });
  };
  session.setSecurity("maxOutputGiB", 12);
  session.setSecurity("maxEntries", 17);
  session.setSecurity("maxCompressionRatio", 9);
  await session.saveSecurity();
  session.resetSecurity();
  await session.saveSecurity();
  session.setPerformance("threads", 8);
  session.setPerformance("memoryKiB", 16);
  session.setPerformance("parallelJobs", 3);
  await session.savePerformance();
  session.resetPerformance();
  await session.savePerformance();
  session.setGeneral("defaultCreateDir", " /Archives/ ");
  session.setGeneral("defaultExtractDir", " /Extracts/ ");
  await session.saveGeneral();
  session.setGeneral("language", "");
  session.setGeneral("defaultCreateDir", "");
  session.setGeneral("defaultExtractDir", "");
  await session.saveGeneral();
  session.updateCustomAccent("#3af", "hex");
  session.setAccentContrastGuard(false);
  await session.saveColors();

  assert.deepEqual(calls, [
    ["security", 12 * 1024 ** 3, 17, 9], ["security", null, null, null],
    ["performance", 8, 16 * 1024, 3], ["performance", null, null, null],
    ["general", "en-US", "/Archives", "/Extracts", false, true],
    ["general", null, null, null, false, true], ["colors", "custom", "#33AAFF", false],
  ]);
  assert.equal(session.securitySaveState, "saved");
  assert.equal(session.performanceSaveState, "saved");
  assert.equal(session.generalSaveState, "saved");
  assert.equal(session.colorsSaveState, "saved");
  assert.equal(effects.filter((effect) => effect.kind === "generalSaved").length, 2);
});

test("invalid folder, numeric and hex drafts never reach IPC or lose the last valid color", async () => {
  const { session, seed, effects } = harness();
  seed();
  const calls = [];
  for (const method of ["setGeneralOptions", "setSafetyLimits", "setPerformanceOptions", "setAccentPalette"]) {
    ipc[method] = async (...args) => { calls.push([method, ...args]); return settings(); };
  }
  session.setGeneral("defaultCreateDir", "relative/folder");
  await session.saveGeneral();
  session.setSecurity("maxOutputGiB", 1.5);
  await session.saveSecurity();
  session.setSecurity("maxOutputGiB", null);
  await session.saveSecurity();
  session.setPerformance("memoryKiB", 7);
  await session.savePerformance();
  session.updateCustomAccent("#336699", "hex");
  session.updateCustomAccent("#not-hex", "hex");
  effects.length = 0;
  await session.saveColors();

  assert.deepEqual(calls, []);
  assert.equal(session.saveTarget, null);
  assert.equal(session.general.defaultCreateDir, "relative/folder");
  assert.equal(session.security.maxOutputGiB, null);
  assert.equal(session.performance.memoryKiB, 7);
  assert.equal(session.colors.input, "#NOT-HEX");
  assert.equal(session.colors.accent, "#336699");
  assert.equal(session.colors.saveError, true);
  assert.equal(session.customAccentValid, false);
  assert.equal(session.savedGeneral.defaultCreateDir, "");
  assert.equal(effects.filter((effect) => effect.kind === "notice").length, 1);
});
