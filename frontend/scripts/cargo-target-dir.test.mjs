import assert from "node:assert/strict";
import test from "node:test";
import { resolve } from "node:path";

import { cargoTargetDirectory } from "./cargo-target-dir.mjs";

test("cargoTargetDirectory uses Cargo metadata with the caller environment", () => {
  const root = resolve("workspace", "Squallz");
  const env = { CARGO_TARGET_DIR: resolve("workspace", "squallz-target") };
  let invocation = null;
  const target = cargoTargetDirectory(root, {
    env,
    spawn(command, args, options) {
      invocation = { command, args, options };
      return {
        status: 0,
        stdout: JSON.stringify({ target_directory: env.CARGO_TARGET_DIR }),
        stderr: "",
      };
    },
  });

  assert.equal(target, env.CARGO_TARGET_DIR);
  assert.equal(invocation.command, "cargo");
  assert.deepEqual(invocation.args, [
    "metadata",
    "--format-version",
    "1",
    "--no-deps",
    "--manifest-path",
    resolve(root, "Cargo.toml"),
  ]);
  assert.equal(invocation.options.cwd, root);
  assert.equal(invocation.options.env, env);
});

test("cargoTargetDirectory rejects unusable Cargo metadata", () => {
  const root = resolve("workspace", "Squallz");
  assert.throws(
    () => cargoTargetDirectory(root, {
      spawn: () => ({ status: 0, stdout: "{}", stderr: "" }),
    }),
    /did not report target_directory/,
  );
  assert.throws(
    () => cargoTargetDirectory(root, {
      spawn: () => ({ status: 1, stdout: "", stderr: "broken manifest" }),
    }),
    /cargo metadata failed: broken manifest/,
  );
});
