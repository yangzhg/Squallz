#!/usr/bin/env node
import { copyFileSync, existsSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(frontendDir, "..");

for (const profile of ["debug", "release"]) {
  const legacyDocs = resolve(root, "target", profile, "bundle", "macos", "Squallz.app", "Contents", "Resources", "docs");
  if (existsSync(legacyDocs)) {
    rmSync(legacyDocs, { recursive: true, force: true });
  }
}

function run(command, args, cwd) {
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

const npmCli = process.platform === "win32"
  ? resolve(dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js")
  : null;
run(process.platform === "win32" ? process.execPath : "npm", npmCli ? [npmCli, "run", "build"] : ["run", "build"], frontendDir);
run("cargo", ["build", "--manifest-path", resolve(root, "Cargo.toml"), "-p", "squallz-cli", "--release"], root);

if (process.platform === "win32") {
  const exePath = resolve(root, "target", "release", "sqz.exe");
  const resourcePath = resolve(root, "target", "release", "sqz");
  if (existsSync(exePath)) {
    copyFileSync(exePath, resourcePath);
  }
}
