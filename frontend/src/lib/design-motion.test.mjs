import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseCss } from "svelte/compiler";

const css = readFileSync(new URL("../design.css", import.meta.url), "utf8");
const stylesheet = parseCss(css);

function stylesheetRules(nodes, media = []) {
  return nodes.flatMap((node) => {
    if (node.type === "Rule") return [{ node, media }];
    if (node.type !== "Atrule" || !node.block || node.name.endsWith("keyframes")) return [];
    return stylesheetRules(node.block.children, node.name === "media" ? [...media, node.prelude] : media);
  });
}

const rules = stylesheetRules(stylesheet.children);

function element(tag, classes = [], parent = null, states = [], attributes = {}) {
  return { tag, classes, parent, states, attributes };
}

function matchesSelector(selector, target, index = selector.children.length - 1) {
  if (!target) return false;
  const part = selector.children[index];
  if (!part.selectors.every((simple) => {
    if (simple.type === "ClassSelector") return target.classes.includes(simple.name);
    if (simple.type === "TypeSelector") return simple.name === "*" || simple.name === target.tag;
    if (simple.type === "IdSelector") return simple.name === target.attributes.id;
    if (simple.type === "AttributeSelector") {
      return simple.matcher === "="
        ? target.attributes[simple.name] === simple.value?.replace(/^['"]|['"]$/gu, "")
        : simple.matcher === null && simple.name in target.attributes;
    }
    if (simple.type !== "PseudoClassSelector") return false;
    if (["is", "where", "not"].includes(simple.name)) {
      const matched = simple.args.children.some((candidate) => matchesSelector(candidate, target));
      return simple.name === "not" ? !matched : matched;
    }
    return target.states.includes(simple.name);
  })) return false;
  if (index === 0) return true;
  if (part.combinator?.name === ">") return matchesSelector(selector, target.parent, index - 1);
  if (part.combinator?.name !== " ") return false;
  for (let ancestor = target.parent; ancestor; ancestor = ancestor.parent) {
    if (matchesSelector(selector, ancestor, index - 1)) return true;
  }
  return false;
}

function compareSpecificity(left, right) {
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

function specificity(selector) {
  return selector.children.flatMap((part) => part.selectors).reduce((total, simple) => {
    let value = [0, 0, 0];
    if (simple.type === "IdSelector") value = [1, 0, 0];
    else if (simple.type === "ClassSelector" || simple.type === "AttributeSelector") value = [0, 1, 0];
    else if (simple.type === "TypeSelector" && simple.name !== "*") value = [0, 0, 1];
    else if (simple.type === "PseudoElementSelector") value = [0, 0, 1];
    else if (simple.type === "PseudoClassSelector" && simple.name !== "where") {
      value = ["is", "not", "has"].includes(simple.name)
        ? simple.args.children.map(specificity).sort(compareSpecificity).at(-1)
        : [0, 1, 0];
    }
    return total.map((amount, index) => amount + value[index]);
  }, [0, 0, 0]);
}

// Resolve motion declarations against small DOM fixtures. This checks source order,
// :where()/state specificity and media gating; browser rendering remains separate QA.
function motionStyle(target, preference) {
  const result = {};
  for (const { node, media } of rules) {
    if (media.some((query) => /prefers-reduced-motion:\s*(reduce|no-preference)/u.exec(query)?.[1]
      && !query.includes(`prefers-reduced-motion: ${preference}`))) continue;
    const matched = node.prelude.children.filter((selector) => matchesSelector(selector, target));
    if (!matched.length) continue;
    const rank = matched.map(specificity).sort(compareSpecificity).at(-1);
    for (const declaration of node.block.children) {
      if (declaration.type !== "Declaration"
        || !["transform", "filter", "transition", "animation", "animation-name", "box-shadow", "border-color"].includes(declaration.property)) continue;
      const key = declaration.property;
      if (!result[key] || compareSpecificity(rank, result[key].rank) >= 0) {
        result[key] = { value: declaration.value, rank };
      }
    }
  }
  return Object.fromEntries(Object.entries(result).map(([key, declaration]) => [key, declaration.value]));
}

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

test("reduced motion wins for hovered and pressed controls, including switch inputs", () => {
  const root = element("div", ["design-root"]);
  const toolbar = element("div", ["modern-toolbar"], root);
  for (const state of ["hover", "active"]) {
    const button = element("button", [], toolbar, [state]);
    const normal = motionStyle(button, "no-preference");
    const reduced = motionStyle(button, "reduce");
    assert.notEqual(normal.transform, "none", `${state}: the fixture must exercise an animated control`);
    assert.ok(normal.transform, state);
    assert.equal(reduced.transform, "none", state);
    assert.equal(reduced.filter, "none", state);
    assert.equal(reduced.transition, "none", state);
    assert.equal(reduced["box-shadow"], normal["box-shadow"], `${state}: keep visual feedback`);
  }
  const input = element("input", [], element("label", ["settings-switch"], root), ["checked"], { type: "checkbox" });
  assert.notEqual(motionStyle(input, "no-preference").transition, "none");
  assert.equal(motionStyle(input, "reduce").transition, "none");
});

test("navigation and format choices remain still while other controls keep press feedback", () => {
  const root = element("div", ["design-root"]);
  for (const container of ["modern-sidebar", "settings-workspace-rail", "preset-row", "format-segments"]) {
    const parent = element("div", [container], root);
    const classes = container === "settings-workspace-rail" ? ["settings-route-card"] : [];
    for (const states of [["hover"], ["active"], ["hover", "active"]]) {
      const button = element("button", classes, parent, states);
      for (const preference of ["no-preference", "reduce"]) {
        const style = motionStyle(button, preference);
        assert.equal(style.transform, "none", `${container}: ${states} / ${preference}`);
        assert.equal(style.filter, "none", `${container}: ${states} / ${preference}`);
      }
    }
  }
});

test("choice labels animate only paint and stop transitioning with reduced motion", () => {
  const root = element("div", ["design-root"]);
  for (const className of ["create-output-choice", "content-policy-option"]) {
    const label = element("label", [className, "selected"], root, ["hover"]);
    const normal = motionStyle(label, "no-preference");
    const properties = normal.transition?.split(",").map((part) => part.trim().split(" ")[0]);
    assert.deepEqual(properties?.sort(), ["background-color", "border-color", "box-shadow"]);
    assert.equal(motionStyle(label, "reduce").transition, "none");
  }
});

test("page content fades without transforming ancestors or the sticky action bar", () => {
  const root = element("div", ["design-root"]);
  const containers = [
    ["modern-create", "create-source-list"],
    ["modern-create", "create-workflow"],
    ["modern-convert", "create-workflow"],
    ["modern-extract", "extract-layout"],
    ["settings-view", "settings-layout"],
    ["settings-view", "password-book-layout"],
    ["settings-view", "checksum-workspace"],
    ["settings-view", "duplicates-workspace"],
    ["appearance-view", "appearance-layout"],
    ["colors-view", "appearance-layout"],
    ["integration-view", "integration-layout"],
  ];
  for (const [page, container] of containers) {
    const target = element("div", [container], element("div", [page], root));
    const normal = motionStyle(target, "no-preference");
    const name = normal["animation-name"] ?? normal.animation?.split(" ")[0];
    const keyframes = stylesheet.children.find((node) => node.type === "Atrule" && node.name === "keyframes" && node.prelude === name);
    assert.ok(keyframes, `${page}: content must animate`);
    const declarations = keyframes.block.children.flatMap((frame) => frame.block.children);
    assert.ok(declarations.length > 0);
    assert.ok(declarations.every((declaration) => declaration.property === "opacity"), `${page}: fade must not establish a transformed containing block`);
    assert.equal(motionStyle(target, "reduce").animation, "none", page);
  }
  const actionBar = element("footer", ["workspace-action-bar"], element("div", ["modern-create"], root));
  assert.equal(motionStyle(actionBar, "no-preference").animation, undefined);
});
