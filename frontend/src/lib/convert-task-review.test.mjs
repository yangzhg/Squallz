import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { convertSessionFor } = await server.ssrLoadModule("/src/lib/convert-session.svelte.ts");
const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");

const originalArchive = { id: 1, path: "/original/photos.7z", source: "/original/photos.7z",
  name: "photos.7z", format: "7z", entry_count: 3, garbled_count: 0, non_utf8_name_count: 0,
  encoding_override: "gbk" };
function draft(overrides = {}) {
  return { src: originalArchive.source, dest: "/output/Original.ZIP", src_encoding: "gbk",
    level: 4, encrypt_names: false, split_size: 123456789, split_mode: "native", ...overrides };
}

function harness(t) {
  const calls = [];
  let archive = { ...originalArchive };
  const bridge = {
    getArchive: () => archive, tr: (_key, fallback) => fallback, tError: () => "Read/write error",
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
  assert.equal(session.surface("modern").protection.passwordError, "");
});
