import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { compileTestScript } from "../../tests/source.mjs";

const source = readFileSync(new URL("./clipboard.ts", import.meta.url), "utf8");
const outputText = compileTestScript(source);
const settle = async () => { for (let index = 0; index < 8; index += 1) await Promise.resolve(); };

function clipboardHarness() {
  const calls = [];
  const native = new Map();
  const children = [];
  const options = { failureAt: null, copyEvent: null, copied: true };
  let clipboard = "";
  let document;
  const selection = {
    ranges: [],
    get rangeCount() { return this.ranges.length; },
    get anchorNode() { return this.ranges[0]?.startContainer ?? null; },
    get focusNode() { return this.ranges[0]?.endContainer ?? null; },
    getRangeAt(index) { return this.ranges[index]; },
    removeAllRanges() { this.ranges = []; },
    addRange(range) { this.ranges.push(range); },
  };
  function range(node) {
    return { startContainer: node, endContainer: node, cloneRange() { return range(node); } };
  }
  class HTMLElement {
    isConnected = true;
    parentNode = null;
    focus(options) {
      calls.push(["focus", this, options]);
      document.activeElement = this;
      if (this.throwFocus) throw new Error("focus unavailable");
    }
    contains(node) { return this === node; }
  }
  class HTMLInputElement extends HTMLElement {
    selectionStart = 1;
    selectionEnd = 4;
    selectionDirection = "backward";
    setSelectionRange(start, end, direction) {
      calls.push(["input-selection", start, end, direction]);
      this.selectionStart = start;
      this.selectionEnd = end;
      this.selectionDirection = direction;
    }
  }
  class HTMLTextAreaElement extends HTMLInputElement {
    value = "";
    setAttribute(name, value) { calls.push(["attribute", name, value]); }
    select() {
      calls.push(["select", this.value]);
      document.activeElement = this;
      selection.ranges = [range(this)];
      if (options.failureAt === "select") throw new Error("selection unavailable");
    }
    remove() {
      calls.push(["remove"]);
      if (options.failureAt === "remove") throw new Error("cleanup failed");
      const index = children.indexOf(this);
      if (index !== -1) children.splice(index, 1);
      this.parentNode = null;
      this.isConnected = false;
      if (document.activeElement === this) document.activeElement = document.body;
    }
  }
  const button = new HTMLElement();
  document = {
    activeElement: button,
    body: Object.assign(new HTMLElement(), { appendChild(node) {
      calls.push(["append"]);
      children.push(node);
      node.parentNode = this;
      if (options.failureAt === "append") throw new Error("append failed");
    }, removeChild(node) {
      calls.push(["parent-remove", node.value]);
      const index = children.indexOf(node);
      if (index !== -1) children.splice(index, 1);
      node.parentNode = null;
      node.isConnected = false;
      if (document.activeElement === node) document.activeElement = this;
    } }),
    createElement(tag) {
      assert.equal(tag, "textarea");
      if (options.failureAt === "create") throw new Error("creation failed");
      return new HTMLTextAreaElement();
    },
    getSelection() {
      if (options.failureAt === "capture") throw new Error("selection unavailable");
      return selection;
    },
    execCommand(command) {
      assert.equal(command, "copy");
      calls.push(["fallback", children.at(-1)?.value]);
      options.copyEvent?.();
      if (options.failureAt === "copy") throw new Error("copy unavailable");
      if (options.copied) clipboard = children.at(-1).value;
      return options.copied;
    },
  };
  const context = { exports: {}, document, HTMLElement, HTMLInputElement, HTMLTextAreaElement,
    navigator: { clipboard: { writeText(text) {
      calls.push(["native", text]);
      return new Promise((resolve, reject) => native.set(text, {
        complete() { clipboard = text; calls.push(["write", text]); resolve(); }, reject,
      }));
    } } },
  };
  vm.runInNewContext(outputText, context);
  return { copy: context.exports.copyTextToClipboard, calls, native, children, options,
    document, selection, button, range, input: () => new HTMLInputElement(), element: () => new HTMLElement(),
    get clipboard() { return clipboard; } };
}

