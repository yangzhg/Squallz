import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("recovery grouping stays conservative until RAR headers are verified", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const names = await server.ssrLoadModule("/src/lib/archive-names.ts");
    const externalTasks = await server.ssrLoadModule("/src/lib/external-tasks.ts");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.part2.rar"), "movie.part2.rar");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.part002.RAR"), "movie.part002.RAR");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.rar.002"), "movie.rar");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.z02"), "movie");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.tar.Z100"), "movie.tar");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.part0.rar"), "movie.part0.rar");
    assert.equal(names.archiveNameWithoutVolumeSuffix("movie.cbr"), "movie.cbr");
    assert.equal(names.isLegacyRarVolumeName("movie.r00"), true);
    assert.equal(names.isLegacyRarVolumeName("movie.R99"), true);
    assert.equal(names.isLegacyRarVolumeName("movie.r100"), false);
    assert.equal(names.stripLegacyRarVolumeSuffix("movie.r07"), "movie");
    assert.equal(names.isNativeSplitZipVolumeName("movie.z01"), true);
    assert.equal(names.isNativeSplitZipVolumeName("movie.Z100"), true);
    assert.equal(names.isNativeSplitZipVolumeName("movie.z00"), false);
    assert.equal(names.isNativeSplitZipVolumeName("movie.z1"), false);
    assert.equal(names.stripNativeSplitZipVolumeSuffix("movie.z07"), "movie");
    assert.equal(names.archiveVolumeFamilyKey("movie.part002.RAR"), "single:movie.part002.RAR");
    assert.equal(names.archiveVolumeFamilyKey("movie.r07"), "single:movie.r07");
    assert.equal(names.archiveVolumeFamilyKey("movie.R00"), "single:movie.R00");
    assert.equal(names.archiveVolumeFamilyKey("movie.RAR"), "single:movie.RAR");
    assert.equal(names.archiveVolumeFamilyKey("movie.rar.007"), "generic:movie.rar");
    assert.equal(names.archiveVolumeFamilyKey("movie.z01"), "single:movie.z01");
    assert.equal(names.archiveVolumeFamilyKey("movie.zip"), "single:movie.zip");
    assert.equal(
      new Set(
        names.archiveVolumeFamilyKeys([
          "/archives/movie.RAR",
          "/archives/movie.R00",
          "/archives/movie.R01",
        ]),
      ).size,
      3,
    );
    assert.equal(
      new Set(
        names.archiveVolumeFamilyKeys([
          "/archives/movie.part1.rar",
          "/archives/movie.part2.rar",
        ]),
      ).size,
      2,
    );
    assert.notEqual(
      names.archiveVolumeFamilyKey("movie.part002.rar"),
      names.archiveVolumeFamilyKey("movie.rar.002"),
    );
    assert.equal(
      new Set(
        names.archiveVolumeFamilyKeys([
          "movie.part002.rar",
          "movie.r07",
          "movie.rar.007",
        ]),
      ).size,
      3,
    );
    assert.equal(externalTasks.defaultExternalArchiveStemName("movie.part7.rar"), "movie.part7");
    assert.equal(externalTasks.defaultExternalArchiveStemName("movie.r07"), "movie");
    assert.equal(externalTasks.defaultExternalArchiveStemName("movie.rar.007"), "movie");
    assert.equal(externalTasks.defaultExternalArchiveStemName("movie.z07"), "movie");
    assert.equal(names.legacyRarVolumeExtensions.length, 100);
    assert.deepEqual(
      [
        names.legacyRarVolumeExtensions[0],
        names.legacyRarVolumeExtensions.at(-1),
      ],
      ["r00", "r99"],
    );
    assert.equal(names.nativeSplitZipVolumeExtensions.length, 99);
    assert.deepEqual(
      [
        names.nativeSplitZipVolumeExtensions[0],
        names.nativeSplitZipVolumeExtensions.at(-1),
      ],
      ["z01", "z99"],
    );
  } finally {
    await server.close();
  }
});
