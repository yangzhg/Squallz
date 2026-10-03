import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";
import { proxy, snapshot } from "svelte/internal/client";
import { createTestServer } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ipc, isErrorDto } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { createProfiles } = await server.ssrLoadModule("/src/lib/ui-model.ts");
const outputOptions = await server.ssrLoadModule("/src/lib/archive-output-options.ts");
const { parseDelimitedRules } = await server.ssrLoadModule("/src/lib/format.ts");
const { loadLocale, tError, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
const originalIpc = { saveArchivePresets: ipc.saveArchivePresets, getArchivePresets: ipc.getArchivePresets,
  getLocaleTable: ipc.getLocaleTable };
test.before(async () => {
  ipc.getLocaleTable = async () => { throw new Error("use bundled locale"); };
  try { await loadLocale("en-US"); }
  finally { ipc.getLocaleTable = originalIpc.getLocaleTable; }
});
test.afterEach(() => Object.assign(ipc, originalIpc));

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function controlledIpc() {
  const writes = [];
  const reads = [];
  ipc.saveArchivePresets = (expectedRevision, document) => {
    const response = deferred();
    writes.push({ expectedRevision, document: structuredClone(document), ...response });
    return response.promise;
  };
  ipc.getArchivePresets = () => {
    const response = deferred();
    reads.push(response);
    return response.promise;
  };
  return { writes, reads };
}

function harness() {
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const declarations = selectFunctions(source, [
    "createProfileData", "createTrashSourceDisabledReason", "effectiveCreateTestAfterCreate",
    "createSplitSizeBytes", "createSplitValidationMessage", "activeCreateProfileData", "createCompressionLevel",
    "archivePresetById", "currentCreateArchivePresetOptions", "currentExtractArchivePresetOptions",
    "normalizePresetName", "presetNameExists", "uniquePresetName", "createPresetSaveValidationMessage",
    "selectedCreateArchivePreset", "selectedExtractArchivePreset", "clonePresetDocument",
    "reconcilePresetSelections", "setPresetMutationState", "persistPresetDocument", "newArchivePresetId",
    "saveArchivePreset", "createPresetFinderCompatible", "createPresetUpdateDisabledReason", "presetReadOnlyReason",
    "archiveEncodingForJob", "createExcludeRules",
  ]);
  const constantNames = ["maxArchivePresets", "maxArchivePresetExcludeRules", "maxArchivePresetExcludeRuleBytes"];
  const constants = source.statements.filter((node) => ts.isVariableStatement(node)
    && node.declarationList.declarations.some((declaration) =>
      ts.isIdentifier(declaration.name) && constantNames.includes(declaration.name.text)));
  for (const name of constantNames) {
    assert.ok(constants.some((node) => node.declarationList.declarations.some((declaration) =>
      ts.isIdentifier(declaration.name) && declaration.name.text === name)), `${source.fileName}: ${name}`);
  }
  const notices = [];
  const operations = [];
  let nextId = 0;
  const state = {
    ipc, isErrorDto, tError, createProfiles, ...outputOptions, parseDelimitedRules, structuredClone, TextEncoder,
    $state: { snapshot },
    crypto: { randomUUID: () => `00000000-0000-0000-0000-${String(++nextId).padStart(12, "0")}` },
    tr: tFallback, trashNameLabel: () => "Trash",
    showNotice: (message) => notices.push(message), recordOperation: (operation) => operations.push(structuredClone(operation)),
    preflightBusy: false, createPreflight: { busy: () => state.preflightBusy },
    activeCreateFormat: "sqz", activeCreateProfile: "custom", customCreateLevel: 7,
    createEncryptionEnabled: false, createEncryptNames: false, createSplitPreset: "custom",
    createCustomSplitAmount: "1.5", createCustomSplitUnit: "gib", createPresetSplitSizeBytes: null,
    createSplitMode: "generic", createContentPolicy: "custom", createExcludeText: "*.cache; temporary/**\n*.cache",
    createSfxEnabled: false, createPresetSfxTarget: "macos", createPresetSqzInnerFormat: "zip",
    createDestinationBase: "default_directory", createOverwritePolicy: "rename", createCompletion: "reveal_output",
    createPostSuccess: "keep_source", createTestAfterCreate: true,
    extractDestinationMode: "smart", extractOverwriteMode: "rename", extractSymlinkMode: "preserve",
    extractPresetEncodingLabel: "gbk", currentArchive: { encoding_override: "shift_jis" },
    selectedCreatePresetId: "user.create.original", selectedExtractPresetId: "user.extract.original",
    createPresetDraftName: "  Daily \n backup ", extractPresetDraftName: " Daily\t backup ",
    createPresetMutationState: "idle", extractPresetMutationState: "idle", presetLoadState: "ready",
    presetDocument: { schema_version: 1, revision: 7, presets: [], bindings: {
      app_default_create: "user.create.original", app_default_extract: "user.extract.original",
      file_manager_create: null, file_manager_extract: "user.extract.original",
    } },
  };
  let presetDocument = proxy(state.presetDocument);
  Object.defineProperty(state, "presetDocument", {
    get: () => presetDocument,
    set: (value) => { presetDocument = proxy(value); },
  });
  const script = [...constants, ...declarations].map((node) => node.getText(source)).join("\n");
  const app = vm.runInNewContext(`${compileTestScript(script)}\n({saveArchivePreset,
    currentCreateArchivePresetOptions, currentExtractArchivePresetOptions, maxArchivePresets})`, state);
  const builtinCreateOptions = { format: "7z", level: 5, credential: { kind: "none" }, encrypt_names: false,
    volumes: { kind: "single" }, content_policy: "custom", excludes: [], output: { kind: "archive" },
    destination: { base: "ask", existing_output: "ask" }, format_options: { kind: "none" },
    completion: "none", post_success: "keep_source", test_after_create: false };
  state.presetDocument.presets = [
    { kind: "create", id: state.selectedCreatePresetId, label: "Daily backup", built_in: false,
      options: structuredClone(app.currentCreateArchivePresetOptions()) },
    { kind: "extract", id: state.selectedExtractPresetId, label: "Extract old", built_in: false,
      options: structuredClone(app.currentExtractArchivePresetOptions()) },
    { kind: "create", id: "builtin.create.balanced-7z", label: "Balanced 7Z", built_in: true,
      options: builtinCreateOptions },
    { kind: "create", id: "builtin.create.cross-platform-7z", label: "Cross-platform 7Z", built_in: true,
      options: { ...builtinCreateOptions, content_policy: "cross_platform_clean" } },
    { kind: "extract", id: "builtin.extract.smart", label: "Smart extract", built_in: true,
      options: { destination: { base: "default_directory", layout: "smart" }, existing_output: "ask",
        symlinks: "preserve", encoding: { kind: "auto" }, credential: { kind: "prompt_when_needed" },
        post_success: "keep_source" } },
  ];
  return { ...app, state, notices, operations };
}

test("saving new create and extract presets captures their options and publishes only successful replies", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  const { state } = app;
  const original = state.presetDocument;
  const originalBytes = JSON.stringify(original);
  const creating = app.saveArchivePreset("create", "save_as");
  assert.equal(writes.length, 1);
  assert.equal(writes[0].expectedRevision, 7);
  const created = writes[0].document.presets.at(-1);
  assert.match(created.id, /^user\.create\.[a-f0-9]{32}$/u);
  assert.notEqual(created.id, state.selectedCreatePresetId);
  assert.deepEqual(created, {
    kind: "create", id: created.id, label: "Daily backup 2", built_in: false,
    options: { format: "sqz", level: 7, credential: { kind: "none" }, encrypt_names: false,
      volumes: { kind: "split", size_bytes: "1610612736" }, content_policy: "custom",
      excludes: ["*.cache", "temporary/**"], output: { kind: "archive" },
      format_options: { kind: "sqz", inner_format: "zip" },
      destination: { base: "default_directory", existing_output: "rename" },
      completion: "reveal_output", post_success: "keep_source", test_after_create: true },
  });
  assert.deepEqual(writes[0].document.bindings, snapshot(original.bindings));
  assert.equal(JSON.stringify(original), originalBytes);
  assert.equal(state.presetDocument, original);
  assert.equal(state.selectedCreatePresetId, "user.create.original");
  assert.equal(state.createPresetDraftName, "  Daily \n backup ");
  assert.equal(state.createPresetMutationState, "saving");
  assert.deepEqual(app.notices, []);
  assert.deepEqual(app.operations, []);
  await app.saveArchivePreset("create", "save_as");
  assert.equal(writes.length, 1, "a second save for the same kind cannot issue another persistence call");
  const createReply = { ...writes[0].document, revision: 8 };
  writes[0].resolve(createReply);
  await creating;
  assert.deepEqual(snapshot(state.presetDocument), createReply);
  const createDocument = state.presetDocument;
  assert.equal(state.selectedCreatePresetId, created.id);
  assert.equal(state.createPresetDraftName, created.label);
  assert.equal(state.createPresetMutationState, "idle");

  const extracting = app.saveArchivePreset("extract", "save_as");
  assert.equal(writes[1].expectedRevision, 8);
  const extracted = writes[1].document.presets.at(-1);
  assert.match(extracted.id, /^user\.extract\.[a-f0-9]{32}$/u);
  assert.notEqual(extracted.id, state.selectedExtractPresetId);
  assert.deepEqual(extracted, {
    kind: "extract", id: extracted.id, label: "Daily backup", built_in: false,
    options: { destination: { base: "default_directory", layout: "smart" }, existing_output: "rename",
      symlinks: "preserve", encoding: { kind: "named", label: "gbk" },
      credential: { kind: "prompt_when_needed" }, post_success: "keep_source" },
  });
  assert.equal(state.presetDocument, createDocument);
  assert.equal(state.selectedExtractPresetId, "user.extract.original");
  assert.equal(state.extractPresetDraftName, " Daily\t backup ");
  const extractReply = { ...writes[1].document, revision: 9 };
  writes[1].resolve(extractReply);
  await extracting;
  assert.deepEqual(snapshot(state.presetDocument), extractReply);
  assert.equal(state.selectedExtractPresetId, extracted.id);
  assert.equal(state.extractPresetDraftName, extracted.label);
  assert.equal(state.extractPresetMutationState, "idle");
  assert.deepEqual(app.notices, ["Preset saved", "Preset saved"]);
  assert.deepEqual(app.operations, [
    { status: "done", title: "Preset saved", detail: "Daily backup 2" },
    { status: "done", title: "Preset saved", detail: "Daily backup" },
  ]);
  assert.equal(reads.length, 0);
});

