import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseCss } from "svelte/compiler";

const css = readFileSync(new URL("../design.css", import.meta.url), "utf8");
const stylesheet = parseCss(css);

function motionRules(preference) {
  const media = stylesheet.children.find((node) => node.type === "Atrule"
    && node.name === "media"
    && node.prelude === `(prefers-reduced-motion: ${preference})`);
  assert.ok(media?.block, `Missing ${preference} motion rules`);
  return media.block.children.filter((node) => node.type === "Rule").map((rule) => ({
    selectors: rule.prelude.children.map((selector) => css.slice(selector.start, selector.end)
      .replace(/\s+/gu, " ").trim()),
    declarations: Object.fromEntries(rule.block.children
      .filter((node) => node.type === "Declaration")
      .map((node) => [node.property, node.value])),
  }));
}

test("reduced-motion animation rules remain valid without vendor progress selectors", () => {
  const rules = motionRules("reduce");
  for (const selector of [".task-modal-state.state-running", ".task-center-panel", ".app-icon"]) {
    const rule = rules.find((candidate) => candidate.selectors.includes(selector));
    assert.equal(rule?.declarations.animation, "none", selector);
    assert.ok(rule.selectors.every((entry) => !/::-(?:webkit|moz)-/u.test(entry)),
      `${selector}: unsupported vendor selectors must not invalidate ordinary animation rules`);
  }
});

test("reduced motion stops progress-fill transitions in each rendering engine", () => {
  const rules = motionRules("reduce");
  for (const pseudo of ["::-webkit-progress-value", "::-moz-progress-bar"]) {
    const rule = rules.find((candidate) => candidate.selectors.some((selector) =>
      selector.endsWith(pseudo)
      && selector.includes(".task-modal-progress-block progress")
      && selector.includes(".meter-progress")));
    assert.ok(rule, `Missing shared progress reduced-motion rule for ${pseudo}`);
    assert.equal(rule.declarations.transition, "none", pseudo);
    assert.equal(rule.declarations.animation, "none", pseudo);
    assert.ok(rule.selectors.every((selector) => selector.endsWith(pseudo)),
      `${pseudo}: native progress selectors must use separate rules`);
  }
});

test("reduced motion stops task reorder indicators without hiding them", () => {
  const rules = motionRules("reduce");
  for (const selector of [".task-center-row::before", ".task-center-row::after"]) {
    const rule = rules.find((candidate) => candidate.selectors.includes(selector));
    assert.equal(rule?.declarations.transition, "none", selector);
    assert.equal(rule.declarations.display, undefined);
    assert.equal(rule.declarations.visibility, undefined);
    assert.equal(rule.declarations.opacity, undefined);
  }
});

test("normal motion keeps the token-driven task status and progress effects", () => {
  const rules = motionRules("no-preference");
  const status = rules.find((rule) => rule.selectors.includes(".task-modal-state.state-running"));
  assert.match(status?.declarations.animation ?? "", /task-running-pulse var\(--motion-status-pulse\)/u);
  for (const pseudo of ["::-webkit-progress-value", "::-moz-progress-bar"]) {
    const fill = rules.find((rule) => rule.selectors.some((selector) => selector.endsWith(pseudo)));
    assert.match(fill?.declarations.animation ?? "", /progress-value-sheen var\(--motion-progress-sweep\)/u);
  }
});
