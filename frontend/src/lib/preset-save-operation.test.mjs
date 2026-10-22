import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
const { ArchivePresetSession, maxArchivePresets, maxArchivePresetExcludeRules, maxArchivePresetExcludeRuleBytes } =
  await server.ssrLoadModule("/src/lib/archive-presets.svelte.ts");
const { CreateOptionsDraft } = await server.ssrLoadModule("/src/lib/create-options-draft.svelte.ts");
const { ExtractOptionsDraft } = await server.ssrLoadModule("/src/lib/extract-options-draft.svelte.ts");
const { createPresetSaveIssue } = await server.ssrLoadModule("/src/lib/create-preset-validation.ts");
const { clearOperationHistory, operationHistory } = await server.ssrLoadModule("/src/lib/history.svelte.ts");
const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
const originalIpc = { ...ipc };
test.before(async () => {
  ipc.getLocaleTable = async () => { throw new Error("use bundled locale"); };
  try { await loadLocale("en-US"); }
  finally { ipc.getLocaleTable = originalIpc.getLocaleTable; }
});
test.beforeEach(() => clearOperationHistory());
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

const builtinCreateOptions = { format: "7z", level: 5, credential: { kind: "none" }, encrypt_names: false,
  volumes: { kind: "single" }, content_policy: "custom", excludes: [], output: { kind: "archive" },
  destination: { base: "ask", existing_output: "ask" }, format_options: { kind: "none" },
  completion: "none", post_success: "keep_source", test_after_create: false };
const builtinExtractOptions = { destination: { base: "default_directory", layout: "smart" }, existing_output: "ask",
  symlinks: "preserve", encoding: { kind: "auto" }, credential: { kind: "prompt_when_needed" }, post_success: "keep_source" };

function harness() {
  const notices = [];
  const desktop = { preflightBusy: false, currentEncoding: "shift_jis", ready: true };
  const context = { capabilityReady: true, sfxCapability: { target: "macos", available: true, extension: "app" } };
  const createOptions = new CreateOptionsDraft({ format: "sqz", profile: "custom", customLevel: 7, touched: true });
  const extractOptions = new ExtractOptionsDraft(() => "macos");
  extractOptions.applyPreset({ ...builtinExtractOptions, existing_output: "rename", encoding: { kind: "named", label: "gbk" } });
  for (const [kind, value] of [["splitPreset", "custom"], ["customSplitAmount", "1.5"], ["customSplitUnit", "gib"],
    ["contentPolicy", "custom"], ["excludeText", "*.cache; temporary/**\n*.cache"], ["sqzInnerFormat", "zip"],
    ["destinationBase", "default_directory"], ["completion", "reveal_output"], ["testAfterCreate", true]]) {
    createOptions.edit({ kind, value }, context, () => {});
  }
  const issueMessage = (issue) => issue === "invalid_part_size"
    ? tFallback("gui.create.invalid_part_size", "Enter a part size of at least 0.1 MiB") : issue;
  const createDisabledReason = () => desktop.preflightBusy ? "Wait for the current preflight to finish" : "";
  const session = new ArchivePresetSession({
    create: {
      options: () => createOptions.snapshotPreset(),
      apply: (preset, announce) => {
        if (desktop.preflightBusy) return false;
        return !preset || !createOptions.applyPreset(preset.options, context, () => {}, announce).issue;
      },
      ready: () => desktop.ready, touched: () => createOptions.state.touched,
      disabledReason: createDisabledReason,
      saveIssue: () => createPresetSaveIssue(createOptions, issueMessage),
    },
    extract: {
      options: () => extractOptions.snapshotPreset(desktop.currentEncoding),
      apply: (preset) => { if (preset) extractOptions.applyPreset(preset.options); return true; },
      ready: () => desktop.ready, touched: () => extractOptions.state.touched, disabledReason: () => "",
    },
    notice: (message) => notices.push(message),
  });
  const document = { schema_version: 1, revision: 7, presets: [
    { kind: "create", id: "user.create.original", label: "Daily backup", built_in: false, options: createOptions.snapshotPreset() },
    { kind: "extract", id: "user.extract.original", label: "Extract old", built_in: false,
      options: extractOptions.snapshotPreset(desktop.currentEncoding) },
    { kind: "create", id: "builtin.create.balanced-7z", label: "Balanced 7Z", built_in: true, options: structuredClone(builtinCreateOptions) },
    { kind: "create", id: "builtin.create.cross-platform-7z", label: "Cross-platform 7Z", built_in: true,
      options: { ...structuredClone(builtinCreateOptions), content_policy: "cross_platform_clean" } },
    { kind: "extract", id: "builtin.extract.smart", label: "Smart extract", built_in: true, options: structuredClone(builtinExtractOptions) },
  ], bindings: { app_default_create: "user.create.original", app_default_extract: "user.extract.original",
    file_manager_create: null, file_manager_extract: "user.extract.original" } };
  return {
    session, createOptions, extractOptions, desktop, context, notices, document,
    get operations() { return [...operationHistory()].reverse().map(({ status, title, detail }) => ({ status, title, detail })); },
    async load(reads) {
      const loading = session.load();
      reads.pop().resolve(structuredClone(document));
      await loading;
      session.apply("create", "user.create.original");
      session.apply("extract", "user.extract.original");
      session.editName("create", "  Daily \n backup ");
      session.editName("extract", " Daily\t backup ");
    },
  };
}

