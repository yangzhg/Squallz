import assert from "node:assert/strict";
import { access, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { gzipSync } from "node:zlib";
import {
  assertInitialJavaScriptBudget,
  entryScriptPath,
  initialJavaScriptFiles,
  removeBuildManifest,
} from "../../scripts/check-bundle-budget.mjs";

test("bundle budget resolves the production module entry", () => {
  assert.equal(
    entryScriptPath('<script type="module" crossorigin src="/assets/index-abc.js"></script>'),
    "assets/index-abc.js",
  );
  assert.equal(
    entryScriptPath('<script src="/assets/index-def.js" type="module"></script>'),
    "assets/index-def.js",
  );
  assert.throws(
    () => entryScriptPath('<script src="/assets/legacy.js"></script>'),
    /no module entry script/,
  );
});

test("bundle budget follows the immediate startup closure without charging deferred work", () => {
  const manifest = {
    "index.html": {
      file: "assets/index.mjs",
      isEntry: true,
      imports: ["_shared.js", "_runtime.js"],
      dynamicImports: ["src/routes/create.ts", "src/lib/app-update.ts", "_window.js"],
    },
    "_shared.js": {
      file: "assets/shared.js",
      imports: ["_runtime.js"],
    },
    "_runtime.js": {
      file: "assets/runtime.mjs",
    },
    "_window.js": {
      file: "assets/window.js",
      name: "window",
      isDynamicEntry: true,
      imports: ["node_modules/event.js", "_runtime.js"],
    },
    "node_modules/event.js": {
      file: "assets/event.js",
      name: "event",
      isDynamicEntry: true,
    },
    "src/lib/app-update.ts": {
      file: "assets/app-update.js",
      name: "app-update.svelte",
      isDynamicEntry: true,
    },
    "src/routes/create.ts": {
      file: "assets/create.js",
      isDynamicEntry: true,
    },
  };

  assert.deepEqual(initialJavaScriptFiles("assets/index.mjs", manifest), [
    "assets/index.mjs",
    "assets/shared.js",
    "assets/runtime.mjs",
    "assets/event.js",
    "assets/window.js",
  ]);
  assert.throws(
    () => initialJavaScriptFiles("assets/missing.js", manifest),
    /has no entry/,
  );
  assert.throws(
    () => initialJavaScriptFiles("assets/index.mjs", {
      ...manifest,
      "_window.js": { ...manifest["_window.js"], name: "renamed-window" },
    }),
    /startup dynamic chunk named window; found 0/,
  );
  assert.throws(
    () => initialJavaScriptFiles("assets/index.mjs", {
      ...manifest,
      "duplicate-window.js": {
        file: "assets/window-copy.js",
        name: "window",
        isDynamicEntry: true,
      },
    }),
    /startup dynamic chunk named window; found 2/,
  );
  assert.throws(
    () => initialJavaScriptFiles("assets/index.mjs", {
      ...manifest,
      "_runtime.js": { file: "assets/runtime.css" },
    }),
    /unsupported file type/,
  );
});

test("bundle budget aggregates independently transferred JavaScript files", () => {
  const entry = Buffer.from("const ready=true;");
  const startup = Buffer.from("export const startup='ready';");
  const raw = entry.byteLength + startup.byteLength;
  const gzip = gzipSync(entry).byteLength + gzipSync(startup).byteLength;
  const accepted = assertInitialJavaScriptBudget([entry, startup], {
    rawBudget: raw,
    gzipBudget: gzip,
  });
  assert.deepEqual({ raw: accepted.raw, gzip: accepted.gzip }, { raw, gzip });
  assert.throws(
    () => assertInitialJavaScriptBudget([entry, startup], {
      rawBudget: raw - 1,
      gzipBudget: gzip,
    }),
    /raw .* exceeds/,
  );
  assert.throws(
    () => assertInitialJavaScriptBudget([entry, startup], {
      rawBudget: raw,
      gzipBudget: gzip - 1,
    }),
    /gzip .* exceeds/,
  );
});

test("bundle metadata cleanup refuses unexpected release assets", async (context) => {
  const dist = await mkdtemp(path.join(tmpdir(), "squallz-bundle-"));
  context.after(() => rm(dist, { recursive: true, force: true }));
  const manifestDir = path.join(dist, ".vite");
  const manifestPath = path.join(manifestDir, "manifest.json");
  await mkdir(manifestDir);
  await writeFile(manifestPath, "{}");
  await writeFile(path.join(manifestDir, "unexpected.json"), "{}");

  await assert.rejects(
    removeBuildManifest(dist),
    /unexpected build metadata/,
  );
  await access(manifestPath);
});
