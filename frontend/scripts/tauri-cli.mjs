#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(frontendDir, "..");
const guiDir = resolve(root, "crates", "squallz-gui");
const tauriScript = resolve(frontendDir, "node_modules", "@tauri-apps", "cli", "tauri.js");

const result = spawnSync(process.execPath, [tauriScript, ...process.argv.slice(2)], {
  cwd: guiDir,
  stdio: "inherit",
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