test("saving new create and extract presets captures their options and publishes only successful replies", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  await app.load(reads);
  const { session, createOptions, extractOptions, desktop, context } = app;
  const original = session.document;
  const originalBytes = JSON.stringify(original);
  const creating = session.save("create", "save_as");
  assert.equal(writes.length, 1);
  assert.equal(writes[0].expectedRevision, 7);
  const created = writes[0].document.presets.at(-1);
  assert.match(created.id, /^user\.create\.[a-f0-9]{32}$/u);
  assert.notEqual(created.id, session.selection("create").id);
  assert.deepEqual(created, {
    kind: "create", id: created.id, label: "Daily backup 2", built_in: false,
    options: { format: "sqz", level: 7, credential: { kind: "none" }, encrypt_names: false,
      volumes: { kind: "split", size_bytes: "1610612736" }, content_policy: "custom",
      excludes: ["*.cache", "temporary/**"], output: { kind: "archive" },
      format_options: { kind: "sqz", inner_format: "zip" },
      destination: { base: "default_directory", existing_output: "rename" },
      completion: "reveal_output", post_success: "keep_source", test_after_create: true },
  });
  assert.deepEqual(writes[0].document.bindings, structuredClone(original.bindings));
  assert.equal(JSON.stringify(original), originalBytes);
  assert.equal(session.document, original);
  assert.equal(session.selection("create").id, "user.create.original");
  assert.equal(session.selection("create").name, "  Daily \n backup ");
  assert.equal(session.selection("create").mutation, "saving");
  assert.deepEqual(app.notices, []);
  assert.deepEqual(app.operations, []);
  await session.save("create", "save_as");
  assert.equal(writes.length, 1, "a second save for the same kind cannot issue another persistence call");
  const createReply = { ...writes[0].document, revision: 8 };
  writes[0].resolve(createReply);
  await creating;
  assert.deepEqual(structuredClone(session.document), createReply);
  const createDocument = session.document;
  assert.equal(session.selection("create").id, created.id);
  assert.equal(session.selection("create").name, created.label);
  assert.equal(session.selection("create").mutation, "idle");

  const extracting = session.save("extract", "save_as");
  assert.equal(writes[1].expectedRevision, 8);
  const extracted = writes[1].document.presets.at(-1);
  assert.match(extracted.id, /^user\.extract\.[a-f0-9]{32}$/u);
  assert.notEqual(extracted.id, session.selection("extract").id);
  assert.deepEqual(extracted, {
    kind: "extract", id: extracted.id, label: "Daily backup", built_in: false,
    options: { destination: { base: "default_directory", layout: "smart" }, existing_output: "rename",
      symlinks: "preserve", encoding: { kind: "named", label: "gbk" },
      credential: { kind: "prompt_when_needed" }, post_success: "keep_source" },
  });
  assert.equal(session.document, createDocument);
  assert.equal(session.selection("extract").id, "user.extract.original");
  assert.equal(session.selection("extract").name, " Daily\t backup ");
  const extractReply = { ...writes[1].document, revision: 9 };
  writes[1].resolve(extractReply);
  await extracting;
  assert.deepEqual(structuredClone(session.document), extractReply);
  assert.equal(session.selection("extract").id, extracted.id);
  assert.equal(session.selection("extract").name, extracted.label);
  assert.equal(session.selection("extract").mutation, "idle");
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
  await app.load(reads);
  const { session, createOptions, extractOptions, desktop, context } = app;
  const original = session.document;
  const unavailable = harness();
  await unavailable.session.save("create", "save_as");
  await unavailable.session.save("extract", "update");
  app.notices.push(...unavailable.notices);
  original.presets[0].built_in = true;
  await session.save("create", "update");
  original.presets[0].built_in = false;
  session.clearSelection("extract");
  await session.save("extract", "update");
  session.apply("extract", "user.extract.original");
  desktop.preflightBusy = true;
  await session.save("create", "update");
  desktop.preflightBusy = false;
  original.presets[0].options = structuredClone(original.presets[2].options);
  original.bindings.file_manager_create = "user.create.original";
  await session.save("create", "update");
  assert.equal(writes.length, 0);
  assert.deepEqual(app.notices, [
    "Could not load presets. The preset file was not changed.",
    "Wait for the current preflight to finish",
    "Turn off file-manager use before saving incompatible changes",
  ]);
  desktop.preflightBusy = true;
  const creating = session.save("create", "save_as");
  assert.equal(writes.length, 1, "SaveAs does not inherit the selected preset's update-only guards");
  const newCreateId = writes[0].document.presets.at(-1).id;
  writes[0].resolve({ ...writes[0].document, revision: 8 });
  await creating;

  desktop.preflightBusy = false;
  const draftBeforeReselect = createOptions.snapshotPreset();
  session.apply("create", "user.create.original");
  createOptions.applyPreset(draftBeforeReselect, context, () => {});
  session.document.bindings.file_manager_create = null;
  session.editName("create", "daily backup 2");
  await session.save("create", "update");
  session.editName("extract", " \n ");
  await session.save("extract", "save_as");
  createOptions.edit({ kind: "customSplitAmount", value: "0" }, context, () => {});
  for (const operation of ["update", "save_as"]) await session.save("create", operation);
  assert.equal(writes.length, 1);
  assert.deepEqual(app.notices.slice(-4), ["That preset name is already in use", "Enter a preset name",
    "Enter a part size of at least 0.1 MiB", "Enter a part size of at least 0.1 MiB"]);

  while (session.document.presets.length < maxArchivePresets) {
    const index = session.document.presets.length;
    session.document.presets.push({ ...session.document.presets[0], id: `user.create.extra${index}`, label: `Extra ${index}` });
  }
  createOptions.edit({ kind: "customSplitAmount", value: "1.5" }, context, () => {});
  await session.save("create", "save_as");
  assert.equal(writes.length, 1);
  assert.equal(app.notices.at(-1), "Delete a preset before saving another one");
  createOptions.edit({ kind: "customSplitAmount", value: "0" }, context, () => {});
  desktop.preflightBusy = true;
  session.editName("extract", "  Extract \n old ");
  desktop.currentEncoding = null;
  extractOptions.applyPreset({ ...extractOptions.snapshotPreset(desktop.currentEncoding),
    destination: { base: "ask", layout: "direct" }, encoding: { kind: "auto" } });
  const extracting = session.save("extract", "update");
  const updatedExtract = writes[1].document.presets.find((preset) => preset.id === "user.extract.original");
  assert.equal(writes[1].document.presets.length, maxArchivePresets);
  assert.deepEqual(updatedExtract.options, { destination: { base: "ask", layout: "direct" },
    existing_output: "rename", symlinks: "preserve", encoding: { kind: "auto" },
    credential: { kind: "prompt_when_needed" }, post_success: "keep_source" });
  assert.equal(session.selection("extract").name, "  Extract \n old ");
  writes[1].resolve({ ...writes[1].document, revision: 9 });
  await extracting;
  assert.equal(session.selection("extract").id, "user.extract.original");
  assert.equal(session.selection("extract").name, "Extract old", "updating may retain its own normalized name at the preset cap");

  desktop.preflightBusy = false;
  createOptions.applyPreset({ ...createOptions.snapshotPreset(), format: "zip",
    credential: { kind: "prompt" }, volumes: { kind: "single" }, output: { kind: "self_extracting", target: "macos" },
    format_options: { kind: "none" } }, context, () => {});
  createOptions.edit({ kind: "password", value: "preset-runtime-secret" }, context, () => {});
  createOptions.edit({ kind: "passwordConfirmation", value: "preset-runtime-secret" }, context, () => {});
  session.editName("create", "  Updated create ");
  const updating = session.save("create", "update");
  const updatedCreate = writes[2].document.presets.find((preset) => preset.id === "user.create.original");
  assert.equal(writes[2].document.presets.length, maxArchivePresets);
  assert.equal(updatedCreate.kind, "create");
  assert.equal(updatedCreate.built_in, false);
  assert.deepEqual(updatedCreate.options.credential, { kind: "prompt" });
  assert.equal(Object.hasOwn(updatedCreate.options, "password"), false);
  assert.equal(JSON.stringify(writes[2].document).includes("preset-runtime-secret"), false, "the persistence DTO never carries a run credential");
  assert.equal(updatedCreate.options.encrypt_names, false);
  assert.deepEqual(updatedCreate.options.volumes, { kind: "single" });
  assert.deepEqual(updatedCreate.options.output, { kind: "self_extracting", target: "macos" });
  assert.deepEqual(updatedCreate.options.format_options, { kind: "none" });
  session.apply("create", newCreateId);
  writes[2].resolve({ ...writes[2].document, revision: 10 });
  await updating;
  assert.equal(session.selection("create").id, newCreateId, "a late update does not reselect the captured old preset");
  assert.equal(session.selection("create").name, "Updated create");
  assert.deepEqual(app.operations.map(({ title }) => title), ["Preset saved", "Preset updated", "Preset updated"]);
  assert.equal(reads.length, 0);
});

