import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
let popupMenu, popupMenuPosition;
try {
  ({ popupMenu, popupMenuPosition } = await server.ssrLoadModule("/src/lib/popup-menu.ts"));
} finally {
  await server.close();
}

function menuFixture() {
  const listeners = new Map();
  const windowListeners = new Map();
  const documentOrder = [];
  const document = {
    activeElement: null,
    documentElement: { clientWidth: 800, clientHeight: 600 },
    addEventListener: (type, fn) => listeners.set(type, fn),
    removeEventListener: (type) => listeners.delete(type),
    querySelectorAll: () => documentOrder,
  };
  document.defaultView = {
    innerWidth: 800, innerHeight: 600,
    getComputedStyle: (element) => ({
      display: element.display ?? "block", visibility: "visible",
      getPropertyValue: (name) => ({ "--popup-viewport-padding": "12px", "--popup-gap": "8px" })[name] ?? "",
    }),
    addEventListener: (type, fn) => windowListeners.set(type, fn),
    removeEventListener: (type) => windowListeners.delete(type),
  };
  function element(name, attributes = {}) {
    return {
      name, ownerDocument: document, isConnected: true, tabIndex: 0, parentElement: null,
      hidden: false, inert: false, disabled: false, attributes: { ...attributes },
      matches(selector) { return selector === ":disabled" && this.disabled; },
      hasAttribute(name) { return name in this.attributes; },
      getAttribute(name) { return this.attributes[name] ?? null; },
      setAttribute(name, value) { this.attributes[name] = value; },
      removeAttribute(name) { delete this.attributes[name]; },
      compareDocumentPosition(other) {
        return documentOrder.indexOf(this) < documentOrder.indexOf(other) ? 4 : 2;
      },
      getBoundingClientRect: () => ({ left: 600, top: 20, right: 720, bottom: 50 }),
      getClientRects: () => [{}],
      focus() {
        document.activeElement = this;
        listeners.get("focusin")?.({ target: this });
      },
    };
  }
  document.body = element("body");
  const anchor = element("opener");
  anchor.getBoundingClientRect = () => ({ left: 600, top: 20, right: 720, bottom: 50 });
  const hidden = element("hidden"); hidden.hidden = true;
  const disabled = element("disabled"); disabled.disabled = true;
  const unavailable = element("aria-disabled", { "aria-disabled": "true" });
  const first = element("first");
  const second = element("second");
  const items = [hidden, disabled, unavailable, first, second];
  const values = new Map();
  const nodeListeners = new Map();
  const node = element("menu");
  node.style = {
    setProperty: (key, value) => values.set(key, value),
    removeProperty: (key) => values.delete(key),
  };
  node.querySelectorAll = () => items;
  node.getBoundingClientRect = () => ({ width: 236, height: 264 });
  node.contains = (target) => target === node || items.includes(target);
  node.addEventListener = (type, fn) => nodeListeners.set(type, fn);
  node.removeEventListener = (type) => nodeListeners.delete(type);
  const previous = element("previous");
  const next = element("next");
  const hiddenOutside = element("hidden outside"); hiddenOutside.hidden = true;
  const disabledOutside = element("disabled outside"); disabledOutside.disabled = true;
  const inertOutside = element("inert outside"); inertOutside.inert = true;
  documentOrder.push(previous, anchor, hiddenOutside, disabledOutside, inertOutside, node, ...items, next);
  const closed = [];
  document.activeElement = anchor;
  const options = { anchor, onClose: (restore) => closed.push(restore) };
  const action = popupMenu(node, options);
  function key(key, extra = {}) {
    const event = {
      key, defaultPrevented: false, stopped: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this.stopped = true; }, ...extra,
    };
    nodeListeners.get("keydown")?.(event);
    return event;
  }
  return { document, anchor, node, items, first, second, values, key, closed,
    action, options, element, windowListeners, nodeListeners, listeners,
    previous, next, documentOrder };
}

test("anchored menus use actual dimensions, flip upward and remain inside the viewport", () => {
  const viewport = { width: 800, height: 600 };
  const size = { width: 236, height: 264 };
  assert.deepEqual(popupMenuPosition(size, viewport, 12, 8, {
    left: 600, right: 720, top: 20, bottom: 50,
  }), { x: 484, y: 58 });
  assert.deepEqual(popupMenuPosition(size, viewport, 12, 8, {
    left: 600, right: 790, top: 550, bottom: 580,
  }), { x: 552, y: 278 });
  assert.deepEqual(popupMenuPosition(size, viewport, 12, 8, undefined, { x: 795, y: 595 }), { x: 552, y: 324 });
  assert.deepEqual(popupMenuPosition(size, viewport, 12, 8, undefined, { x: -10, y: -20 }), { x: 12, y: 12 });
  assert.deepEqual(popupMenuPosition({ width: 900, height: 700 }, viewport, 12, 8), { x: 12, y: 12 });
});

