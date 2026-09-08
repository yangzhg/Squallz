#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

export function cargoTargetDirectory(
  root,
  { env = process.env, spawn = spawnSync } = {},
) {
  const result = spawn("cargo", [
    "metadata",
    "--format-version",
    "1",
    "--no-deps",
    "--manifest-path",
    resolve(root, "Cargo.toml"),
  ], {
    cwd: root,
    env,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const detail = result.stderr?.trim();
    throw new Error(detail ? `cargo metadata failed: ${detail}` : "cargo metadata failed");
  }

  let metadata;
  try {
    metadata = JSON.parse(result.stdout);
  } catch (error) {
    throw new Error("cargo metadata returned invalid JSON", { cause: error });
  }
  if (typeof metadata.target_directory !== "string" || metadata.target_directory.length === 0) {
    throw new Error("cargo metadata did not report target_directory");
  }
  return resolve(root, metadata.target_directory);
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : null;
if (invokedPath === fileURLToPath(import.meta.url)) {
  const root = resolve(process.argv[2] ?? process.cwd());
  try {
    process.stdout.write(`${cargoTargetDirectory(root)}\n`);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