test("failed saves preserve drafts and selection while conflicts reload and reconcile the canonical document", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  await app.load(reads);
  const { session, createOptions, extractOptions, desktop, context } = app;
  const original = session.document;
  const originalBytes = JSON.stringify(original);
  session.editName("extract", "  Renamed extract ");
  const creating = session.save("create", "save_as");
  const extracting = session.save("extract", "update");
  assert.equal(writes.length, 2, "the other kind retains its independent persistence admission");
  assert.equal(session.selection("create").mutation, "saving");
  assert.equal(session.selection("extract").mutation, "saving");
  await session.save("extract", "update");
  assert.equal(writes.length, 2);
  writes[0].reject(new Error("persistence rejected"));
  const conflict = { key: "error.presets_conflict", params: {} };
  writes[1].reject(conflict);
  await Promise.all([creating, extracting]);
  assert.equal(session.document, original);
  assert.equal(JSON.stringify(original), originalBytes);
  assert.equal(session.selection("create").id, "user.create.original");
  assert.equal(session.selection("extract").id, "user.extract.original");
  assert.equal(session.selection("create").name, "  Daily \n backup ");
  assert.equal(session.selection("extract").name, "  Renamed extract ");
  assert.equal(session.selection("create").mutation, "error");
  assert.equal(session.selection("extract").mutation, "error");
  assert.equal(reads.length, 1);
  assert.deepEqual(app.operations, []);
  assert.deepEqual(app.notices, ["Could not save",
    "Presets changed in another Squallz window; the latest copy was reloaded"]);

  session.editName("create", "New create draft");
  session.editName("extract", "New extract draft");
  const latest = structuredClone(original);
  latest.revision = 12;
  latest.presets = latest.presets.filter((preset) => preset.id !== "user.extract.original");
  latest.bindings.app_default_extract = null;
  latest.bindings.file_manager_extract = null;
  reads[0].resolve(latest);
  await reads[0].promise;
  assert.deepEqual(structuredClone(session.document), latest);
  const latestDocument = session.document;
  assert.equal(session.loadState, "ready");
  assert.equal(session.selection("create").id, "user.create.original");
  assert.equal(session.selection("extract").id, null);
  assert.equal(session.selection("create").name, "New create draft");
  assert.equal(session.selection("extract").name, "New extract draft");
  assert.deepEqual(app.operations, []);

  const retrying = session.save("create", "update");
  assert.equal(writes[2].expectedRevision, 12);
  writes[2].reject(conflict);
  await retrying;
  assert.equal(reads.length, 2);
  reads[1].reject(new Error("reload rejected"));
  await reads[1].promise.catch(() => undefined);
  assert.equal(session.document, latestDocument);
  assert.equal(session.selection("create").id, "user.create.original");
  assert.equal(session.selection("create").name, "New create draft");
  assert.equal(session.selection("create").mutation, "error");
  assert.deepEqual(app.operations, []);

  session.editName("extract", "  Disk rejected draft ");
  const saving = session.save("extract", "save_as");
  writes[3].reject({ key: "error.io", params: { detail: "Disk write denied" } });
  await saving;
  assert.equal(app.notices.at(-1), "I/O error: Disk write denied");
  assert.equal(session.document, latestDocument);
  assert.equal(session.selection("extract").id, null);
  assert.equal(session.selection("extract").name, "  Disk rejected draft ");
  assert.equal(session.selection("extract").mutation, "error");
  assert.equal(reads.length, 2);
  assert.deepEqual(app.operations, []);
});

