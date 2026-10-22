import { jobResult } from "../../tests/job-results.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function archiveInfo(extension = "zip") {
  const path = `/archives/A.${extension}`;
  return { id: 1, path, source: path, name: `A.${extension}`, format: extension,
    read_only: false, encoding_override: null, volumes: null };
}

async function withSession(run) {
  const server = await createTestServer();
  try {
    const { RecoverySession } = await server.ssrLoadModule("/src/lib/recovery-session.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const history = await server.ssrLoadModule("/src/lib/history.svelte.ts");
    const state = { active: true, mode: "modern", generation: 0, archive: archiveInfo(),
      tasks: [], selection: null, platform: "macos", level: 2, profile: "balanced" };
    const notices = [], submitted = [], native = [];
    const dialogs = { open: async () => null, save: async () => null, confirm: async () => true };
    const services = {
      read: () => state,
      dialogs: async () => dialogs,
      openDialog: (purpose, open, options) => { native.push({ purpose, ...options }); return open(options); },
      saveDialog: (purpose, save, options) => { native.push({ purpose, ...options }); return save(options); },
      submit: async (spec) => { submitted.push(structuredClone(spec)); return submitted.length; },
      blocked: () => false, focusBlockingTask: () => false,
      notice: (message) => notices.push(message),
      extension: (name) => name.replace(/\.\d{3,}$/, "").split(".").at(-1),
      stem: (name) => name.replace(/\.\d{3,}$/, "").replace(/\.[^.]+$/, ""),
      extractDestination: (path) => path.replace(/\.[^.]+$/, ""),
      mutationDisabledReason: () => "Read-only archive",
      outputFilterName: (format) => format, inspectionId: () => "recovery-output-check",
    };
    ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
    ipc.cancelCreateDestinationInspection = async () => true;
    const session = new RecoverySession(services);
    const navigate = (active) => { if (state.active !== active) session.leave(); state.active = active; };
    const prepare = (extension = "zip", count = 1) => {
      state.archive = archiveInfo(extension);
      const source = state.archive.path;
      session.selectSet(`${source}.par2`, source);
      state.tasks = [{ id: 1, state: "done", spec: { kind: "verify_recovery", path: source, recovery: `${source}.par2` },
        result: jobResult("verify_recovery", { operation: "verify", ok: false, source_file_count: count,
          metrics: { repair_possible: true, blocks_needed: 1, recovery_blocks_available: 3 } }) }];
      state.level = 2;
      state.profile = "balanced";
      services.dialogs = async () => dialogs;
      dialogs.open = async () => null;
      dialogs.save = async () => null;
      dialogs.confirm = async () => true;
      ipc.inspectCreateDestination = async () => ({ conflict: false, guard: null });
      return source;
    };
    await run({ session, state, services, dialogs, notices, submitted, native, ipc, history, prepare, navigate });
    session.dispose();
  } finally {
    await server.close();
  }
}

test("recovery owns source routing, exact report binding and authoritative inner archive handles", async () => {
  await withSession(async ({ session, state, notices, prepare, submitted }) => {
    assert.equal(session.sourcePath(), null);
    session.open();
    assert.equal(session.usesCurrentArchive(), true);
    session.selectSource("/selected/B.zip", "/selected/B.par2");
    session.open("preserve");
    assert.equal(session.sourcePath(), "/selected/B.zip");
    assert.equal(session.sidecarPath(), "/selected/B.par2");
    session.open("current");
    assert.equal(session.sourcePath(), state.archive.path);
    assert.equal(session.sidecarPath(), `${state.archive.path}.par2`);

    prepare("zip", 4);
    const matching = state.tasks[0];
    state.tasks.push({ ...matching, spec: { ...matching.spec, path: "/other/A.zip" }, result: jobResult("verify_recovery", { ok: true }) });
    state.tasks.push({ ...matching, spec: { ...matching.spec, recovery: "/other/A.par2" }, result: jobResult("verify_recovery", { ok: true }) });
    assert.equal(session.latestReportTask(), matching);
    assert.equal(session.view().protectedSourceCount, 4);
    assert.match(session.view().repairOutputMode, /all 4 protected files/);
    assert.equal(session.view().repairDisabledReason, "");
    assert.equal(session.view().metrics.remainingMargin, "2");
    state.tasks.push({ ...matching, result: jobResult("verify_recovery", { operation: "verify", ok: false,
      metrics: { repair_possible: false, blocks_needed: 7, recovery_blocks_available: 3 } }) });
    assert.equal(session.view().beyondCapacity, true);
    assert.match(session.view().repairDisabledReason, /more damage/);

    state.archive = { ...archiveInfo(), source: "squallz-archive://7", path: "/outer.zip › inner.zip", read_only: true };
    const actual = { id: 7, spec: { kind: "extract", path: state.archive.source, best_effort: true } };
    const displayed = { ...actual.spec, path: state.archive.path };
    assert.equal(session.adoptTask(actual, displayed), true);
    assert.equal(session.usesCurrentArchive(), true);
    assert.equal(session.sourcePath(), displayed.path);
    assert.equal(session.jobSource(), actual.spec.path);
    assert.equal(session.usesTaskContext({ ...actual, spec: { ...actual.spec, kind: "test" } }), false);
    state.selection = ["chosen.txt"];
    await session.extractReadable();
    assert.equal(submitted[0].path, actual.spec.path);
    assert.deepEqual(submitted[0].selection, ["chosen.txt"]);
    assert.equal(submitted[0].overwrite, "rename");
    assert.equal(submitted[0].best_effort, true);
    state.archive = archiveInfo();
    session.selectSource("/kept.zip");
    assert.equal(session.adoptTask(actual, displayed), false);
    assert.equal(session.sourcePath(), "/kept.zip");
    assert.match(notices.at(-1), /Reopen the original archive/);
  });
});

test("recovery pickers discard every departed module, selection, cancellation and failure", async () => {
  await withSession(async ({ session, state, services, notices, native, navigate }) => {
    const select = (kind) => kind === "archive" ? session.chooseArchive() : session.choosePar2();
    for (const kind of ["archive", "par2"]) {
      for (const phase of ["loading", "choosing"]) {
        for (const result of ["selected", "cancelled", "failure"]) {
          session.selectSet("/previous/A.par2", "/previous/A.zip");
          const pending = deferred(), started = deferred();
          services.dialogs = phase === "loading" ? () => { started.resolve(); return pending.promise; }
            : async () => ({ open: () => { started.resolve(); return pending.promise; } });
          const beforeNative = native.length;
          const work = select(kind);
          await started.promise;
          assert.equal(session.picker, kind);
          navigate(false);
          navigate(true);
          assert.equal(session.picker, "idle");
          const beforeNotice = notices.length;
          if (result === "failure") pending.reject(new Error("late picker failure"));
          else if (phase === "loading") pending.resolve({ open: async () => result === "selected" ? "/late/old.zip" : null });
          else pending.resolve(result === "selected" ? ["/late/old.zip"] : null);
          await work;
          assert.equal(session.sourcePath(), "/previous/A.zip");
          assert.equal(session.sidecarPath(), "/previous/A.par2");
          assert.equal(session.picker, "idle");
          assert.equal(notices.length, beforeNotice);
          assert.equal(native.length - beforeNative, phase === "loading" ? 0 : 1);
          navigate(false);
          await select(kind);
          assert.equal(native.length - beforeNative, phase === "loading" ? 0 : 1);
          navigate(true);
        }
      }
    }
    for (const action of ["new-set", "current", "default", "task", "dispose"]) {
      session.selectSet("/previous/A.par2", "/previous/A.zip");
      const old = deferred(), started = deferred();
      services.dialogs = async () => ({ open: () => { started.resolve(); return old.promise; } });
      const work = session.chooseArchive();
      await started.promise;
      if (action === "new-set") session.selectSet("/next/B.par2", "/next/B.zip");
      if (action === "current") session.useCurrentArchive();
      if (action === "default") session.useDefaultPar2();
      if (action === "task") assert.equal(session.adoptTask({ spec: { kind: "repair_zip", src: "/next/B.zip" } }), true);
      if (action === "dispose") session.dispose();
      const kept = [session.sourcePath(), session.sidecarPath(), notices.length];
      old.resolve("/late/old.zip");
      await work;
      assert.deepEqual([session.sourcePath(), session.sidecarPath(), notices.length], kept);
      assert.equal(session.picker, "idle");
    }
    assert.equal(state.archive.path, "/archives/A.zip");
  });
});

test("current pickers retain feedback and old cleanup cannot release a newer picker", async () => {
  await withSession(async ({ session, services, notices, dialogs }) => {
    for (const kind of ["archive", "par2"]) {
      for (const result of ["selected", "cancelled", "failure"]) {
        session.selectSet("/previous/A.par2", "/previous/A.zip");
        services.dialogs = async () => dialogs;
        dialogs.open = async () => {
          if (result === "failure") throw new Error("current picker failure");
          return result === "selected" ? ["/selected/B.zip"] : null;
        };
        const count = notices.length;
        await (kind === "archive" ? session.chooseArchive() : session.choosePar2());
        assert.equal(notices.length, count + 2);
        assert.equal(session.sourcePath(), result === "selected" && kind === "archive" ? "/selected/B.zip" : "/previous/A.zip");
        assert.equal(session.sidecarPath(), result === "selected" && kind === "par2" ? "/selected/B.zip" : "/previous/A.par2");
      }
    }
    const first = deferred(), second = deferred(), firstStarted = deferred(), secondStarted = deferred();
    services.dialogs = async () => ({ open: () => { firstStarted.resolve(); return first.promise; } });
    const old = session.chooseArchive();
    await firstStarted.promise;
    session.selectSource("/next/B.zip");
    services.dialogs = async () => ({ open: () => { secondStarted.resolve(); return second.promise; } });
    const current = session.choosePar2();
    await secondStarted.promise;
    const count = notices.length;
    first.reject(new Error("old picker"));
    await old;
    assert.equal(session.picker, "par2");
    assert.equal(notices.length, count);
    second.resolve("/selected/B.par2");
    await current;
    assert.equal(session.picker, "idle");
    assert.equal(session.sidecarPath(), "/selected/B.par2");
  });
});

test("recovery output stages discard late modules, saves, inspections, approvals and unique directories", async () => {
  await withSession(async ({ session, services, dialogs, notices, submitted, native, ipc, prepare, navigate, history }) => {
    for (const stage of ["module", "save", "inspection", "confirmation", "unique"]) {
      prepare(stage === "inspection" || stage === "confirmation" ? "sqz" : "zip", stage === "unique" ? 2 : 1);
      const kind = stage === "inspection" || stage === "confirmation" ? "export_sqz" : "repair_recovery";
      if (kind === "export_sqz") session.useCurrentArchive();
      const pending = deferred(), started = deferred();
      if (stage === "module") services.dialogs = () => { started.resolve(); return pending.promise; };
      dialogs.save = async () => "/output/A.zip";
      if (stage === "save") dialogs.save = () => { started.resolve(); return pending.promise; };
      if (stage === "inspection") ipc.inspectCreateDestination = () => { started.resolve(); return pending.promise; };
      if (stage === "confirmation") {
        ipc.inspectCreateDestination = async () => ({ conflict: true, guard: "existing-version" });
        dialogs.confirm = () => { started.resolve(); return pending.promise; };
      }
      if (stage === "unique") {
        dialogs.open = async () => "/output";
        ipc.uniqueCreateDestination = (candidate) => { started.resolve(candidate); return pending.promise; };
      }
      const work = session.submitOutput(kind);
      await started.promise;
      const request = session.preparation;
      const nativeCount = native.length;
      await session.submitOutput(kind);
      assert.equal(session.preparation, request);
      assert.equal(native.length, nativeCount);
      if (stage === "save") { navigate(false); navigate(true); }
      else if (stage === "inspection") {
        dialogs.open = async () => "/recovery/B.par2";
        await session.choosePar2();
      } else if (stage === "unique") {
        assert.equal(session.adoptTask({ spec: { kind: "repair_zip", src: "/recovery/B.zip" } }), true);
      } else session.selectSet("/recovery/B.par2", "/recovery/B.zip");
      assert.equal(session.preparation, null);
      const count = notices.length, historyCount = history.operationHistory().length;
      if (stage === "module") pending.resolve(dialogs);
      if (stage === "save" || stage === "unique") pending.resolve("/output/old.zip");
      if (stage === "inspection") pending.resolve({ conflict: true, guard: "old-version" });
      if (stage === "confirmation") pending.resolve(true);
      await work;
      assert.equal(submitted.length, 0);
      assert.equal(notices.length, count);
      assert.equal(history.operationHistory().length, historyCount);
      assert.equal(native.length, nativeCount + (stage === "inspection" ? 1 : 0));
    }
  });
});

test("recovery output authorization preserves all four operation snapshots and no-replace boundaries", async () => {
  await withSession(async ({ session, state, dialogs, submitted, notices, ipc, prepare, history }) => {
    for (const kind of ["repair_sqz", "repair_zip", "repair_recovery", "export_sqz"]) {
      const source = prepare(kind === "repair_sqz" || kind === "export_sqz" ? "sqz" : "zip");
      if (kind === "export_sqz") session.useCurrentArchive();
      const save = deferred(), started = deferred();
      dialogs.save = () => { started.resolve(); return save.promise; };
      const work = session.submitOutput(kind);
      await started.promise;
      const request = session.preparation;
      state.level = 9;
      state.profile = "fast";
      session.open("preserve");
      assert.equal(session.preparation, request);
      const destination = `/output/${kind}.${kind === "repair_sqz" ? "sqz" : "zip"}`;
      save.resolve(destination);
      await work;
      const spec = submitted.at(-1);
      assert.equal(spec.kind, kind);
      assert.equal(session.preparation, null);
      if (kind === "repair_recovery") {
        assert.equal(spec.path, source);
        assert.equal(spec.output, destination);
        assert.equal(spec.output_directory, false);
        assert.equal(spec.recovery, `${source}.par2`);
      } else {
        assert.equal(spec.src, source);
        assert.equal(spec.dest, destination);
        assert.equal(spec.level, 2);
      }
      if (kind === "export_sqz") {
        assert.equal(spec.replace_existing, false);
        assert.equal(spec.replacement_guard, null);
        assert.match(history.operationHistory()[0].detail, /balanced/);
      }
    }
    assert.equal(submitted.length, 4);
    prepare("zip");
    dialogs.save = async () => "/output/existing.zip";
    ipc.inspectCreateDestination = async () => ({ conflict: true, guard: "existing-version" });
    await session.submitOutput("repair_zip");
    assert.equal(submitted.length, 4);
    assert.match(notices.at(-1), /already exists.*Choose a different name or folder/);
    dialogs.save = async () => state.archive.path;
    await session.submitOutput("repair_zip");
    assert.equal(submitted.length, 4);
    assert.match(notices.at(-1), /source archive will not be overwritten/);
    prepare("sqz");
    session.useCurrentArchive();
    dialogs.save = async () => "/output/replace.zip";
    ipc.inspectCreateDestination = async () => ({ conflict: true, guard: "accepted-version" });
    dialogs.confirm = async () => true;
    await session.submitOutput("export_sqz");
    assert.equal(submitted.at(-1).replace_existing, true);
    assert.equal(submitted.at(-1).replacement_guard, "accepted-version");
  });
});

test("old recovery output cleanup cannot release a new request and current failures retain feedback", async () => {
  await withSession(async ({ session, services, dialogs, notices, submitted, prepare }) => {
    prepare();
    const first = deferred(), second = deferred(), firstStarted = deferred(), secondStarted = deferred();
    dialogs.save = () => { firstStarted.resolve(); return first.promise; };
    const old = session.submitOutput("repair_zip");
    await firstStarted.promise;
    session.selectSource("/next/B.zip");
    dialogs.save = () => { secondStarted.resolve(); return second.promise; };
    const current = session.submitOutput("repair_zip");
    await secondStarted.promise;
    const request = session.preparation, count = notices.length;
    first.reject(new Error("old chooser failure"));
    await old;
    assert.equal(session.preparation, request);
    assert.equal(request.phase, "choosing");
    assert.equal(notices.length, count);
    second.resolve(null);
    await current;
    assert.equal(session.preparation, null);
    assert.match(notices.at(-1), /ZIP index rebuild cancelled/);
    services.dialogs = async () => { throw new Error("current chooser failure"); };
    await session.submitOutput("repair_zip");
    assert.equal(session.preparation, null);
    assert.equal(submitted.length, 0);
    assert.match(notices.at(-1), /Check the save location and try again/);
  });
});

test("only accepted source changes invalidate pickers and unchanged routes keep their request", async () => {
  await withSession(async ({ session, state, services, notices }) => {
    for (const action of ["preserve", "missing-current", "missing-default", "unavailable-task", "current-route"]) {
      session.selectSource(action === "missing-default" || action === "current-route" ? null : "/previous/A.zip");
      const pending = deferred(), started = deferred();
      services.dialogs = async () => ({ open: () => { started.resolve(); return pending.promise; } });
      const work = session.chooseArchive();
      await started.promise;
      if (action === "preserve") session.open("preserve");
      if (action === "current-route") session.open("preserve");
      if (action === "missing-current") {
        const archive = state.archive;
        state.archive = null;
        session.useCurrentArchive();
        state.archive = archive;
      }
      if (action === "missing-default") session.useDefaultPar2();
      if (action === "unavailable-task") assert.equal(session.adoptTask({ spec: { kind: "repair_zip", src: "squallz-archive://7" } },
        { kind: "repair_zip", src: "/displayed/outer.zip" }), false);
      const count = notices.length;
      pending.resolve("/selected/B.zip");
      await work;
      assert.equal(notices.length, count + (action === "current-route" ? 0 : 1));
      assert.equal(session.sourcePath(), action === "current-route" ? state.archive.path : "/selected/B.zip");
    }
  });
});

test("layout or current-archive generation changes cancel inspection ownership and discard late approval", async () => {
  await withSession(async ({ session, state, dialogs, ipc, notices, submitted, prepare }) => {
    for (const change of ["mode", "generation"]) {
      prepare("sqz");
      session.useCurrentArchive();
      const pending = deferred(), started = deferred(), cancelled = [];
      dialogs.save = async () => "/output/A.zip";
      dialogs.confirm = async () => { throw new Error("obsolete confirmation must not open"); };
      ipc.inspectCreateDestination = () => { started.resolve(); return pending.promise; };
      ipc.cancelCreateDestinationInspection = async (id) => { cancelled.push(id); };
      const work = session.submitOutput("export_sqz");
      await started.promise;
      if (change === "mode") state.mode = "classic";
      else state.generation += 1;
      session.syncContext();
      assert.equal(session.preparation, null);
      assert.deepEqual(cancelled, ["recovery-output-check"]);
      const count = notices.length;
      pending.resolve({ conflict: true, guard: "old-version" });
      await work;
      assert.equal(submitted.length, 0);
      assert.equal(notices.length, count);
    }
  });
});

test("recovery submission has one owner and late completion cannot clear a newer request", async () => {
  await withSession(async ({ session, services, notices, history, prepare }) => {
    prepare();
    session.useCurrentArchive();
    const first = deferred(), second = deferred();
    let calls = 0;
    services.submit = () => { calls += 1; return calls === 1 ? first.promise : second.promise; };
    const old = session.testArchive();
    assert.match(session.view().preparationMessage, /Adding recovery task/);
    await session.verify();
    assert.equal(calls, 1);
    session.selectSource("/next/B.zip");
    const current = session.testArchive();
    assert.equal(calls, 2);
    const count = notices.length;
    first.resolve(10);
    await old;
    assert.equal(notices.length, count);
    assert.match(session.view().preparationMessage, /Adding recovery task/);
    assert.equal(history.operationHistory()[0].title, "Archive test added to queue");
    assert.equal(session.usesTaskContext({ id: 10, spec: { kind: "test", path: "/archives/A.zip" } }), true);
    second.resolve(11);
    await current;
    assert.equal(session.view().preparationMessage, "");
    assert.match(notices.at(-1), /Archive test added/);
    session.dispose();
    await session.testArchive();
    assert.equal(calls, 2);
  });
});