test("clipboard writes retain request order and restore only the fallback's own focus and selection", async () => {
  const ordered = clipboardHarness();
  const older = ordered.copy("older");
  assert.deepEqual(ordered.calls, [["native", "older"]], "an idle write must start within the click's synchronous call");
  const middle = ordered.copy("middle");
  const newest = ordered.copy("newest");
  assert.equal(await middle, "superseded");
  assert.equal(await ordered.copy("  "), "failed");
  let newestFinished = false;
  newest.then(() => { newestFinished = true; });
  await settle();
  assert.equal(newestFinished, false);
  assert.deepEqual([...ordered.native.keys()], ["older"], "a pending native write cannot be unlocked by a newer request");
  ordered.native.get("older").complete();
  assert.equal(await older, "superseded");
  assert.deepEqual([...ordered.native.keys()], ["older", "newest"]);
  ordered.native.get("newest").complete();
  assert.equal(await newest, "copied");
  assert.equal(ordered.clipboard, "newest", "the old native operation is finished before the newest can succeed");
  assert.equal(ordered.calls.some(([kind]) => kind === "select"), false);

  const rejected = clipboardHarness();
  const oldFailure = rejected.copy("old-failure");
  const newSuccess = rejected.copy("new-success");
  rejected.native.get("old-failure").reject(new Error("denied"));
  assert.equal(await oldFailure, "superseded");
  assert.equal(rejected.calls.some(([kind]) => kind === "fallback"), false);
  rejected.native.get("new-success").complete();
  assert.equal(await newSuccess, "copied");

  const cancelled = clipboardHarness();
  let valid = true;
  const inFlight = cancelled.copy("in-flight", () => valid);
  const waiting = cancelled.copy("waiting", () => valid);
  valid = false;
  await settle();
  assert.deepEqual([...cancelled.native.keys()], ["in-flight"], "cancellation must not release an unfinished native write");
  cancelled.native.get("in-flight").reject(new Error("denied"));
  assert.deepEqual(await Promise.all([inFlight, waiting]), ["superseded", "superseded"]);
  assert.equal(cancelled.calls.some(([kind]) => kind === "select"), false);
  const retry = cancelled.copy("retry");
  assert.equal(cancelled.native.has("retry"), true);
  cancelled.native.get("retry").complete();
  assert.equal(await retry, "copied");

  const fallback = clipboardHarness();
  const copied = fallback.copy("fallback");
  const currentInput = fallback.input();
  const originalRangeNode = fallback.element();
  fallback.document.activeElement = currentInput;
  fallback.selection.ranges = [fallback.range(originalRangeNode)];
  fallback.native.get("fallback").reject(new Error("denied"));
  assert.equal(await copied, "copied");
  assert.equal(fallback.clipboard, "fallback");
  assert.equal(fallback.document.activeElement, currentInput, "restore the focus at fallback time, not at click time");
  assert.deepEqual([currentInput.selectionStart, currentInput.selectionEnd, currentInput.selectionDirection], [1, 4, "backward"]);
  assert.equal(fallback.selection.ranges[0].startContainer, originalRangeNode);
  assert.equal(fallback.children.length, 0);
  assert.equal(fallback.calls.some(([kind, target]) => kind === "focus" && target === fallback.button), false);
  assert.equal(fallback.calls.find(([kind]) => kind === "focus")[2].preventScroll, true);

  const changedFocus = clipboardHarness();
  const intentionalInput = changedFocus.input();
  const intentionalRange = changedFocus.element();
  changedFocus.options.copyEvent = () => {
    changedFocus.document.activeElement = intentionalInput;
    changedFocus.selection.ranges = [changedFocus.range(intentionalRange)];
  };
  const changed = changedFocus.copy("new-focus");
  changedFocus.native.get("new-focus").reject(new Error("denied"));
  assert.equal(await changed, "copied");
  assert.equal(changedFocus.document.activeElement, intentionalInput);
  assert.equal(changedFocus.selection.ranges[0].startContainer, intentionalRange);
  assert.equal(changedFocus.calls.some(([kind]) => kind === "focus"), false);
  assert.equal(changedFocus.children.length, 0);

  const duringFallback = clipboardHarness();
  let current = true;
  duringFallback.options.copyEvent = () => { current = false; };
  const obsolete = duringFallback.copy("obsolete", () => current);
  duringFallback.native.get("obsolete").reject(new Error("denied"));
  assert.equal(await obsolete, "superseded", "fallback completion must recheck its caller's ownership");
  assert.equal(duringFallback.children.length, 0);

  for (const failureAt of ["capture", "create", "append", "select", "copy", "focus", "remove", "not-copied"]) {
    const failing = clipboardHarness();
    failing.options.failureAt = failureAt;
    if (failureAt === "focus") failing.button.throwFocus = true;
    if (failureAt === "not-copied") failing.options.copied = false;
    const failed = failing.copy(failureAt);
    failing.native.get(failureAt).reject(new Error("denied"));
    const expected = failureAt === "focus" || failureAt === "remove" ? "copied" : "failed";
    assert.equal(await failed, expected, `${failureAt} must report the actual write result without rejecting or stranding the queue`);
    assert.equal(failing.children.length, 0, `${failureAt} must clean up the temporary textarea`);
    if (failureAt === "remove") {
      assert.deepEqual(failing.calls.find(([kind]) => kind === "parent-remove"), ["parent-remove", ""]);
      assert.equal(failing.clipboard, failureAt, "cleanup must not turn an actual copy success into a reported failure");
    }
    const afterFailure = failing.copy(`after-${failureAt}`);
    assert.equal(failing.native.has(`after-${failureAt}`), true);
    failing.native.get(`after-${failureAt}`).complete();
    assert.equal(await afterFailure, "copied");
  }
});