test("loading and default admission respect readiness, edited drafts, failures, and disposal", async () => {
  const { reads } = controlledIpc();
  const app = harness();
  const { session, desktop, createOptions } = app;
  assert.equal(session.status("create"), "saving");
  assert.equal(session.disabledReason("extract"), "Loading presets");
  const loading = session.load();
  desktop.ready = false;
  reads[0].resolve(app.document);
  await loading;
  assert.equal(session.loadState, "ready");
  assert.equal(session.selection("create").id, null);
  assert.equal(session.selection("extract").id, null);
  desktop.ready = true;
  session.applyDefault("create");
  session.applyDefault("extract");
  assert.equal(session.selection("create").id, null, "loading must not replace edited create options");
  assert.equal(session.selection("extract").id, "user.extract.original");
  assert.equal(createOptions.snapshotPreset().format, "sqz");
  session.apply("create", "builtin.create.balanced-7z", false);
  assert.equal(session.status("create"), "applied");
  session.editName("create", "  Balanced\n7Z ");
  assert.equal(session.status("create"), "applied");
  session.editName("create", "Edited");
  assert.equal(session.status("create"), "modified");
  session.clearSelection("create");
  assert.equal(session.status("create"), "idle");
  assert.deepEqual(session.selection("create"), { id: null, name: "", mutation: "idle" });
  const failed = harness();
  const failing = failed.session.load();
  reads[1].reject(new Error("unreadable"));
  await failing;
  assert.equal(failed.session.loadState, "error");
  assert.equal(failed.session.status("extract"), "error");
  assert.equal(failed.session.disabledReason("create"), "Could not load presets. The preset file was not changed.");
  assert.equal(failed.session.document, null);
  const disposed = harness();
  const pending = disposed.session.load();
  disposed.session.dispose();
  reads[2].resolve(disposed.document);
  await pending;
  assert.equal(disposed.session.document, null);
  assert.equal(disposed.session.selection("extract").id, null);
  assert.deepEqual(disposed.notices, []);
});

