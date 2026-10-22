import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { compileTestScript } from "../../tests/source.mjs";

function harness() {
  const ticks = [];
  const document = {
    activeElement: null,
    defaultView: { getComputedStyle: (element) => element.style },
    modals: [],
    querySelectorAll() { return this.modals; },
  };
  class Element {
    ownerDocument = document;
    isConnected = true;
    tabIndex = 0;
    disabled = false;
    parentElement = null;
    attributes = new Map();
    style = {};
    rendered = true;
    children = [];
    constructor(parent = null) {
      this.parentElement = parent;
      parent?.children.push(this);
    }
    hasAttribute(name) { return this.attributes.has(name); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    matches() { return this.disabled; }
    getClientRects() { return this.rendered ? [{}] : []; }
    querySelectorAll() { return this.children.flatMap((child) => [child, ...child.querySelectorAll()]); }
    contains(target) {
      for (let current = target; current; current = current.parentElement) {
        if (current === this) return true;
      }
      return false;
    }
    focus() { document.activeElement = this; }
  }
  document.body = new Element();
  const root = new Element(document.body);
  root.tabIndex = -1;
  const target = new Element(document.body);
  const exports = {};
  const context = vm.createContext({
    exports,
    require: () => ({ tick: () => new Promise((resolve) => ticks.push(resolve)) }),
  });
  const source = readFileSync(new URL("./modal-focus.ts", import.meta.url), "utf8");
  vm.runInContext(compileTestScript(source), context);
  return {
    ...exports, document, Element, root, target,
    flush: async () => { ticks.splice(0).forEach((resolve) => resolve()); await Promise.resolve(); },
  };
}

test("modal Tab skips hidden, inert, disabled, and untabbable controls and their hidden ancestors", () => {
  const run = harness();
  const first = new run.Element(run.root);
  for (const unavailable of ["hidden", "inert", "aria-hidden", "display", "visibility", "disabled", "tabindex", "unrendered"]) {
    const ancestor = new run.Element(run.root);
    ancestor.tabIndex = -1;
    const child = new run.Element(ancestor);
    if (unavailable === "hidden" || unavailable === "inert") ancestor.attributes.set(unavailable, "");
    if (unavailable === "aria-hidden") ancestor.attributes.set(unavailable, "true");
    if (unavailable === "display") ancestor.style.display = "none";
    if (unavailable === "visibility") ancestor.style.visibility = "hidden";
    if (unavailable === "disabled") child.disabled = true;
    if (unavailable === "tabindex") child.tabIndex = -1;
    if (unavailable === "unrendered") child.rendered = false;
  }
  const last = new run.Element(run.root);
  let prevented = 0;
  const tab = (shiftKey = false) => run.trapModalFocus({
    key: "Tab", shiftKey, preventDefault: () => { prevented += 1; },
  }, run.root);
  first.focus();
  tab(true);
  assert.equal(run.document.activeElement, last);
  tab();
  assert.equal(run.document.activeElement, first);
  first.disabled = true;
  tab();
  assert.equal(run.document.activeElement, last, "a control disabled while focused does not strand keyboard navigation");
  last.attributes.set("hidden", "");
  tab();
  assert.equal(run.document.activeElement, run.root);
  assert.equal(prevented, 4);
});

test("return focus waits for removal and restores the original control", async () => {
  const run = harness();
  const input = new run.Element(run.root);
  input.focus();
  run.restoreModalFocus(run.root, run.target);
  assert.equal(run.document.activeElement, input);
  run.root.isConnected = false;
  run.document.activeElement = run.document.body;
  await run.flush();
  assert.equal(run.document.activeElement, run.target);
});

test("return focus respects newer user focus and newly opened modals, including before their first focus", async () => {
  for (const destination of ["control", "new-modal", "new-modal-before-focus"]) {
    const run = harness();
    run.root.focus();
    run.restoreModalFocus(run.root, run.target);
    const next = new run.Element(run.document.body);
    if (destination !== "control") run.document.modals.push(next);
    if (destination === "new-modal-before-focus") run.document.body.focus();
    else next.focus();
    const expected = run.document.activeElement;
    await run.flush();
    assert.equal(run.document.activeElement, expected);
  }
});

test("return focus skips unavailable sources and permits a still-open parent dialog", async () => {
  for (const state of ["removed", "disabled", "hidden", "inert", "aria-hidden", "display", "visibility", "unrendered"]) {
    const run = harness();
    const parent = new run.Element(run.document.body);
    run.target.parentElement = parent;
    run.root.focus();
    run.restoreModalFocus(run.root, run.target);
    if (state === "removed") run.target.isConnected = false;
    if (state === "disabled") run.target.disabled = true;
    if (["hidden", "inert"].includes(state)) parent.attributes.set(state, "");
    if (state === "aria-hidden") parent.attributes.set(state, "true");
    if (state === "display") parent.style.display = "none";
    if (state === "visibility") parent.style.visibility = "hidden";
    if (state === "unrendered") run.target.rendered = false;
    await run.flush();
    assert.equal(run.document.activeElement, run.root, state);
  }
  const run = harness();
  const parentDialog = new run.Element(run.document.body);
  run.target.parentElement = parentDialog;
  run.document.modals.push(parentDialog);
  run.root.focus();
  run.restoreModalFocus(run.root, run.target);
  await run.flush();
  assert.equal(run.document.activeElement, run.target);
});
