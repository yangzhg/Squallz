#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const cssPath = path.join(repoRoot, "frontend", "src", "design.css");
const css = fs.readFileSync(cssPath, "utf8").split("\n");

const rawColorPattern =
  /#[0-9a-fA-F]{3,8}|rgba?\([^)]*\)|linear-gradient\([^;]*\)|(?<!-)\b(?:white|black)\b(?!-)/g;
const violations = [];

for (let index = 0; index < css.length; index += 1) {
  const line = css[index];
  const trimmed = line.trim();

  if (
    trimmed.startsWith("--") ||
    line.includes("conic-gradient")
  ) {
    continue;
  }

  const matches = line.match(rawColorPattern);
  if (matches) {
    violations.push({ line: index + 1, text: trimmed, matches });
  }
}

if (violations.length > 0) {
  console.error("UI token audit failed. Raw colors outside token declarations:");
  for (const violation of violations.slice(0, 80)) {
    console.error(`${violation.line}: ${violation.text}`);
  }
  if (violations.length > 80) {
    console.error(`... ${violations.length - 80} more`);
  }
  process.exit(1);
}

console.log("UI token audit passed: raw colors are isolated to token declarations and the functional color wheel.");
