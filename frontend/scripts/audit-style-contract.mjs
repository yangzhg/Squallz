#!/usr/bin/env node
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const frontendRoot = path.resolve(scriptDir, "..");
const srcRoot = path.join(frontendRoot, "src");

const allowedTypeTokens = new Set([
  "--type-control",
  "--type-control-line",
  "--type-body",
  "--type-body-line",
  "--type-metric",
  "--type-metric-line",
  "--type-title",
  "--type-title-line",
  "--type-page-title",
  "--type-page-title-line",
]);
const allowedWeightTokens = new Set(["--font-weight-normal", "--font-weight-medium"]);
const colorLiteralFiles = new Set([
  "src/components/AppIcon.svelte",
  "src/design.css",
  "src/lib/theme.ts",
  "src/lib/ui-model.ts",
]);
const legacyColorTokens = [
  "--ink",
  "--muted",
  "--faint",
  "--line",
  "--soft-line",
  "--surface",
  "--surface-2",
  "--surface-3",
  "--shadow-soft",
  "--control-bg",
  "--control-bg-subtle",
  "--control-border",
  "--control-ink",
  "--disabled-bg",
  "--disabled-border",
  "--disabled-ink",
];
const paletteSeedTokens = ["--accent", "--accent-2", "--accent-soft", "--accent-ink", "--accent-shadow"];
const retiredSelectors = [
  ".mode-choice-grid",
  ".mode-card",
  ".mode-setting",
  ".color-wheel-picker.small",
  ".meter.slim",
];
const requiredThemeTokens = [
  "--color-bg-app",
  "--color-bg-window",
  "--color-bg-content",
  "--color-bg-chrome",
  "--color-bg-sidebar",
  "--color-surface",
  "--color-surface-raised",
  "--color-surface-muted",
  "--color-surface-inset",
  "--color-surface-overlay",
  "--color-border-default",
  "--color-border-muted",
  "--color-border-strong",
  "--color-divider",
  "--color-text-primary",
  "--color-text-strong",
  "--color-text-secondary",
  "--color-text-tertiary",
  "--color-text-inverse",
  "--color-focus-ring",
  "--color-accent",
  "--color-accent-strong",
  "--color-accent-bg",
  "--color-accent-fg",
  "--color-accent-contrast",
  "--color-accent-border",
  "--color-accent-shadow",
  "--color-callout-bg",
  "--color-callout-border",
  "--color-callout-fg",
  "--color-success-bg",
  "--color-success-border",
  "--color-success-fg",
  "--color-warning-bg",
  "--color-warning-border",
  "--color-warning-fg",
  "--color-danger-bg",
  "--color-danger-border",
  "--color-danger-fg",
  "--color-info-bg",
  "--color-info-border",
  "--color-info-fg",
  "--color-recovery-bg",
  "--color-recovery-border",
  "--color-recovery-fg",
  "--color-control-bg",
  "--color-control-bg-hover",
  "--color-control-bg-active",
  "--color-control-border",
  "--color-control-fg",
  "--color-control-disabled-bg",
  "--color-control-disabled-border",
  "--color-control-disabled-fg",
  "--button-primary-bg",
  "--button-primary-fg",
  "--button-primary-border",
  "--button-primary-bg-hover",
  "--button-primary-fg-hover",
  "--button-secondary-bg",
  "--button-secondary-bg-hover",
  "--button-secondary-fg",
  "--button-secondary-fg-hover",
  "--button-secondary-border",
  "--button-selected-bg",
  "--button-selected-fg",
  "--button-selected-border",
  "--input-bg",
  "--input-border",
  "--input-fg",
  "--input-placeholder",
  "--card-bg",
  "--card-border",
  "--table-head-bg",
  "--table-row-bg",
  "--table-row-selected-bg",
  "--badge-bg",
  "--badge-fg",
  "--popover-bg",
  "--popover-fg",
  "--popover-border",
  "--toast-bg",
  "--toast-fg",
  "--toast-border",
  "--chip-bg",
  "--chip-fg",
  "--chip-border",
  "--shadow-surface",
  "--shadow-popover",
  "--shadow-floating",
];

const findings = [];

function walk(dir) {
  for (const name of readdirSync(dir)) {
    const file = path.join(dir, name);
    const stat = statSync(file);
    if (stat.isDirectory()) {
      walk(file);
    } else if (/\.(css|svelte|ts)$/.test(name)) {
      auditFile(file);
    }
  }
}

