import { readFile, readdir, rmdir, unlink } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

export const ENTRY_RAW_BUDGET = 453_000;
export const ENTRY_GZIP_BUDGET = 132_000;
export const STARTUP_DYNAMIC_CHUNK_NAMES = ["event", "window"];

const JAVASCRIPT_EXTENSIONS = new Set([".js", ".mjs"]);

function formatKilobytes(bytes) {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

export function entryScriptPath(indexHtml) {
  const script = /<script\b(?=[^>]*\btype="module")(?=[^>]*\bsrc="([^"]+)")[^>]*>/i.exec(indexHtml);
  if (!script) throw new Error("Production index.html has no module entry script");
  return script[1].replace(/^\/+/, "");
}

export function initialJavaScriptFiles(
  entryScript,
  manifest,
  { startupDynamicChunkNames = STARTUP_DYNAMIC_CHUNK_NAMES } = {},
) {
  const normalizedEntry = entryScript.replace(/^\/+/, "");
  const entry = Object.entries(manifest).find(
    ([, chunk]) => chunk?.isEntry === true && chunk.file?.replace(/^\/+/, "") === normalizedEntry,
  );
  if (!entry) {
    throw new Error(`Vite manifest has no entry for ${normalizedEntry}`);
  }

  const files = [];
  const visitedChunks = new Set();
  const visitedFiles = new Set();
  const visit = (key) => {
    if (visitedChunks.has(key)) return;
    const chunk = manifest[key];
    if (!chunk || typeof chunk.file !== "string") {
      throw new Error(`Vite manifest references missing chunk ${key}`);
    }
    visitedChunks.add(key);
    const file = chunk.file.replace(/^\/+/, "");
    if (!JAVASCRIPT_EXTENSIONS.has(path.posix.extname(file))) {
      throw new Error(`Vite manifest JavaScript chunk has unsupported file type: ${file}`);
    }
    if (!visitedFiles.has(file)) {
      visitedFiles.add(file);
      files.push(file);
    }
    for (const importedKey of chunk.imports ?? []) visit(importedKey);
  };

  visit(entry[0]);
  for (const name of startupDynamicChunkNames) {
    const matches = Object.entries(manifest).filter(
      ([, chunk]) => chunk?.isDynamicEntry === true && chunk.name === name,
    );
    if (matches.length !== 1) {
      throw new Error(
        `Vite manifest must contain exactly one startup dynamic chunk named ${name}; found ${matches.length}`,
      );
    }
    visit(matches[0][0]);
  }
  return files;
}

export function assertInitialJavaScriptBudget(
  files,
  {
    rawBudget = ENTRY_RAW_BUDGET,
    gzipBudget = ENTRY_GZIP_BUDGET,
  } = {},
) {
  const raw = files.reduce((total, file) => total + file.byteLength, 0);
  const gzip = files.reduce((total, file) => total + gzipSync(file).byteLength, 0);
  const failures = [];
  if (raw > rawBudget) {
    failures.push(`raw ${formatKilobytes(raw)} exceeds ${formatKilobytes(rawBudget)}`);
  }
  if (gzip > gzipBudget) {
    failures.push(`gzip ${formatKilobytes(gzip)} exceeds ${formatKilobytes(gzipBudget)}`);
  }
  if (failures.length > 0) {
    throw new Error(`Initial JavaScript bundle budget failed: ${failures.join("; ")}`);
  }
  return { raw, gzip, rawBudget, gzipBudget };
}

function resolveDistFile(resolvedDist, relativeFile) {
  const filePath = path.resolve(resolvedDist, relativeFile);
  if (
    filePath !== resolvedDist
    && !filePath.startsWith(`${resolvedDist}${path.sep}`)
  ) {
    throw new Error("Production bundle file resolves outside the distribution directory");
  }
  return filePath;
}

export async function removeBuildManifest(resolvedDist) {
  const manifestDir = path.join(resolvedDist, ".vite");
  const entries = await readdir(manifestDir);
  if (entries.length !== 1 || entries[0] !== "manifest.json") {
    throw new Error("Production manifest directory contains unexpected build metadata");
  }
  await unlink(path.join(manifestDir, "manifest.json"));
  await rmdir(manifestDir);
}

export async function checkBundleBudget(distDir) {
  const resolvedDist = path.resolve(distDir);
  const indexHtml = await readFile(path.join(resolvedDist, "index.html"), "utf8");
  const relativeEntry = entryScriptPath(indexHtml);
  const manifest = JSON.parse(
    await readFile(path.join(resolvedDist, ".vite/manifest.json"), "utf8"),
  );
  const relativeFiles = initialJavaScriptFiles(relativeEntry, manifest);
  const files = await Promise.all(
    relativeFiles.map((relativeFile) => readFile(resolveDistFile(resolvedDist, relativeFile))),
  );
  return {
    relativeEntry,
    relativeFiles,
    ...assertInitialJavaScriptBudget(files),
  };
}

async function main() {
  const scriptDir = path.dirname(fileURLToPath(import.meta.url));
  const distDir = process.env.SQUALLZ_DIST_DIR
    ? path.resolve(process.env.SQUALLZ_DIST_DIR)
    : path.resolve(scriptDir, "../dist");
  const result = await checkBundleBudget(distDir);
  await removeBuildManifest(path.resolve(distDir));
  console.log(
    `Bundle budget passed: ${result.relativeFiles.length} initial JS file(s), entry ${result.relativeEntry} `
      + `${formatKilobytes(result.raw)} / ${formatKilobytes(result.rawBudget)}, `
      + `gzip ${formatKilobytes(result.gzip)} / ${formatKilobytes(result.gzipBudget)}`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
