import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseCss } from "svelte/compiler";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { contrastRatio, deriveCustomPaletteTokens, parseHexColor } =
  await server.ssrLoadModule("/src/lib/theme.ts");
const { builtInPalettes } = await server.ssrLoadModule("/src/lib/ui-model.ts");

const css = readFileSync(new URL("../design.css", import.meta.url), "utf8");
const rules = parseCss(css).children.filter((node) => node.type === "Rule").map((rule) => ({
  selectors: rule.prelude.children.map((selector) => css.slice(selector.start, selector.end).trim()),
  declarations: Object.fromEntries(rule.block.children.filter((node) => node.type === "Declaration")
    .map((node) => [node.property, node.value])),
}));

function declarations(selector) {
  const matches = rules.filter((rule) => rule.selectors.includes(selector));
  assert.ok(matches.length > 0, `Missing CSS selector ${selector}`);
  return Object.assign({}, ...matches.map((rule) => rule.declarations));
}

function builtInTokens(palette, theme) {
  const tokens = { ...declarations(":root"), ...declarations(`.palette-${palette}`) };
  return theme === "dark"
    ? { ...tokens, ...declarations(".theme-dark"), ...declarations(`.theme-dark.palette-${palette}`) }
    : { ...tokens, ...declarations(`.palette-${palette}:not(.theme-dark)`) };
}

function resolveValue(value, tokens, seen = new Set()) {
  return value.replace(/var\((--[\w-]+)\)/gu, (_match, token) => {
    assert.equal(seen.has(token), false, `Circular color token ${token}`);
    assert.equal(typeof tokens[token], "string", `Missing color token ${token}`);
    return resolveValue(tokens[token], tokens, new Set([...seen, token]));
  });
}

// Evaluate the CSS's declared opaque sRGB colors, including its hover color-mix.
// Keep fractional channels: CSS interpolation does not round to eight-bit hex.
function cssColor(value, tokens) {
  const resolved = resolveValue(value, tokens).trim();
  const named = { white: "#ffffff", black: "#000000" };
  if (named[resolved]) return parseHexColor(named[resolved]);
  if (/^#[0-9a-f]{3}(?:[0-9a-f]{3})?$/iu.test(resolved)) return parseHexColor(resolved);
  const mix = /^color-mix\(in srgb,\s*(.+)\s+([\d.]+)%,\s*(.+)\s+([\d.]+)%\)$/u.exec(resolved);
  assert.ok(mix, `Expected an opaque solid button color, received ${resolved}`);
  const left = cssColor(mix[1], tokens);
  const right = cssColor(mix[3], tokens);
  const leftWeight = Number(mix[2]);
  const rightWeight = Number(mix[4]);
  assert.equal(leftWeight + rightWeight, 100, "Primary colors must stay opaque");
  return Object.fromEntries(["r", "g", "b"].map((channel) => [channel,
    (left[channel] * leftWeight + right[channel] * rightWeight) / 100,
  ]));
}

function assertPrimaryContrast(tokens, label) {
  for (const state of ["", "-hover"]) {
    const background = cssColor(tokens[`--button-primary-bg${state}`], tokens);
    const foreground = cssColor(tokens[`--button-primary-fg${state}`], tokens);
    const ratio = contrastRatio(foreground, background);
    assert.ok(ratio >= 4.5, `${label} ${state || "normal"}: ${ratio.toFixed(4)}:1 is below 4.5:1`);
  }
}

test("every built-in palette keeps primary text readable in normal and hover colors", () => {
  for (const palette of builtInPalettes) {
    for (const theme of ["light", "dark"]) {
      assertPrimaryContrast(builtInTokens(palette.id, theme), `${palette.id}/${theme}`);
    }
  }
});

test("custom primary colors keep text readable across bright colors and foreground-switch boundaries", () => {
  const accents = [
    "#000000", "#FFFFFF", "#071216", "#FF0000", "#00FF00", "#0000FF",
    "#FFFF00", "#00FFFF", "#FF00FF", "#FFF150", "#0A7C86", "#6E506F",
    "#707070", "#737373", "#747474", "#757575", "#767676", "#777777",
    "#787878", "#7F7F7F", "#808080", "#888888",
  ];
  for (const accent of accents) {
    for (const theme of ["light", "dark"]) {
      for (const contrastGuard of [false, true]) {
        assertPrimaryContrast(deriveCustomPaletteTokens(accent, theme, contrastGuard),
          `${accent}/${theme}/guard=${contrastGuard}`);
      }
    }
  }
});
