import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("system opening requires confirmation for potentially executable file types", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { previewSystemOpenRequiresConfirmation } = await server.ssrLoadModule(
      "/src/lib/preview-presentation.ts",
    );

    for (const entry of [
      "tools/setup.EXE",
      "tools/setup.exe.",
      "tools/REPORT.EXE ",
      "scripts/install.command",
      "scripts/cleanup.sh",
      "scripts/build.py",
      "scripts/check.zsh",
      "windows/start.ps1",
      "windows/launch.PIF",
      "windows/clickonce.appref-ms",
      "windows/shortcut.application",
      "windows/shell.scf",
      "windows/console.msc",
      "windows/sandbox.wsb",
      "linux/launcher.desktop",
      "release/tool.AppImage",
      "packages/installer.pkg",
      "packages/installer.rpm",
      "utilities/runner.jar",
    ]) {
      assert.equal(previewSystemOpenRequiresConfirmation(entry), true, entry);
    }
    for (const entry of [
      "documents/manual.pdf",
      "documents/report.docx",
      "documents/budget.xlsx",
      "documents/slides.pptx",
      "images/photo.jpeg",
      "media/launch.mov",
      "notes/readme.txt",
      "firmware/device.bin",
      "scripts/example.scpt",
      "archives/source.tar.gz",
      "folder.with.dots/no-extension",
    ]) {
      assert.equal(previewSystemOpenRequiresConfirmation(entry), false, entry);
    }
  } finally {
    await server.close();
  }
});