test("preset application, file-manager bindings, and deletion use one saved document", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  await app.load(reads);
  const { session, desktop } = app;
  const original = session.document;
  const selected = session.selection("create");
  session.apply("create", "user.extract.original");
  assert.equal(session.selection("create"), selected, "a wrong-kind preset cannot replace the selection");
  desktop.preflightBusy = true;
  session.apply("create", "builtin.create.balanced-7z");
  assert.equal(session.selection("create"), selected, "rejected draft application preserves selection");
  desktop.preflightBusy = false;
  original.presets[0].options.format = "not-an-archive-format";
  session.apply("create", "user.create.original");
  assert.equal(session.selection("create"), selected, "real draft format validation gates selection publication");
  original.presets[0].options = structuredClone(builtinCreateOptions);
  session.apply("create", "builtin.create.balanced-7z");
  assert.match(session.readOnlyReason("create"), /Built-in/);
  await session.save("create", "update");
  await session.remove("create");
  assert.equal(writes.length, 0);
  session.apply("create", "user.create.original");
  const incompatible = [
    { format: "zip" }, { credential: { kind: "prompt" } }, { output: { kind: "self_extracting", target: "macos" } },
    { completion: "open_in_squallz" }, { post_success: "trash_source" },
  ];
  for (const change of incompatible) {
    original.presets[0].options = { ...structuredClone(builtinCreateOptions), ...change };
    await session.bind("create", "file_manager", true);
    assert.match(session.finderDisabledReason(), /standard 7Z preset/);
  }
  assert.equal(writes.length, 0);
  original.presets[0].options = structuredClone(builtinCreateOptions);
  const binding = session.bind("create", "file_manager", true);
  assert.equal(original.bindings.file_manager_create, null, "binding is unpublished until persistence succeeds");
  assert.equal(writes[0].document.bindings.file_manager_create, "user.create.original");
  writes[0].resolve({ ...writes[0].document, revision: 8 });
  await binding;
  assert.equal(session.isBound("create", "file_manager"), true);
  const deleting = session.remove("create");
  assert.equal(session.document.presets.some(({ id }) => id === "user.create.original"), true);
  assert.equal(writes[1].document.bindings.app_default_create, "builtin.create.balanced-7z");
  assert.equal(writes[1].document.bindings.file_manager_create, "builtin.create.balanced-7z");
  assert.equal(writes[1].document.presets.some(({ id }) => id === "user.create.original"), false);
  desktop.preflightBusy = true;
  writes[1].resolve({ ...writes[1].document, revision: 9 });
  await deleting;
  assert.equal(session.selection("create").id, null, "deletion clears selection when draft application is blocked");
  assert.deepEqual(app.operations.map(({ title }) => title), ["Preset default changed", "Preset deleted"]);
  desktop.currentEncoding = null;
  session.apply("extract", "user.extract.original");
  const extractDeletion = session.remove("extract");
  writes[2].resolve({ ...writes[2].document, revision: 10 });
  await extractDeletion;
  assert.equal(session.selection("extract").id, "builtin.extract.smart");
  assert.equal(session.status("extract"), "applied");
});

