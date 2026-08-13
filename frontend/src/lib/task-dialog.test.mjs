import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function sfxTask(overrides = {}) {
  return {
    id: 42,
    title: "Create self-extractor Installer.app",
    state: "done",
    spec: {
      kind: "compress",
      inputs: ["/Users/alex/Reports"],
      dest: "/Users/alex/Exports/Installer.app",
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: null,
      split_mode: "generic",
      excludes: [],
      sfx_target: "macos",
      replace_existing: true,
      completion: "none",
      post_success: "keep_source",
    },
    done: 48_000_000,
    total: 48_000_000,
    current: "",
    currentDone: 0,
    currentTotal: 0,
    scanEntries: null,
    speed: 0,
    phase: null,
    interruptible: true,
    pausable: true,
    error: null,
    result: {
      operation: "create_sfx",
      primary_output: "/Users/alex/Exports/Installer.app",
      outputs: ["/Users/alex/Exports/Installer.app"],
      preserved_outputs: [
        "/Users/alex/Exports/.squallz-sfx-holder-42-1/previous",
      ],
      total_bytes: 48_000_000,
      volume_count: 1,
      split: false,
    },
    revealPath: "/Users/alex/Exports/Installer.app",
    historyRecorded: true,
    controlIntent: null,
    expanded: true,
    ...overrides,
  };
}