test("menus focus an enabled visible item and contain arrow navigation", async () => {
  const fixture = menuFixture();
  await Promise.resolve(); await Promise.resolve();
  assert.equal(fixture.document.activeElement, fixture.first);
  assert.ok(fixture.items.every((item) => item.tabIndex === -1));
  for (const [key, expected] of [["ArrowDown", fixture.second], ["ArrowDown", fixture.first],
    ["ArrowUp", fixture.second], ["Home", fixture.first], ["End", fixture.second]]) {
    const event = fixture.key(key);
    assert.equal(fixture.document.activeElement, expected);
    assert.equal(event.defaultPrevented, true);
    assert.equal(event.stopped, true, "row or application keyboard handlers must not run");
  }
  fixture.second.disabled = true;
  fixture.key("ArrowDown");
  assert.equal(fixture.document.activeElement, fixture.first, "availability is checked on each keypress");
  fixture.action.destroy();
});

test("Escape closes only the menu; Tab resumes the opener's page order", async () => {
  for (const shiftKey of [false, true]) {
    const fixture = menuFixture();
    await Promise.resolve(); await Promise.resolve();
    const event = fixture.key("Tab", { shiftKey });
    assert.equal(fixture.document.activeElement, shiftKey ? fixture.previous : fixture.next);
    assert.equal(event.defaultPrevented, true, "focus moves explicitly before WebKit loses the removed menu");
    assert.equal(event.stopped, true);
    assert.deepEqual(fixture.closed, [false]);
    fixture.action.destroy();
  }
  const fixture = menuFixture();
  const ignored = fixture.key("Escape", { defaultPrevented: true });
  assert.equal(ignored.stopped, false);
  assert.deepEqual(fixture.closed, []);
  const escape = fixture.key("Escape");
  assert.equal(escape.defaultPrevented, true);
  assert.equal(escape.stopped, true);
  assert.deepEqual(fixture.closed, [true]);
  fixture.action.destroy();
});

test("menu Tab exit respects positive tabindex, context rows and page boundaries", async () => {
  for (const [from, backward, to] of [
    ["firstPositive", false, "secondPositive"], ["secondPositive", false, "previous"],
    ["firstPositive", true, "next"], ["next", false, "firstPositive"],
    ["anchor", false, "next"], ["anchor", true, "previous"],
  ]) {
    const fixture = menuFixture();
    await Promise.resolve(); await Promise.resolve();
    fixture.firstPositive = fixture.element("first positive"); fixture.firstPositive.tabIndex = 1;
    fixture.secondPositive = fixture.element("second positive"); fixture.secondPositive.tabIndex = 2;
    fixture.documentOrder.push(fixture.secondPositive, fixture.firstPositive);
    fixture.anchor.tabIndex = -1;
    fixture.action.update({ ...fixture.options, anchor: fixture[from] });
    fixture.key("Tab", { shiftKey: backward });
    assert.equal(fixture.document.activeElement, fixture[to], `${from} ${backward ? "backward" : "forward"}`);
    assert.deepEqual(fixture.closed, [false], "destination focus must not close the menu a second time");
    fixture.action.destroy();
  }
});

test("focus moving outside closes the menu without restoring or overriding its new destination", async () => {
  const fixture = menuFixture();
  await Promise.resolve(); await Promise.resolve();
  fixture.anchor.focus();
  assert.deepEqual(fixture.closed, [], "the opener owns its toggle behavior");
  fixture.first.focus();
  fixture.next.focus();
  assert.deepEqual(fixture.closed, [false]);
  assert.equal(fixture.document.activeElement, fixture.next);
  fixture.previous.focus();
  fixture.key("Escape");
  assert.deepEqual(fixture.closed, [false], "one dismissal per menu lifetime");
  fixture.action.destroy();
});

test("an unavailable menu keeps a keyboard exit and cannot navigate the row underneath", async () => {
  const fixture = menuFixture();
  fixture.first.disabled = true;
  fixture.second.disabled = true;
  await Promise.resolve(); await Promise.resolve();
  assert.equal(fixture.document.activeElement, fixture.node);
  const arrow = fixture.key("ArrowDown");
  assert.equal(arrow.defaultPrevented, true);
  assert.equal(arrow.stopped, true);
  fixture.key("Escape");
  assert.deepEqual(fixture.closed, [true]);
  fixture.action.destroy();
});

test("menu position follows viewport changes and point updates without refocusing", async () => {
  const fixture = menuFixture();
  await Promise.resolve(); await Promise.resolve();
  assert.equal(fixture.values.get("--popup-left"), "484px");
  assert.equal(fixture.values.get("--popup-top"), "58px");
  fixture.document.documentElement.clientWidth = 500;
  fixture.windowListeners.get("resize")();
  assert.equal(fixture.values.get("--popup-left"), "252px");
  fixture.second.focus();
  fixture.action.update({ ...fixture.options, point: { x: 100, y: 550 } });
  assert.equal(fixture.values.get("--popup-left"), "100px");
  assert.equal(fixture.values.get("--popup-top"), "324px");
  assert.equal(fixture.document.activeElement, fixture.second);
  fixture.action.destroy();
  assert.equal(fixture.values.size, 0);
  assert.equal(fixture.windowListeners.size, 0);
  assert.equal(fixture.nodeListeners.size, 0);
  assert.equal(fixture.listeners.size, 0);
});

test("closing during mount or moving focus elsewhere prevents delayed menu focus", async () => {
  for (const destroy of [false, true]) {
    const fixture = menuFixture();
    const other = fixture.element("another input");
    other.focus();
    if (destroy) fixture.action.destroy();
    await Promise.resolve(); await Promise.resolve();
    assert.equal(fixture.document.activeElement, other);
    if (!destroy) fixture.action.destroy();
  }
});
