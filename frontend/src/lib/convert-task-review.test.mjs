import assert from "node:assert/strict";
import test from "node:test";
import { englishTranslate, createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { CreateOptionsDraft } = await server.ssrLoadModule("/src/lib/create-options-draft.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");

const originalArchive = { id: 1, path: "/original/photos.7z", source: "/original/photos.7z",
  name: "photos.7z", format: "7z", entry_count: 3, garbled_count: 0, non_utf8_name_count: 0,
  encoding_override: "gbk" };
function draft(overrides = {}) {
  return { src: originalArchive.source, dest: "/output/Original.ZIP", src_encoding: "gbk",
    level: 4, encrypt_names: false, outputPasswordRequired: false,
    split_size: 123456789, split_mode: "native", ...overrides };
}

function harness(t) {
  const calls = [];
  let archive = { ...originalArchive };
  const bridge = {
    getArchive: () => archive, tr: englishTranslate, tError: () => "Read/write error",
    showNotice: (message) => calls.push(["notice", message]), ensurePreflightListener: async () => {},
    getDialogModule: async () => ({ save() {}, confirm: async () => { calls.push(["confirm"]); return true; } }),
    saveNativeDialog: async (_kind, _save, options) => { calls.push(["save", options.defaultPath]); return options.defaultPath; },
    submitJob: async (spec) => { calls.push(["submit", spec]); return 42; },
    focusBlockingTaskIfAny: () => false, isJobSubmitBlocked: () => false,
    jobSubmitBlockedMessage: () => "Busy", recordQueuedOperation: (title) => calls.push(["queued", title]),
    archiveStemName: (name) => name.replace(/\.[^.]+$/, ""), platform: () => "macos",
    prepareSubmitFocus() {}, shouldRestorePrimaryFocus: () => false, register() {},
  };
  const originals = { inspectCreateDestination: ipc.inspectCreateDestination, planConvert: ipc.planConvert,
    checkDiskSpace: ipc.checkDiskSpace, tempDir: ipc.tempDir };
  t.after(() => Object.assign(ipc, originals));
  let inspectionCount = 0;
  ipc.inspectCreateDestination = async (path) => {
    calls.push(["inspect", path]);
    return { conflict: true, guard: `fresh-${++inspectionCount}` };
  };
  ipc.planConvert = async (spec) => {
    calls.push(["plan", spec]);
    return { input_count: 1, entries: 3, files: 2, directories: 1, symlinks: 0,
      total_bytes: 700, primary_output: spec.dest, output_budget_bytes: 800,
      archive_output_budget_bytes: 800, final_output_budget_bytes: 800,
      workspace_budget_bytes: 1600, system_temp_budget_bytes: 0, split_volume_count_budget: 2 };
  };
  ipc.checkDiskSpace = async (path, required_bytes) => {
    calls.push(["space", path, required_bytes]);
    return { path, required_bytes, available_bytes: 10000, ok: true };
  };
  ipc.tempDir = async () => "/temporary";
  const session = convertSessionFor({}, bridge);
  session.syncArchive(archive);
  t.after(() => session.dispose());
  return { session, bridge, calls, setArchive: (value) => { archive = value; } };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert.fail("Conversion did not reach the expected state");
}

test("review restores conversion options and obtains a fresh output authorization before submission", async (t) => {
  const { session, calls } = harness(t);
  session.surface("modern").protection.onPasswordInput("unrelated-password");
  session.surface("modern").protection.onPasswordConfirmationInput("unrelated-password");
  assert.equal(session.restoreTaskDraft(draft()), true);
  const surface = session.surface("classic");
  assert.equal(surface.source.path, originalArchive.path);
  assert.equal(surface.formats.find((item) => item.selected).id, "zip");
  assert.equal(surface.compression.level, 4);
  assert.equal(surface.protection.splitMode, "native");
  assert.equal(surface.protection.password, "");
  assert.equal(surface.protection.passwordConfirmation, "");
  assert.equal(surface.review, null);
  assert.match(surface.destination.path, /^\/output\/Original\.ZIP/);
  assert.equal(calls.length, 0);
  surface.start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  assert.deepEqual(calls.find(([name]) => name === "save"), ["save", "/output/Original.ZIP"]);
  const plan = calls.find(([name]) => name === "plan")[1];
  assert.equal(plan.src, originalArchive.source);
  assert.equal(plan.src_encoding, "gbk");
  assert.equal(plan.level, 4);
  assert.equal(plan.split_size, 123456789);
  assert.equal(plan.split_mode, "native");
  assert.equal(plan.src_password, null);
  assert.equal(plan.dest_password, null);
  assert.equal(plan.replacement_guard, "fresh-1");
  assert.equal(calls.some(([name]) => name === "submit"), false);
  assert.ok(calls.some(([name]) => name === "space"));
  session.surface("modern").review.onConfirm();
  await waitFor(() => session.surface("modern").preflight.phase === "ready");
  const submitted = calls.find(([name]) => name === "submit")[1];
  assert.equal(submitted.replacement_guard, "fresh-2");
  assert.equal(submitted.split_size, 123456789);
  assert.equal(calls.filter(([name]) => name === "confirm").length, 2);
});

test("format changes update the restored destination and explicit size changes replace exact bytes", async (t) => {
  const { session, calls } = harness(t);
  assert.equal(session.restoreTaskDraft(draft()), true);
  session.surface("modern").formats.find((item) => item.id === "7z").onSelect();
  assert.match(session.status().destination, /^\/output\/Original\.7z/);
  assert.equal(session.surface("modern").protection.splitMode, "generic");
  session.surface("modern").protection.onCustomSplitAmountInput("2");
  session.surface("modern").start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  assert.equal(calls.find(([name]) => name === "plan")[1].split_size, 2 * 1024 ** 2);
  session.surface("modern").review.onCancel();
  assert.equal(session.restoreTaskDraft(draft({ dest: "/output/Original.swm", split_mode: "native" })), true);
  assert.match(session.status().destination, /^\/output\/Original\.swm/);
  session.surface("modern").protection.onSplitPresetChange("none");
  assert.equal(session.status().destination, "/output/Original.wim");
  assert.equal(session.restoreTaskDraft(draft({ dest: "/output/Original.TZST", split_size: null, split_mode: "generic" })), true);
  assert.equal(session.status().destination, "/output/Original.TZST");
});

test("active checks and unconfirmed plans are not overwritten by a failed task review", async (t) => {
  const { session, bridge, calls } = harness(t);
  let finishPicker;
  bridge.saveNativeDialog = () => new Promise((resolve) => { finishPicker = resolve; });
  session.surface("modern").start.onSelect();
  await waitFor(() => Boolean(finishPicker));
  assert.equal(session.restoreTaskDraft(draft()), false);
  assert.equal(session.surface("modern").compression.level, 6);
  finishPicker("/different/output.zip");
  await waitFor(() => session.surface("modern").review !== null);
  assert.equal(session.restoreTaskDraft(draft()), false);
  assert.equal(session.surface("modern").preflight.destination, "/different/output.zip");
  assert.equal(calls.some(([name]) => name === "submit"), false);
});

test("unsupported formats and a different source leave the current conversion draft intact", (t) => {
  const { session } = harness(t);
  session.surface("modern").protection.onSplitPresetChange("100-mib");
  for (const options of [{ dest: "/output/a.rar" }, { src: "/another.zip" }, { src_encoding: null }]) {
    assert.equal(session.restoreTaskDraft(draft(options)), false);
    assert.equal(session.surface("modern").protection.splitPreset, "100-mib");
    assert.equal(session.surface("modern").compression.level, 6);
  }
});

test("restored data encryption blocks plaintext conversion until the user explicitly turns it off", async (t) => {
  const { session, calls } = harness(t);
  assert.equal(session.restoreTaskDraft(draft({ outputPasswordRequired: true })), true);
  session.surface("modern").start.onSelect();
  await Promise.resolve();
  assert.equal(calls.some(([name]) => name === "save"), false, "redacted passwords must not silently disable encryption");
  assert.equal(session.surface("classic").protection.encryptionEnabled, true);
  session.surface("classic").protection.onPasswordInput("new-password");
  session.surface("classic").protection.onPasswordConfirmationInput("new-password");
  session.leave();
  assert.equal(session.surface("modern").protection.password, "");
  assert.equal(session.surface("modern").protection.encryptionEnabled, true);
  session.surface("modern").formats.find((item) => item.id === "tar.zst").onSelect();
  assert.equal(session.surface("modern").formats.find((item) => item.selected).id, "zip");
  session.surface("modern").protection.onPasswordInput("replacement");
  session.surface("modern").protection.onPasswordConfirmationInput("replacement");
  session.surface("modern").protection.onPasswordInput("");
  session.surface("modern").start.onSelect();
  await Promise.resolve();
  assert.equal(calls.some(([name]) => name === "save"), false);
  session.surface("modern").protection.onEncryptionEnabledChange(false);
  assert.equal(session.surface("modern").protection.encryptionEnabled, false);
  session.surface("modern").start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  assert.equal(calls.find(([name]) => name === "plan")[1].dest_password, null);
});

test("restored name encryption requires a new password or an explicit opt-out and survives leaving", async (t) => {
  const { session, calls } = harness(t);
  assert.equal(session.restoreTaskDraft(draft({ dest: "/output/secure.7z", split_mode: "generic", encrypt_names: true })), true);
  assert.equal(session.surface("modern").protection.encryptNames, true);
  assert.match(session.surface("modern").protection.passwordError, /new destination password/);
  session.surface("modern").start.onSelect();
  await Promise.resolve();
  assert.equal(calls.some(([name]) => name === "save"), false);
  session.surface("modern").protection.onPasswordInput("new-password");
  session.surface("modern").protection.onPasswordConfirmationInput("new-password");
  session.leave();
  assert.equal(session.surface("modern").protection.password, "");
  assert.equal(session.surface("modern").protection.encryptNames, true);
  session.surface("modern").start.onSelect();
  await Promise.resolve();
  assert.equal(calls.some(([name]) => name === "save"), false);
  session.surface("modern").protection.onPasswordInput("new-password");
  session.surface("modern").protection.onPasswordConfirmationInput("new-password");
  session.surface("modern").start.onSelect();
  await waitFor(() => session.surface("modern").review !== null);
  assert.equal(calls.find(([name]) => name === "plan")[1].encrypt_names, true);
  session.surface("modern").review.onCancel();
  session.surface("modern").protection.onEncryptNamesChange(false);
  assert.equal(session.surface("modern").protection.encryptNames, false);
  assert.match(session.surface("modern").protection.passwordError, /new destination password/);
  session.surface("modern").protection.onEncryptionEnabledChange(false);
  assert.equal(session.surface("modern").protection.passwordError, "");
});

test("creation and conversion share output edits while preserving format capability guards", (t) => {
  const { session, calls } = harness(t);
  const create = new CreateOptionsDraft({ format: "7z", profile: "balanced", customLevel: 6 });
  const context = { capabilityReady: true, sfxCapability: { available: true, target: "macos", extension: "app" } };
  session.surface("modern").formats.find((item) => item.id === "7z").onSelect();
  const callbacks = {
    password: "onPasswordInput", passwordConfirmation: "onPasswordConfirmationInput",
    passwordVisible: "onPasswordVisibleChange", encryptionEnabled: "onEncryptionEnabledChange",
    encryptNames: "onEncryptNamesChange", splitPreset: "onSplitPresetChange", splitMode: "onSplitModeChange",
    customSplitAmount: "onCustomSplitAmountInput", customSplitUnit: "onCustomSplitUnitChange",
  };
  function edit(kind, value) {
    const result = create.edit({ kind, value }, context, () => {});
    const surface = session.surface("classic");
    if (kind === "format") surface.formats.find((item) => item.id === value).onSelect();
    else surface.protection[callbacks[kind]](value);
    const protection = session.surface("modern").protection;
    for (const field of Object.keys(callbacks)) assert.equal(protection[field], create.state[field], field);
    assert.equal(session.surface("modern").formats.find((item) => item.selected).id, create.state.format);
    return result;
  }
  edit("encryptionEnabled", true);
  assert.equal(create.passwordIssue, "password_required");
  assert.match(session.surface("modern").protection.passwordError, /new destination password/);
  edit("password", "destination-secret");
  edit("passwordConfirmation", "different-secret");
  assert.equal(create.passwordIssue, "passwords_do_not_match");
  assert.match(session.surface("modern").protection.passwordError, /do not match/);
  edit("passwordConfirmation", "destination-secret");
  edit("passwordVisible", true);
  edit("encryptNames", true);
  assert.equal(edit("format", "zip").issue, "name_encryption_format_change");
  assert.match(calls.at(-1)[1], /Turn off file name encryption/);
  assert.equal(edit("format", "tar.zst").issue, "encryption_format_change");
  edit("encryptNames", false);
  edit("format", "zip");
  edit("splitPreset", "custom");
  edit("splitMode", "native");
  edit("customSplitUnit", "gib");
  edit("customSplitAmount", "4");
  assert.equal(create.splitIssue, "native_zip_part_size_limit");
  assert.match(session.surface("modern").protection.splitError, /4 GiB/);
  edit("splitPreset", "4-gib");
  assert.equal(create.splitSize, 0xffff_ffff);
  assert.equal(session.surface("modern").protection.splitError, "");
  edit("password", "");
  edit("encryptionEnabled", false);
  edit("format", "tar.zst");
  assert.equal(edit("splitMode", "native").issue, "native_layout_unavailable");
  edit("splitPreset", "none");
  assert.equal(create.splitSize, null);
});

test("restoring output sizes keeps exact bytes and each screen's preset presentation", (t) => {
  const { session } = harness(t);
  const create = new CreateOptionsDraft({ format: "zip", profile: "balanced", customLevel: 6 });
  const context = { capabilityReady: true, sfxCapability: { available: false } };
  for (const [size, preset] of [[25 * 1024 ** 2, "25-mib"], [0xffff_ffff, "4-gib"], [123456789, "custom"]]) {
    const options = { ...create.snapshotPreset(), volumes: { kind: "split", size_bytes: String(size) } };
    assert.equal(create.applyPreset(options, context, () => {}).changed, true);
    assert.equal(session.restoreTaskDraft(draft({ split_size: size })), true);
    assert.equal(create.state.splitPreset, preset);
    assert.equal(session.surface("modern").protection.splitPreset, "custom");
    assert.equal(create.splitSize, size);
    assert.equal(create.snapshotPreset().volumes.size_bytes, String(size));
  }
  create.edit({ kind: "customSplitAmount", value: "2" }, context, () => {});
  assert.equal(create.splitSize, 2 * 1024 ** 2);
});

test("creation applies output edits after invalidating credentials and retains its SFX guards", () => {
  const create = new CreateOptionsDraft({ format: "zip", profile: "balanced", customLevel: 6 });
  const context = { capabilityReady: true, sfxCapability: { available: true, target: "macos", extension: "app" } };
  const invalidate = () => create.resetCredentials(null);
  create.edit({ kind: "password", value: "new-secret" }, context, invalidate);
  assert.equal(create.state.password, "new-secret");
  assert.equal(create.state.encryptionEnabled, true);
  create.edit({ kind: "passwordVisible", value: true }, context, invalidate);
  assert.equal(create.state.password, "new-secret");
  create.edit({ kind: "splitPreset", value: "100-mib" }, context, invalidate);
  assert.equal(create.state.password, "");
  create.edit({ kind: "sfxEnabled", value: true }, context, invalidate);
  assert.equal(create.state.splitPreset, "none");
  assert.equal(create.outputExtension("app"), "app");
  assert.equal(create.nativeSplitKind, null);
  assert.equal(create.edit({ kind: "format", value: "7z" }, context, invalidate).issue, "sfx_zip_only");
  assert.equal(create.edit({ kind: "splitPreset", value: "25-mib" }, context, invalidate).issue, "sfx_no_split");
  assert.equal(create.edit({ kind: "splitMode", value: "native" }, context, invalidate).issue, "native_layout_unavailable");
  assert.equal(create.edit({ kind: "customLevel", value: "" }, context, invalidate).issue, "custom_level_invalid");
  assert.equal(create.state.customLevel, 6);
  create.edit({ kind: "customLevel", value: "9" }, context, invalidate);
  assert.equal(create.compressionLevel, 9);
  assert.equal(create.state.customLevelInvalid, false);
});
