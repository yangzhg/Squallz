import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const repositoryRoot = path.resolve(frontendRoot, "..");
const selectedSourceSha256 = "c8386f791bf3a6fc54d98638f44ec7ed72324c7d40417fec5b327de3594eec24";

function pngInfo(bytes) {
  assert.deepEqual(
    bytes.subarray(0, 8),
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  );
  assert.equal(bytes.subarray(12, 16).toString("ascii"), "IHDR");
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
    bitDepth: bytes[24],
    colorType: bytes[25],
  };
}

test("the selected layered zipper icon is the canonical cross-platform source", async () => {
  const [
    source,
    bundleIcon,
    appIcon,
    component,
    composerSource,
    composerManifest,
    compiledMacosIcon,
    compiledMacosManifest,
    tauriConfig,
    infoPlist,
  ] = await Promise.all([
    readFile(path.join(repositoryRoot, "crates/squallz-gui/icons/squallz-icon-source.png")),
    readFile(path.join(repositoryRoot, "crates/squallz-gui/icons/icon.png")),
    readFile(path.join(frontendRoot, "src/assets/squallz-app-icon.png")),
    readFile(path.join(frontendRoot, "src/components/AppIcon.svelte"), "utf8"),
    readFile(
      path.join(repositoryRoot, "crates/squallz-gui/icons/AppIcon.icon/Assets/Squallz.png"),
    ),
    readFile(
      path.join(repositoryRoot, "crates/squallz-gui/icons/AppIcon.icon/icon.json"),
      "utf8",
    ),
    readFile(path.join(repositoryRoot, "crates/squallz-gui/icons/AppIcon-compiled.car")),
    readFile(
      path.join(repositoryRoot, "crates/squallz-gui/icons/macos-icon-build.json"),
      "utf8",
    ),
    readFile(path.join(repositoryRoot, "crates/squallz-gui/tauri.conf.json"), "utf8"),
    readFile(path.join(repositoryRoot, "crates/squallz-gui/Info.plist"), "utf8"),
  ]);

  assert.equal(createHash("sha256").update(source).digest("hex"), selectedSourceSha256);
  assert.deepEqual(pngInfo(source), {
    width: 1254,
    height: 1254,
    bitDepth: 8,
    colorType: 6,
  });
  assert.deepEqual(pngInfo(bundleIcon), {
    width: 512,
    height: 512,
    bitDepth: 8,
    colorType: 6,
  });
  assert.deepEqual(appIcon, bundleIcon);
  assert.deepEqual(composerSource, source);
  assert.equal(compiledMacosIcon.subarray(0, 8).toString("ascii"), "BOMStore");
  assert.ok(compiledMacosIcon.length > 1_000_000);
  const compiledManifest = JSON.parse(compiledMacosManifest);
  assert.equal(compiledManifest.schema, 1);
  assert.equal(compiledManifest.source_sha256, selectedSourceSha256);
  assert.equal(
    compiledManifest.assets_car_sha256,
    createHash("sha256").update(compiledMacosIcon).digest("hex"),
  );
  assert.ok(Number.parseInt(compiledManifest.xcode_version, 10) >= 26);
  assert.match(compiledManifest.xcode_build, /^[A-Za-z0-9]+$/);
  assert.equal(compiledManifest.minimum_macos, "11.0");
  assert.match(component, /assets\/squallz-app-icon\.png/);

  const composer = JSON.parse(composerManifest);
  assert.deepEqual(composer["supported-platforms"], { squares: ["macOS"] });
  assert.deepEqual(composer.groups.flatMap((group) => group.layers), [
    {
      glass: false,
      hidden: false,
      "image-name": "Squallz.png",
      name: "Squallz",
    },
  ]);

  const config = JSON.parse(tauriConfig);
  assert.equal(
    config.bundle.macOS.files["Resources/Assets.car"],
    "icons/AppIcon-compiled.car",
  );
  assert.equal(
    config.bundle.macOS.files["Resources/AppIcon.icns"],
    "icons/icon.icns",
  );
  assert.match(infoPlist, /<key>CFBundleIconFile<\/key>\s*<string>AppIcon<\/string>/);
  assert.match(infoPlist, /<key>CFBundleIconName<\/key>\s*<string>AppIcon<\/string>/);
});