test("updating preserves preset identity and keeps SaveAs admission separate from update guards", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  const { state } = app;
  const original = state.presetDocument;
  state.presetDocument = null;
  await app.saveArchivePreset("create", "save_as");
  await app.saveArchivePreset("extract", "update");
  state.presetDocument = original;
  original.presets[0].built_in = true;
  await app.saveArchivePreset("create", "update");
  original.presets[0].built_in = false;
  state.selectedExtractPresetId = "missing";
  await app.saveArchivePreset("extract", "update");
  state.selectedExtractPresetId = "user.extract.original";
  state.preflightBusy = true;
  await app.saveArchivePreset("create", "update");
  state.preflightBusy = false;
  original.presets[0].options = snapshot(original.presets[2].options);
  original.bindings.file_manager_create = "user.create.original";
  await app.saveArchivePreset("create", "update");
  assert.equal(writes.length, 0);
  assert.deepEqual(app.notices, [
    "Could not load presets. The preset file was not changed.",
    "Wait for the current preflight to finish",
    "Turn off file-manager use before saving incompatible changes",
  ]);
  state.preflightBusy = true;
  const creating = app.saveArchivePreset("create", "save_as");
  assert.equal(writes.length, 1, "SaveAs does not inherit the selected preset's update-only guards");
  const newCreateId = writes[0].document.presets.at(-1).id;
  writes[0].resolve({ ...writes[0].document, revision: 8 });
  await creating;

  state.selectedCreatePresetId = "user.create.original";
  state.preflightBusy = false;
  state.presetDocument.bindings.file_manager_create = null;
  state.createPresetDraftName = "daily backup 2";
  await app.saveArchivePreset("create", "update");
  state.extractPresetDraftName = " \n ";
  await app.saveArchivePreset("extract", "save_as");
  state.createCustomSplitAmount = "0";
  for (const operation of ["update", "save_as"]) await app.saveArchivePreset("create", operation);
  assert.equal(writes.length, 1);
  assert.deepEqual(app.notices.slice(-4), ["That preset name is already in use", "Enter a preset name",
    "Enter a part size of at least 0.1 MiB", "Enter a part size of at least 0.1 MiB"]);

  while (state.presetDocument.presets.length < app.maxArchivePresets) {
    const index = state.presetDocument.presets.length;
    state.presetDocument.presets.push({ ...state.presetDocument.presets[0], id: `user.create.extra${index}`, label: `Extra ${index}` });
  }
  state.createCustomSplitAmount = "1.5";
  await app.saveArchivePreset("create", "save_as");
  assert.equal(writes.length, 1);
  assert.equal(app.notices.at(-1), "Delete a preset before saving another one");
  state.createCustomSplitAmount = "0";
  state.preflightBusy = true;
  state.extractPresetDraftName = "  Extract \n old ";
  state.extractDestinationMode = "choose";
  state.extractPresetEncodingLabel = null;
  state.currentArchive.encoding_override = null;
  const extracting = app.saveArchivePreset("extract", "update");
  const updatedExtract = writes[1].document.presets.find((preset) => preset.id === "user.extract.original");
  assert.equal(writes[1].document.presets.length, app.maxArchivePresets);
  assert.deepEqual(updatedExtract.options, { destination: { base: "ask", layout: "direct" },
    existing_output: "rename", symlinks: "preserve", encoding: { kind: "auto" },
    credential: { kind: "prompt_when_needed" }, post_success: "keep_source" });
  assert.equal(state.extractPresetDraftName, "  Extract \n old ");
  writes[1].resolve({ ...writes[1].document, revision: 9 });
  await extracting;
  assert.equal(state.selectedExtractPresetId, "user.extract.original");
  assert.equal(state.extractPresetDraftName, "Extract old", "updating may retain its own normalized name at the preset cap");

  Object.assign(state, { preflightBusy: false, createSplitPreset: "none", activeCreateFormat: "zip",
    createEncryptionEnabled: true, createSfxEnabled: true, createPresetDraftName: "  Updated create " });
  const updating = app.saveArchivePreset("create", "update");
  const updatedCreate = writes[2].document.presets.find((preset) => preset.id === "user.create.original");
  assert.equal(writes[2].document.presets.length, app.maxArchivePresets);
  assert.equal(updatedCreate.kind, "create");
  assert.equal(updatedCreate.built_in, false);
  assert.deepEqual(updatedCreate.options.credential, { kind: "prompt" });
  assert.equal(updatedCreate.options.encrypt_names, false);
  assert.deepEqual(updatedCreate.options.volumes, { kind: "single" });
  assert.deepEqual(updatedCreate.options.output, { kind: "self_extracting", target: "macos" });
  assert.deepEqual(updatedCreate.options.format_options, { kind: "none" });
  state.selectedCreatePresetId = newCreateId;
  writes[2].resolve({ ...writes[2].document, revision: 10 });
  await updating;
  assert.equal(state.selectedCreatePresetId, newCreateId, "a late update does not reselect the captured old preset");
  assert.equal(state.createPresetDraftName, "Updated create");
  assert.deepEqual(app.operations.map(({ title }) => title), ["Preset saved", "Preset updated", "Preset updated"]);
  assert.equal(reads.length, 0);
});