test("SFX result and recovery details use the durable single-backup contract", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { loadLocale } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const {
      taskDialogResultSummary,
      taskCurrentProgressBadge,
      taskCurrentProgressSource,
      taskCurrentProgressSummary,
      taskCurrentSectionVisible,
      taskCurrentSectionLabel,
      taskNextStepDetail,
      taskOverallProgressBadge,
      taskOverallProgressIndeterminate,
      taskOverallProgressLabel,
      taskOpenOutputLabel,
      taskOutputCanOpen,
      taskOutcomeNeedsAttention,
      taskPhaseControlNoticeDetail,
      taskPhaseControlNoticeTitle,
      taskPhaseControlNoticeVisible,
      taskProgressPercent,
      taskProgressSummary,
      taskResultDetailRows,
      taskResultDetailTitle,
      taskResultActionLabel,
      taskHasInlineResults,
      taskKindLabel,
      taskResultAvailableForSurface,
    } = await server.ssrLoadModule("/src/lib/task-dialog.ts");
    const { taskCanPublishMacosSfx } = await server.ssrLoadModule(
      "/src/lib/macos-sfx-publish.ts",
    );
    await loadLocale("en-US");

    const completed = sfxTask();
    assert.equal(taskCanPublishMacosSfx(completed), true);
    assert.equal(
      taskDialogResultSummary(completed),
      "Created Installer.app (45.8 MB) · unsigned · Review 1 preserved backup",
    );
    assert.equal(taskOutcomeNeedsAttention(completed), true);
    assert.equal(
      taskNextStepDetail(completed, false),
      "Test the new app and review the preserved previous output. Keep that backup until the app passes testing, signing, and notarization.",
    );
    assert.ok(
      taskResultDetailRows(completed).some(
        (row) => row.value === "/Users/alex/Exports/.squallz-sfx-holder-42-1/previous",
      ),
    );
    assert.equal(
      taskCanPublishMacosSfx(sfxTask({
        result: {
          ...completed.result,
          requires_signing: false,
        },
      })),
      false,
    );

    const published = {
      ...completed,
      title: "Publish Installer-published.app",
      spec: {
        kind: "publish_macos_sfx",
        source: "/Users/alex/Exports/Installer.app",
        output: "/Users/alex/Exports/Installer-published.app",
        identity: "Developer ID Application: Acme Studio (A1B2C3D4E5)",
        notary_profile: "release-notary",
      },
      result: {
        operation: "sfx_publish_macos",
        primary_output: "/Users/alex/Exports/Installer-published.app",
        outputs: ["/Users/alex/Exports/Installer-published.app"],
        team_id: "A1B2C3D4E5",
        submission_id: "019f5b43-a83a-7144-a11b-ccaa5106fb0e",
        notarization: "Accepted",
        stapled: true,
        source_preserved: true,
        requires_signing: false,
      },
      revealPath: "/Users/alex/Exports/Installer-published.app",
    };
    assert.equal(taskCanPublishMacosSfx(published), false);
    assert.equal(taskKindLabel(published), "Trusted macOS app");
    assert.equal(taskHasInlineResults(published), true);
    assert.equal(taskResultAvailableForSurface(published, false), true);
    assert.equal(taskResultDetailTitle(published), "Published app");
    assert.equal(
      taskDialogResultSummary(published),
      "Published Installer-published.app · Developer ID · Team A1B2C3D4E5",
    );
    assert.equal(taskOutputCanOpen(published), true);
    assert.equal(taskOpenOutputLabel(published), "Open published app");
    assert.equal(
      taskNextStepDetail(published, false),
      "Distribute the verified app. Keep the unsigned source if you may need to rebuild or publish it again.",
    );
    const publishedDetails = taskResultDetailRows(published);
    assert.deepEqual(
      publishedDetails.map((row) => row.label),
      [
        "Output",
        "Signature",
        "Apple notarization",
        "Team ID",
        "Submission ID",
        "Unsigned source",
      ],
    );
    assert.doesNotMatch(
      publishedDetails.map((row) => row.value).join("\n"),
      /release-notary|Acme Studio/u,
    );
    assert.equal(
      taskCanPublishMacosSfx({
        ...published,
        state: "failed",
        result: null,
        revealPath: null,
      }),
      true,
    );

    const converted = sfxTask({
      spec: {
        kind: "convert",
        src: "/Users/alex/Backups/source.zip",
        dest: "/Users/alex/Backups/converted.7z",
        level: 6,
        src_encoding: null,
        src_password: null,
        dest_password: null,
        encrypt_names: false,
        split_size: 100 * 1024 ** 2,
        split_mode: "generic",
        replace_existing: false,
        replacement_guard: null,
      },
      result: {
        operation: "convert",
        primary_output: "/Users/alex/Backups/converted.7z.001",
        outputs: [
          "/Users/alex/Backups/converted.7z.001",
          "/Users/alex/Backups/converted.7z.002",
        ],
        preserved_outputs: [],
        total_bytes: 150_000_000,
        volume_count: 2,
        split: true,
      },
      revealPath: "/Users/alex/Backups/converted.7z.001",
    });
    assert.equal(taskHasInlineResults(converted), true);
    assert.equal(taskResultDetailTitle(converted), "Converted output");
    assert.equal(
      taskDialogResultSummary(converted),
      "Converted to converted.7z.001 · 2 volumes · 143.1 MB",
    );
    assert.equal(taskOutputCanOpen(converted), false);
    assert.equal(taskOpenOutputLabel(converted), "Open converted archive");
    assert.equal(taskOutcomeNeedsAttention(converted), false);
    assert.equal(
      taskNextStepDetail(converted, false),
      "Keep every numbered volume in the same folder. Reveal the output set before sharing or extracting it.",
    );
    assert.ok(
      taskResultDetailRows(converted).some(
        (row) => row.label === "File list" && row.value.includes("converted.7z.002"),
      ),
    );

    const recoveryIndex = "/Users/alex/Backups/product-backup.zip.par2";
    const protectedSet = sfxTask({
      spec: {
        kind: "protect",
        path: "/Users/alex/Backups/product-backup.zip",
        redundancy: 10,
        recovery: recoveryIndex,
      },
      result: {
        operation: "protect",
        ok: true,
        recovery: recoveryIndex,
        outputs: [
          recoveryIndex,
          "/Users/alex/Backups/product-backup.zip.vol00+01.par2",
          "/Users/alex/Backups/product-backup.zip.vol01+02.par2",
        ],
        source_file_count: 1,
      },
      revealPath: recoveryIndex,
    });
    assert.equal(taskHasInlineResults(protectedSet), true);
    assert.equal(taskResultDetailTitle(protectedSet), "PAR2 files created");
    assert.equal(
      taskDialogResultSummary(protectedSet),
      "Created 3 PAR2 files · product-backup.zip.par2",
    );
    assert.equal(taskOutputCanOpen(protectedSet), false);
    assert.equal(taskResultActionLabel(protectedSet), "Hide results");
    assert.equal(
      taskResultActionLabel({ ...protectedSet, expanded: false }),
      "View results",
    );
    assert.equal(
      taskNextStepDetail(protectedSet, false),
      "Keep every file in this PAR2 set together. Reveal the complete set before moving or sharing it.",
    );
    const protectedRows = taskResultDetailRows(protectedSet);
    assert.ok(
      protectedRows.some(
        (row) => row.label === "Output files" && row.value === "3",
      ),
    );
    assert.ok(
      protectedRows.some(
        (row) => row.label === "File list"
          && row.value.includes("product-backup.zip.vol01+02.par2"),
      ),
    );

    const convertedWithBackup = {
      ...converted,
      result: {
        ...converted.result,
        preserved_outputs: ["/Users/alex/Backups/.converted.7z.001.split-backup"],
      },
    };
    assert.equal(taskOutcomeNeedsAttention(convertedWithBackup), true);
    assert.equal(
      taskDialogResultSummary(convertedWithBackup),
      "Converted to converted.7z.001 · Review 1 preserved backup",
    );

    const failedRecovery = sfxTask({
      spec: {
        kind: "repair_recovery",
        path: "/Users/alex/Samples/damaged.zip",
        output: "/Users/alex/Samples/damaged.repaired.zip",
        recovery: "/Users/alex/Samples/damaged.zip.par2",
      },
      result: {
        operation: "repair",
        ok: false,
        output: "/Users/alex/Samples/damaged.repaired.zip",
        metrics: {
          repair_possible: true,
          blocks_needed: 3,
          recovery_blocks_available: 8,
        },
      },
      revealPath: null,
    });
    assert.equal(
      taskResultDetailRows(failedRecovery).some((row) => row.label === "Output"),
      false,
    );

    const outputConflict = sfxTask({
      state: "failed",
      spec: failedRecovery.spec,
      result: null,
      revealPath: null,
      error: {
        key: "error.output_exists",
        params: {},
        detail: "archive output already exists",
      },
    });
    assert.equal(taskDialogResultSummary(outputConflict), "Output Location Is Occupied");
    assert.equal(
      taskNextStepDetail(outputConflict, false),
      "Choose a different output name. Squallz left the existing item unchanged.",
    );

    const repairWorkspace =
      "/Users/alex/Samples/.damaged.repaired.zip.sqz-par2-repair-42-9.work";
    const repairJournal =
      "/Users/alex/Samples/.squallz-par2-repair-8f3d4a9e1c7b2d5f.json";
    const cleanupReady = sfxTask({
      state: "failed",
      spec: failedRecovery.spec,
      result: null,
      revealPath: null,
      error: {
        key: "error.recovery_cleanup_output_ready",
        params: {
          target: failedRecovery.spec.output,
          workspace: repairWorkspace,
          journal: repairJournal,
        },
        detail: "The repaired copy is ready, but cleanup failed.",
      },
    });
    assert.equal(
      taskDialogResultSummary(cleanupReady),
      `The repaired copy is ready at ${failedRecovery.spec.output}, but its private workspace could not be removed`,
    );
    const cleanupRows = taskResultDetailRows(cleanupReady);
    assert.ok(cleanupRows.some((row) => row.value === failedRecovery.spec.output));
    assert.ok(cleanupRows.some((row) => row.value === repairWorkspace));
    assert.ok(cleanupRows.some((row) => row.value === repairJournal));
    assert.match(taskNextStepDetail(cleanupReady, false), /Test the repaired copy/u);
    assert.match(taskNextStepDetail(cleanupReady, false), /Do not delete adjacent hidden folders/u);

    const cleanupUnconfirmed = {
      ...cleanupReady,
      error: {
        ...cleanupReady.error,
        key: "error.recovery_cleanup_unconfirmed",
      },
    };
    assert.equal(
      taskDialogResultSummary(cleanupUnconfirmed),
      `PAR2 repair at ${failedRecovery.spec.output} needs cleanup before it can be retried safely`,
    );
    assert.match(
      taskNextStepDetail(cleanupUnconfirmed, false),
      /late filesystem error may have left an output/u,
    );
    assert.match(taskNextStepDetail(cleanupUnconfirmed, false), /remove only that workspace/u);

    const damagedCleanupRecord = {
      ...cleanupReady,
      error: {
        key: "error.recovery_cleanup_record",
        params: {
          target: failedRecovery.spec.output,
          journal: repairJournal,
        },
        detail: "The target-bound recovery record is damaged.",
      },
    };
    assert.equal(
      taskDialogResultSummary(damagedCleanupRecord),
      `The automatic PAR2 recovery record for ${failedRecovery.spec.output} needs attention`,
    );
    const damagedRecordRows = taskResultDetailRows(damagedCleanupRecord);
    assert.ok(damagedRecordRows.some((row) => row.value === repairJournal));
    assert.equal(damagedRecordRows.some((row) => row.value === repairWorkspace), false);
    assert.match(
      taskNextStepDetail(damagedCleanupRecord, false),
      /no workspace path was trusted/u,
    );

    const changedDestination = sfxTask({
      state: "failed",
      spec: failedRecovery.spec,
      result: null,
      revealPath: null,
      error: {
        key: "error.destination_changed",
        params: {},
        detail: "destination changed after replacement confirmation",
      },
    });
    assert.equal(taskDialogResultSummary(changedDestination), "Destination Changed");
    assert.equal(
      taskNextStepDetail(changedDestination, false),
      "Review the current destination and start again, or choose a different output. Squallz did not write to it.",
    );

    const changedInput = sfxTask({
      state: "failed",
      spec: {
        kind: "extract",
        path: "/Users/alex/Downloads/Photos.zip",
        dest: "/Users/alex/Downloads/Photos",
        selection: ["original.jpg"],
        overwrite: "rename",
        symlinks: "preserve",
        smart: true,
        best_effort: false,
        encoding: null,
        password: null,
      },
      result: null,
      revealPath: null,
      error: {
        key: "error.input_changed",
        params: {},
        detail: "archive input changed after extraction preflight",
      },
    });
    assert.equal(taskDialogResultSummary(changedInput), "Archive Changed");
    assert.equal(
      taskNextStepDetail(changedInput, false),
      "Reopen the archive, review the selected files and destination, then start extraction again. Squallz did not extract anything.",
    );

    const missingVolume = sfxTask({
      state: "failed",
      result: null,
      revealPath: null,
      error: {
        key: "gui.error.corrupt.volume_missing",
        params: { name: "backup.7z.004" },
        detail: "required split volume is missing",
      },
    });
    assert.equal(
      taskDialogResultSummary(missingVolume),
      "Volume backup.7z.004 is missing. Keep all volumes in the same folder.",
    );
    assert.equal(
      taskNextStepDetail(missingVolume, false),
      "Volume backup.7z.004 is missing. Keep all volumes in the same folder.",
    );

    const splitWim = sfxTask({
      state: "failed",
      result: null,
      revealPath: null,
      error: {
        key: "error.unsupported_split_wim",
        params: {},
        detail: "split WIM requires joined parts",
      },
    });
    const splitWimAction =
      "This Split WIM stream has no source folder. Open any .swm member from disk and keep every part together.";
    assert.equal(taskDialogResultSummary(splitWim), splitWimAction);
    assert.equal(taskNextStepDetail(splitWim, false), splitWimAction);

    const splitWimCreate = sfxTask({
      state: "failed",
      result: null,
      revealPath: null,
      error: {
        key: "error.unsupported_split_wim_create",
        params: {},
        detail: "creating native Split WIM is not supported",
      },
    });
    const splitWimCreateAction =
      "Creating .swm requires a split size and the Native Split WIM layout.";
    assert.equal(taskDialogResultSummary(splitWimCreate), splitWimCreateAction);
    assert.equal(taskNextStepDetail(splitWimCreate, false), splitWimCreateAction);

    const extracted = sfxTask({
      spec: {
        kind: "extract",
        path: "/Users/alex/Downloads/Photos.zip",
        dest: "/Users/alex/Downloads/Photos",
        selection: null,
        overwrite: "rename",
        symlinks: "preserve",
        smart: true,
        best_effort: true,
        encoding: null,
        password: null,
      },
      result: {
        dest: "/Users/alex/Downloads/Photos",
        best_effort: true,
        skipped: 1,
        problems: ["broken.jpg: CRC mismatch"],
        counts: {
          destination: "/Users/alex/Downloads/Photos",
          selected_entries: 12,
          created: 6,
          directories: 2,
          replaced: 1,
          renamed: 1,
          skipped: 1,
          failed: 1,
          output_bytes: 12_582_912,
        },
      },
      revealPath: "/Users/alex/Downloads/Photos",
    });
    assert.equal(taskOutcomeNeedsAttention(extracted), true);
    assert.equal(taskHasInlineResults(extracted), true);
    assert.equal(taskResultDetailTitle(extracted), "Extraction results");
    assert.equal(
      taskDialogResultSummary(extracted),
      "10 completed · 1 skipped · 1 failed",
    );
    const extractRows = taskResultDetailRows(extracted);
    assert.ok(extractRows.some((row) => row.label === "Selected entries" && row.value === "12"));
    assert.ok(extractRows.some((row) => row.label === "Renamed" && row.value === "1"));
    assert.ok(extractRows.some((row) => row.label === "Data written" && row.value === "12.0 MB"));
    assert.ok(extractRows.some((row) => row.label === "Problem 1" && row.value.includes("CRC mismatch")));

    const boundedProblems = {
      ...extracted,
      result: {
        ...extracted.result,
        problems: Array.from(
          { length: 20 },
          (_, index) => `damaged/item-${index + 1}.bin: checksum mismatch`,
        ),
        problems_total: 30,
        problems_truncated: true,
        counts: {
          ...extracted.result.counts,
          failed: 30,
        },
      },
    };
    const boundedRows = taskResultDetailRows(boundedProblems);
    assert.equal(
      boundedRows.filter((row) => row.label.startsWith("Problem ")).length,
      6,
    );
    assert.ok(
      boundedRows.some(
        (row) => row.label === "More problems" && row.value === "24 more not shown",
      ),
    );

    const boundedTest = {
      ...extracted,
      spec: {
        kind: "test",
        path: "/Users/alex/Downloads/Photos.zip",
        encoding: null,
        password: null,
      },
      result: {
        operation: "test",
        ok: false,
        entries: 42,
        entries_tested: 42,
        problems: 30,
        problem_messages: Array.from(
          { length: 20 },
          (_, index) => `damaged/item-${index + 1}.bin: checksum mismatch`,
        ),
        problems_total: 30,
        problems_truncated: true,
      },
      revealPath: null,
    };
    assert.equal(
      taskDialogResultSummary(boundedTest),
      "Archive test found 30 problem(s)",
    );
    const boundedTestRows = taskResultDetailRows(boundedTest);
    assert.ok(
      boundedTestRows.some(
        (row) => row.label === "Problems" && row.value === "30",
      ),
    );
    assert.equal(
      boundedTestRows.filter((row) => row.label.startsWith("Problem ")).length,
      6,
    );
    assert.ok(
      boundedTestRows.some(
        (row) => row.label === "More problems" && row.value === "24 more not shown",
      ),
    );

    const recoveredZipTest = {
      ...boundedTest,
      result: {
        ...boundedTest.result,
        problems: 1,
        problem_messages: ["core-only structural diagnostic"],
        problems_total: 1,
        problems_truncated: false,
        structure: "zip_local_headers_recovered",
      },
    };
    assert.ok(
      taskResultDetailRows(recoveredZipTest).some(
        (row) => row.label === "Problem 1"
          && row.value === "ZIP central directory is missing or unreadable; entries were recovered from local headers",
      ),
    );
    const recoveredZipExtract = {
      ...extracted,
      result: {
        ...extracted.result,
        skipped: 0,
        problems: [],
        problems_total: 0,
        structure: "zip_local_headers_recovered",
        counts: {
          ...extracted.result.counts,
          skipped: 0,
          failed: 0,
        },
      },
    };
    assert.equal(taskOutcomeNeedsAttention(recoveredZipExtract), true);
    assert.equal(
      taskNextStepDetail(recoveredZipExtract, false),
      "The ZIP index is missing or unreadable. The visible files came from local headers; test the archive and rebuild its index before relying on it.",
    );
    assert.ok(
      taskResultDetailRows(recoveredZipExtract).some(
        (row) => row.label === "ZIP index damaged"
          && row.value === "The ZIP index is missing or unreadable. The visible files came from local headers; test the archive and rebuild its index before relying on it.",
      ),
    );
    await loadLocale("zh-CN");
    const recoveredZipRowsZh = taskResultDetailRows(recoveredZipTest);
    assert.ok(
      recoveredZipRowsZh.some(
        (row) => row.label === "问题 1"
          && row.value === "ZIP 中央目录缺失或无法读取；条目来自本地文件头恢复视图",
      ),
    );
    assert.ok(
      recoveredZipRowsZh.every(
        (row) => !row.value.includes("core-only structural diagnostic"),
      ),
    );
    assert.ok(
      taskResultDetailRows(recoveredZipExtract).some(
        (row) => row.label === "ZIP 索引已损坏"
          && row.value === "ZIP 索引缺失或无法读取。当前文件列表来自本地文件头；请先测试压缩包并重建索引，再将其作为完整压缩包使用。",
      ),
    );
    await loadLocale("en-US");

    const skippedOnlyExtract = {
      ...extracted,
      result: {
        ...extracted.result,
        counts: {
          ...extracted.result.counts,
          failed: 0,
        },
      },
    };
    assert.equal(taskOutcomeNeedsAttention(skippedOnlyExtract), true);
    assert.equal(
      taskDialogResultSummary(skippedOnlyExtract),
      "10 completed · 1 skipped · 0 failed",
    );

    const groupedBatch = {
      ...extracted,
      spec: {
        kind: "batch_extract",
        items: [
          { path: "/Users/alex/sample.part1.rar", dest: "/Users/alex/sample" },
          { path: "/Users/alex/sample.part2.rar", dest: "/Users/alex/sample-part2" },
        ],
        overwrite: "skip",
        symlinks: "preserve",
        smart: false,
      },
      result: {
        operation: "batch_extract",
        archives: 1,
        selected_archives: 2,
        collapsed_volumes: 1,
        extracted: 1,
        failed: 0,
        outputs: [],
        failures: [],
      },
    };
    assert.equal(
      taskDialogResultSummary(groupedBatch),
      "Selected files 2 → archives 1 · 1 extracted · 0 failed",
    );
    const groupedRows = taskResultDetailRows(groupedBatch);
    assert.ok(groupedRows.some((row) => row.label === "Archives" && row.value === "1"));
    assert.ok(groupedRows.some((row) => row.label === "Selected files" && row.value === "2"));

    const recoveredBatch = {
      ...groupedBatch,
      result: {
        ...groupedBatch.result,
        structure: "zip_local_headers_recovered",
        recovered_archives: 1,
        outputs: [
          {
            archive: "/Users/alex/recovered.zip",
            dest: "/Users/alex/recovered",
            structure: "zip_local_headers_recovered",
          },
        ],
      },
    };
    assert.equal(taskOutcomeNeedsAttention(recoveredBatch), true);
    assert.ok(
      taskResultDetailRows(recoveredBatch).some(
        (row) => row.label === "ZIP index damaged"
          && row.value.includes("recovered.zip")
          && row.value.includes("The ZIP index is missing or unreadable."),
      ),
    );

    const legacyExtract = {
      ...extracted,
      result: {
        dest: "/Users/alex/Downloads/Photos",
        best_effort: false,
        skipped: 0,
        problems: [],
      },
    };
    assert.equal(taskHasInlineResults(legacyExtract), false);
    assert.equal(taskResultDetailTitle(legacyExtract), "Result details");
    assert.equal(
      taskDialogResultSummary(legacyExtract),
      "Output: Photos",
    );

    const journal = "/Users/alex/Exports/.squallz-sfx-transaction.json";
    const holder = "/Users/alex/Exports/.squallz-sfx-holder-42-2";
    const failed = sfxTask({
      state: "failed",
      result: null,
      revealPath: null,
      error: {
        key: "error.sfx_recovery",
        params: {
          target: "/Users/alex/Exports/Installer.app",
          journal,
          count: "4",
          paths: [journal, holder, `${holder}/previous`, `${holder}/replacement`].join("\n"),
        },
        detail: "The replacement needs manual recovery.",
      },
    });
    const recoveryRows = taskResultDetailRows(failed);
    assert.ok(recoveryRows.some((row) => row.value === journal));
    assert.ok(recoveryRows.some((row) => row.value.includes(`${holder}/previous`)));
    const recoveryStep = taskNextStepDetail(failed, false);
    assert.match(recoveryStep, /test .*Installer\.app/u);
    assert.match(recoveryStep, /delete only the listed previous backup/u);
    assert.match(recoveryStep, /target is missing or changed, do not delete the backup/u);

    const scanning = sfxTask({
      state: "running",
      done: 0,
      total: 0,
      current: "Contents/Resources/theme.dat",
      scanEntries: 37,
      result: null,
    });
    assert.equal(taskProgressPercent(scanning), 0);
    assert.equal(taskOverallProgressIndeterminate(scanning), true);
    assert.equal(taskOverallProgressBadge(scanning), "Scanning");
    assert.equal(taskProgressSummary(scanning), "Entries scanned: 37");
    assert.equal(taskCurrentSectionLabel(scanning), "Current input");
    assert.equal(taskCurrentProgressBadge(scanning), "Scanning");
    assert.equal(taskCurrentProgressSource(scanning), "scan-entry");
    const cancelledScan = { ...scanning, state: "cancelled" };
    assert.equal(taskOverallProgressIndeterminate(cancelledScan), false);
    assert.equal(taskOverallProgressBadge(cancelledScan), "Cancelled");
    assert.equal(taskCurrentSectionVisible(cancelledScan), true);
    const queuedScan = { ...scanning, state: "queued" };
    assert.equal(taskOverallProgressIndeterminate(queuedScan), true);
    assert.equal(taskOverallProgressBadge(queuedScan), "Waiting");
    assert.equal(taskCurrentProgressBadge(queuedScan), "Waiting");
    assert.equal(taskCurrentProgressSummary(queuedScan), "Waiting");
    const pausedScan = { ...scanning, state: "paused" };
    assert.equal(taskOverallProgressIndeterminate(pausedScan), true);
    assert.equal(taskOverallProgressBadge(pausedScan), "Paused");
    assert.equal(taskCurrentProgressBadge(pausedScan), "Paused");
    assert.equal(taskCurrentProgressSummary(pausedScan), "Paused");
    assert.equal(taskCurrentSectionVisible(scanning), true);
    assert.equal(
      taskCurrentSectionVisible({
        ...completed,
        current: "Complete",
        currentDone: completed.total,
        currentTotal: completed.total,
      }),
      false,
    );

    const verifyPhase = sfxTask({
      state: "running",
      done: 25,
      total: 100,
      phase: "update_verify",
      interruptible: true,
      result: null,
    });
    assert.equal(taskOverallProgressLabel(verifyPhase), "Current phase");
    assert.equal(taskOverallProgressBadge(verifyPhase), "25%");
    assert.match(taskProgressSummary(verifyPhase), /^Verifying packages · 25%/u);
    assert.equal(taskPhaseControlNoticeVisible(verifyPhase), false);

    const commitPhase = {
      ...verifyPhase,
      done: 0,
      total: 0,
      phase: "update_commit",
      interruptible: false,
    };
    assert.equal(taskOverallProgressIndeterminate(commitPhase), true);
    assert.equal(taskOverallProgressBadge(commitPhase), "Installing update");
    assert.equal(taskPhaseControlNoticeVisible(commitPhase), true);
    assert.match(taskPhaseControlNoticeDetail(commitPhase), /Pause and cancel are unavailable/u);

    const outputVerifyPhase = {
      ...verifyPhase,
      phase: "output_verify",
    };
    assert.match(taskProgressSummary(outputVerifyPhase), /^Verifying output · 25%/u);
    assert.equal(taskPhaseControlNoticeVisible(outputVerifyPhase), false);

    const outputSplitPhase = {
      ...verifyPhase,
      done: 50,
      total: 100,
      phase: "output_split",
    };
    assert.equal(taskOverallProgressBadge(outputSplitPhase), "50%");
    assert.match(taskProgressSummary(outputSplitPhase), /^Writing volume files · 50%/u);
    assert.equal(taskPhaseControlNoticeVisible(outputSplitPhase), false);

    const outputCommitPhase = {
      ...commitPhase,
      phase: "output_commit",
    };
    assert.equal(taskOverallProgressBadge(outputCommitPhase), "Publishing output");
    assert.equal(taskPhaseControlNoticeTitle(outputCommitPhase), "Publishing output");
    assert.match(taskPhaseControlNoticeDetail(outputCommitPhase), /verified output/u);
    assert.doesNotMatch(taskPhaseControlNoticeDetail(outputCommitPhase), /update/u);

    const outputRecoveryPhase = {
      ...outputCommitPhase,
      phase: "output_recovery",
    };
    assert.equal(taskOverallProgressBadge(outputRecoveryPhase), "Recovering output");
    assert.equal(taskPhaseControlNoticeTitle(outputRecoveryPhase), "Recovering output");
    assert.match(taskPhaseControlNoticeDetail(outputRecoveryPhase), /durable output recovery/u);
    assert.doesNotMatch(taskPhaseControlNoticeDetail(outputRecoveryPhase), /update/u);

    const recoveryProcessPhase = {
      ...verifyPhase,
      done: 375,
      total: 1000,
      phase: "recovery_process",
      pausable: false,
    };
    assert.equal(taskOverallProgressBadge(recoveryProcessPhase), "38%");
    assert.equal(
      taskProgressSummary(recoveryProcessPhase),
      "Processing recovery blocks · 38%",
    );
    assert.doesNotMatch(taskProgressSummary(recoveryProcessPhase), /B|\/s/u);
    assert.equal(taskPhaseControlNoticeVisible(recoveryProcessPhase), false);

    const recoveryFinalizePhase = {
      ...recoveryProcessPhase,
      done: 0,
      total: 0,
      phase: "recovery_finalize",
      interruptible: false,
    };
    assert.equal(taskPhaseControlNoticeVisible(recoveryFinalizePhase), true);
    assert.equal(taskPhaseControlNoticeTitle(recoveryFinalizePhase), "Completing recovery work");
    assert.match(taskPhaseControlNoticeDetail(recoveryFinalizePhase), /safe boundary/u);
    assert.doesNotMatch(taskPhaseControlNoticeDetail(recoveryFinalizePhase), /update/u);

    const failedAfterVerify = { ...verifyPhase, state: "failed" };
    assert.equal(taskOverallProgressBadge(failedAfterVerify), "Failed");
  } finally {
    await server.close();
  }
});