test("actual draft validation enforces rule limits and Unicode name normalization before saving", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  await app.load(reads);
  const { session, createOptions, context } = app;
  createOptions.edit({ kind: "excludeText", value: Array.from({ length: maxArchivePresetExcludeRules + 1 }, (_, n) => `rule-${n}`).join("\n") }, context, () => {});
  await session.save("create", "save_as");
  assert.equal(app.notices.at(-1), "Presets can store up to 64 exclude rules");
  createOptions.edit({ kind: "excludeText", value: "界".repeat(Math.floor(maxArchivePresetExcludeRuleBytes / 3) + 1) }, context, () => {});
  await session.save("create", "save_as");
  assert.match(app.notices.at(-1), /each rule can use up to 256 bytes/);
  assert.equal(writes.length, 0);
  createOptions.edit({ kind: "excludeText", value: "a".repeat(maxArchivePresetExcludeRuleBytes) }, context, () => {});
  session.editName("create", "  " + "😀".repeat(45) + "\n ");
  const saving = session.save("create", "save_as");
  const first = writes[0].document.presets.at(-1);
  assert.equal(first.label, "😀".repeat(40), "name truncation must preserve Unicode code points");
  assert.deepEqual(first.options.excludes, ["a".repeat(maxArchivePresetExcludeRuleBytes)]);
  writes[0].resolve({ ...writes[0].document, revision: 8 });
  await saving;
  const duplicate = session.save("create", "save_as");
  const second = writes[1].document.presets.at(-1);
  assert.equal(second.label, "😀".repeat(38) + " 2");
  assert.equal(Array.from(second.label).length, 40);
  writes[1].resolve({ ...writes[1].document, revision: 9 });
  await duplicate;
});