function auditFile(file) {
  const relativePath = path.relative(frontendRoot, file).replaceAll(path.sep, "/");
  const source = readFileSync(file, "utf8");
  const lines = source.split(/\r?\n/);

  for (const [index, line] of lines.entries()) {
    const number = index + 1;
    if (/font-size:\s*(?!var\()[^;]*\b\d+(?:\.\d+)?px\b/.test(line)) {
      add(relativePath, number, "Raw typography size; use the --type-* scale.");
    }
    if (/line-height:\s*(?!var\()[^;]*\b\d+(?:\.\d+)?(?:px|rem|em)?\b/.test(line)) {
      add(relativePath, number, "Raw line-height; use the --type-* scale.");
    }
    if (/font-weight:\s*(?:bold|bolder|[1-9][0-9]{2})\b/.test(line)) {
      add(relativePath, number, "Raw font weight; use --font-weight-normal or --font-weight-medium.");
    }

    for (const token of legacyColorTokens) {
      if (line.includes(token)) {
        add(relativePath, number, `Legacy color alias ${token}; use the semantic --color-* token.`);
      }
    }

    if (!colorLiteralFiles.has(relativePath) && hasColorLiteral(line)) {
      add(relativePath, number, "Hard-coded color outside the theme contract.");
    }
  }

  if (relativePath === "src/design.css") {
    auditDesignCss(source, lines);
  }
}

function auditDesignCss(source, lines) {
  const typeTokens = new Set([...source.matchAll(/--type-[a-z0-9-]+(?=\s*:)/g)].map((match) => match[0]));
  const weightTokens = new Set([...source.matchAll(/--font-weight-[a-z0-9-]+(?=\s*:)/g)].map((match) => match[0]));

  for (const token of typeTokens) {
    if (!allowedTypeTokens.has(token)) {
      add("src/design.css", 1, `Unexpected typography token ${token}; fold it into the canonical scale.`);
    }
  }
  for (const token of allowedTypeTokens) {
    if (!typeTokens.has(token)) {
      add("src/design.css", 1, `Missing typography token ${token}.`);
    }
  }
  for (const token of weightTokens) {
    if (!allowedWeightTokens.has(token)) {
      add("src/design.css", 1, `Unexpected font-weight token ${token}; fold it into the canonical scale.`);
    }
  }
  for (const token of allowedWeightTokens) {
    if (!weightTokens.has(token)) {
      add("src/design.css", 1, `Missing font-weight token ${token}.`);
    }
  }
  for (const selector of retiredSelectors) {
    if (selectorIsDefined(source, selector)) {
      add("src/design.css", 1, `Retired selector ${selector} is still defined.`);
    }
  }

  const rootBlock = cssBlock(source, ":root");
  const darkBlock = cssBlock(source, ".theme-dark");
  for (const token of requiredThemeTokens) {
    if (!rootBlock.includes(`${token}:`)) {
      add("src/design.css", 1, `Light theme missing ${token}.`);
    }
    if (!darkBlock.includes(`${token}:`)) {
      add("src/design.css", 1, `Dark theme missing ${token}.`);
    }
  }

  const modelSource = readFileSync(path.join(srcRoot, "lib/ui-model.ts"), "utf8");
  const paletteIds = parsePaletteIds(modelSource);
  for (const paletteId of paletteIds) {
    if (!source.includes(`.palette-${paletteId} {`)) {
      add("src/design.css", 1, `Missing light palette selector .palette-${paletteId}.`);
    }
    if (!source.includes(`.theme-dark.palette-${paletteId} {`)) {
      add("src/design.css", 1, `Missing dark palette selector .theme-dark.palette-${paletteId}.`);
    }
  }

  const firstComponentLine = lines.findIndex((line) => line.trim() === ".window {") + 1;
  if (firstComponentLine <= 0) {
    add("src/design.css", 1, "Cannot find component layer boundary .window.");
    return;
  }
  for (let index = firstComponentLine - 1; index < lines.length; index += 1) {
    if (hasColorLiteral(lines[index])) {
      add("src/design.css", index + 1, "Component layer contains a raw color; add or reuse a theme token.");
    }
    for (const token of paletteSeedTokens) {
      if (lines[index].includes(`var(${token}`)) {
        add("src/design.css", index + 1, `Component layer reads palette seed ${token}; use a semantic token instead.`);
      }
    }
  }
}

function hasColorLiteral(line) {
  return /(^|[^A-Za-z0-9_-])#(?:[0-9A-Fa-f]{3}|[0-9A-Fa-f]{6}|[0-9A-Fa-f]{8})(?![A-Za-z0-9_-])|\b(?:rgba?|hsla?|oklch|oklab|lab|lch)\(/.test(line);
}

function cssBlock(source, selector) {
  const start = source.indexOf(`${selector} {`);
  if (start < 0) return "";
  const bodyStart = source.indexOf("{", start);
  let depth = 0;
  for (let index = bodyStart; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") depth += 1;
    if (char === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(bodyStart + 1, index);
    }
  }
  return "";
}

function selectorIsDefined(source, selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(^|[\\s,}])${escaped}(?![A-Za-z0-9_-])`).test(source);
}

function parsePaletteIds(source) {
  const match = /export const paletteIds:[^\n=]+=\s*\[([^\]]+)\]/m.exec(source);
  if (!match) {
    add("src/lib/ui-model.ts", 1, "Cannot parse paletteIds.");
    return [];
  }
  return [...match[1].matchAll(/"([^"]+)"/g)].map((id) => id[1]);
}

function add(file, line, message) {
  findings.push(`${file}:${line} ${message}`);
}

walk(srcRoot);

if (findings.length > 0) {
  console.error("Style contract audit failed:");
  for (const finding of findings) {
    console.error(`- ${finding}`);
  }
  process.exit(1);
}

console.log("Style contract audit passed.");