test("task questions keep empty passwords pending and normalize conflict scope", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const {
      applyCreateDestinationAuthorization,
      normalizeTaskConflictAnswer,
      taskPasswordReady,
    } = await server.ssrLoadModule("/src/lib/task-dialog.ts");

    assert.equal(taskPasswordReady(""), false);
    assert.equal(taskPasswordReady(" "), true);
    assert.equal(taskPasswordReady("archive-secret"), true);
    assert.deepEqual(normalizeTaskConflictAnswer("abort", true), {
      decision: "abort",
      applyAll: false,
    });
    for (const decision of ["skip", "overwrite", "rename"]) {
      assert.deepEqual(normalizeTaskConflictAnswer(decision, true), {
        decision,
        applyAll: true,
      });
    }

    const initiallyAbsentDestination = {
      kind: "compress",
      inputs: ["/Users/alex/Reports"],
      dest: "/Users/alex/Exports/reports.zip",
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: null,
      split_mode: "generic",
      excludes: [],
      replace_existing: false,
      replacement_guard: null,
    };
    const confirmedLateConflict = applyCreateDestinationAuthorization(
      initiallyAbsentDestination,
      "sqcg1_content-bound-guard",
    );
    assert.equal(confirmedLateConflict.replace_existing, true);
    assert.equal(
      confirmedLateConflict.replacement_guard,
      "sqcg1_content-bound-guard",
    );
    assert.deepEqual(
      applyCreateDestinationAuthorization(confirmedLateConflict, null),
      initiallyAbsentDestination,
    );
  } finally {
    await server.close();
  }
});