test("disposal rejects late save publication and conflict reload publication", async () => {
  const { writes, reads } = controlledIpc();
  const app = harness();
  await app.load(reads);
  const original = app.session.document;
  const saving = app.session.save("create", "save_as");
  app.session.dispose();
  writes[0].resolve({ ...writes[0].document, revision: 8 });
  await saving;
  assert.equal(app.session.document, original);
  assert.equal(app.session.selection("create").id, "user.create.original");
  assert.deepEqual(app.notices, []);
  assert.deepEqual(app.operations, []);
  await app.session.save("extract", "save_as");
  assert.equal(writes.length, 1);
  const conflicted = harness();
  await conflicted.load(reads);
  const beforeConflict = conflicted.session.document;
  const conflicting = conflicted.session.save("extract", "save_as");
  writes[1].reject({ key: "error.presets_conflict", params: {} });
  await conflicting;
  const noticeCount = conflicted.notices.length;
  conflicted.session.dispose();
  reads[0].resolve({ ...beforeConflict, revision: 99, presets: [] });
  await reads[0].promise;
  assert.equal(conflicted.session.document, beforeConflict);
  assert.equal(conflicted.session.selection("extract").id, "user.extract.original");
  assert.equal(conflicted.notices.length, noticeCount);
  assert.deepEqual(conflicted.operations, []);
});

test("both preset picker presentations render the same session selection, status, and guidance", async () => {
  const { reads } = controlledIpc();
  const app = harness();
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: Picker } = await server.ssrLoadModule("/src/components/ArchivePresetPicker.svelte");
  for (const variant of ["modern", "classic"]) {
    const body = render(Picker, { props: { variant, kind: "create", instanceId: variant, session: app.session } }).body;
    assert.match(body, /Loading presets/);
    assert.match(body, new RegExp(`aria-describedby="${variant}-preset-disabled-reason"`));
    assert.match(body, /<select[^>]*disabled/);
  }
  await app.load(reads);
  for (const variant of ["modern", "classic"]) {
    const body = render(Picker, { props: { variant, kind: "create", instanceId: variant, session: app.session } }).body;
    assert.match(body, /Daily backup/);
    assert.match(body, /SQZ/);
    assert.match(body, /status-applied/);
    assert.doesNotMatch(body, /<select[^>]*disabled/);
  }
});