test("failed saves preserve drafts and selection while conflicts reload and reconcile the canonical document", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  const { state } = app;
  const original = state.presetDocument;
  const originalBytes = JSON.stringify(original);
  state.extractPresetDraftName = "  Renamed extract ";
  const creating = app.saveArchivePreset("create", "save_as");
  const extracting = app.saveArchivePreset("extract", "update");
  assert.equal(writes.length, 2, "the other kind retains its independent persistence admission");
  assert.equal(state.createPresetMutationState, "saving");
  assert.equal(state.extractPresetMutationState, "saving");
  await app.saveArchivePreset("extract", "update");
  assert.equal(writes.length, 2);
  writes[0].reject(new Error("persistence rejected"));
  const conflict = { key: "error.presets_conflict", params: {} };
  writes[1].reject(conflict);
  await Promise.all([creating, extracting]);
  assert.equal(state.presetDocument, original);
  assert.equal(JSON.stringify(original), originalBytes);
  assert.equal(state.selectedCreatePresetId, "user.create.original");
  assert.equal(state.selectedExtractPresetId, "user.extract.original");
  assert.equal(state.createPresetDraftName, "  Daily \n backup ");
  assert.equal(state.extractPresetDraftName, "  Renamed extract ");
  assert.equal(state.createPresetMutationState, "error");
  assert.equal(state.extractPresetMutationState, "error");
  assert.equal(reads.length, 1);
  assert.deepEqual(app.operations, []);
  assert.deepEqual(app.notices, ["Could not save",
    "Presets changed in another Squallz window; the latest copy was reloaded"]);

  state.createPresetDraftName = "New create draft";
  state.extractPresetDraftName = "New extract draft";
  const latest = snapshot(original);
  latest.revision = 12;
  latest.presets = latest.presets.filter((preset) => preset.id !== "user.extract.original");
  latest.bindings.app_default_extract = null;
  latest.bindings.file_manager_extract = null;
  reads[0].resolve(latest);
  await reads[0].promise;
  assert.deepEqual(snapshot(state.presetDocument), latest);
  const latestDocument = state.presetDocument;
  assert.equal(state.presetLoadState, "ready");
  assert.equal(state.selectedCreatePresetId, "user.create.original");
  assert.equal(state.selectedExtractPresetId, null);
  assert.equal(state.createPresetDraftName, "New create draft");
  assert.equal(state.extractPresetDraftName, "New extract draft");
  assert.deepEqual(app.operations, []);

  const retrying = app.saveArchivePreset("create", "update");
  assert.equal(writes[2].expectedRevision, 12);
  writes[2].reject(conflict);
  await retrying;
  assert.equal(reads.length, 2);
  reads[1].reject(new Error("reload rejected"));
  await reads[1].promise.catch(() => undefined);
  assert.equal(state.presetDocument, latestDocument);
  assert.equal(state.selectedCreatePresetId, "user.create.original");
  assert.equal(state.createPresetDraftName, "New create draft");
  assert.equal(state.createPresetMutationState, "error");
  assert.deepEqual(app.operations, []);

  state.extractPresetDraftName = "  Disk rejected draft ";
  const saving = app.saveArchivePreset("extract", "save_as");
  writes[3].reject({ key: "error.io", params: { detail: "Disk write denied" } });
  await saving;
  assert.equal(app.notices.at(-1), "I/O error: Disk write denied");
  assert.equal(state.presetDocument, latestDocument);
  assert.equal(state.selectedExtractPresetId, null);
  assert.equal(state.extractPresetDraftName, "  Disk rejected draft ");
  assert.equal(state.extractPresetMutationState, "error");
  assert.equal(reads.length, 2);
  assert.deepEqual(app.operations, []);
});
