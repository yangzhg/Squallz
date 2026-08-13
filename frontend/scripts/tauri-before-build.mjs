#!/usr/bin/env node
import { chmodSync, copyFileSync, existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(frontendDir, "..");
const universalMacTarget = "universal-apple-darwin";
const universalMacComponents = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
const linuxSfxDataMagic = Buffer.from("SQZSFXD1", "ascii");

for (const profile of ["debug", "release"]) {
  const legacyDocs = resolve(root, "target", profile, "bundle", "macos", "Squallz.app", "Contents", "Resources", "docs");
  if (existsSync(legacyDocs)) {
    rmSync(legacyDocs, { recursive: true, force: true });
  }
}

function run(command, args, cwd, env = process.env) {
  const result = spawnSync(command, args, {
    cwd,
    env,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function writeLinuxSfxData(runtimePath, templatePath) {
  const runtime = readFileSync(runtimePath);
  const length = Buffer.alloc(8);
  length.writeBigUInt64LE(BigInt(runtime.length));
  const digest = createHash("sha256").update(runtime).digest();
  writeFileSync(templatePath, Buffer.concat([linuxSfxDataMagic, length, digest, runtime]), {
    mode: 0o644,
  });
  chmodSync(templatePath, 0o644);
}

function rustHostTriple() {
  const result = spawnSync("rustc", ["-vV"], {
    cwd: root,
    encoding: "utf8",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
  const host = result.stdout
    .split("\n")
    .find((line) => line.startsWith("host: "))
    ?.slice("host: ".length)
    .trim();
  if (!host) throw new Error("rustc did not report a host target triple");
  return host;
}

function macosDeploymentTarget() {
  const configPath = resolve(root, "crates", "squallz-gui", "tauri.conf.json");
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  const version = config?.bundle?.macOS?.minimumSystemVersion;
  if (typeof version !== "string" || !/^\d+(?:\.\d+){1,2}$/.test(version)) {
    throw new Error("Tauri macOS minimumSystemVersion must be an explicit dotted version");
  }
  return version;
}

const npmCli = process.platform === "win32"
  ? resolve(dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js")
  : null;
run(process.platform === "win32" ? process.execPath : "npm", npmCli ? [npmCli, "run", "build"] : ["run", "build"], frontendDir);
const hostTriple = rustHostTriple();
const targetTriple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple;
const cargoEnv = targetTriple.endsWith("apple-darwin")
  ? { ...process.env, MACOSX_DEPLOYMENT_TARGET: macosDeploymentTarget() }
  : process.env;
const executableSuffix = targetTriple.includes("windows") ? ".exe" : "";
const sidecarPath = resolve(root, "target", "release", `sqz-${targetTriple}${executableSuffix}`);

if (targetTriple === universalMacTarget) {
  const componentPaths = universalMacComponents.map((componentTarget) => {
    run("cargo", [
      "build",
      "--manifest-path",
      resolve(root, "Cargo.toml"),
      "-p",
      "squallz-cli",
      "--release",
      "--target",
      componentTarget,
    ], root, cargoEnv);
    return resolve(root, "target", componentTarget, "release", "sqz");
  });
  for (const componentPath of componentPaths) {
    if (!existsSync(componentPath)) {
      throw new Error(`built sqz sidecar component is missing: ${componentPath}`);
    }
  }
  run("lipo", ["-create", ...componentPaths, "-output", sidecarPath], root);
} else {
  const cargoArgs = ["build", "--manifest-path", resolve(root, "Cargo.toml"), "-p", "squallz-cli", "--release"];
  if (targetTriple !== hostTriple) cargoArgs.push("--target", targetTriple);
  run("cargo", cargoArgs, root, cargoEnv);

  const profileDir = targetTriple === hostTriple
    ? resolve(root, "target", "release")
    : resolve(root, "target", targetTriple, "release");
  const cliPath = resolve(profileDir, `sqz${executableSuffix}`);
  if (!existsSync(cliPath)) {
    throw new Error(`built sqz sidecar is missing: ${cliPath}`);
  }
  copyFileSync(cliPath, sidecarPath);

  if (targetTriple.includes("windows") || targetTriple.includes("linux")) {
    const runtimeArgs = [
      "build",
      "--manifest-path",
      resolve(root, "Cargo.toml"),
      "-p",
      "squallz-sfx-runtime",
      "--release",
    ];
    if (targetTriple !== hostTriple) runtimeArgs.push("--target", targetTriple);
    run("cargo", runtimeArgs, root, cargoEnv);

    const runtimePath = resolve(profileDir, `sqz-sfx${executableSuffix}`);
    if (!existsSync(runtimePath)) {
      throw new Error(`built sqz-sfx runtime is missing: ${runtimePath}`);
    }
    const templatePath = resolve(root, "target", "release", "sqz-sfx-template.stub");
    if (targetTriple.includes("linux")) {
      writeLinuxSfxData(runtimePath, templatePath);
    } else {
      copyFileSync(runtimePath, templatePath);
    }
  }
}
if (!executableSuffix) chmodSync(sidecarPath, 0o755);

if (targetTriple.endsWith("apple-darwin")) {
  run("bash", [
    resolve(root, "scripts", "build_macos_quicklook_extension.sh"),
  ], root, cargoEnv);
}
