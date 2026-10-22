import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

const server = await createTestServer();
let taskWindowFitTarget, createTaskWindowFitter, taskWindowContentHeight, fitTaskWindow;
try {
  ({ taskWindowFitTarget, createTaskWindowFitter, taskWindowContentHeight, fitTaskWindow }
    = await server.ssrLoadModule("/src/lib/task-window-size.ts"));
} finally { await server.close(); }

function geometry(overrides = {}) {
  return {
    inner: { width: 1360, height: 720 }, outer: { width: 1360, height: 776 },
    position: { x: 400, y: 100 }, scale: 2, viewportHeight: 360,
    workArea: { position: { x: 0, y: 50 }, size: { width: 3000, height: 1800 } },
    maximized: false, fullscreen: false, ...overrides,
  };
}

const request = (height) => ({ height, margin: 24, maxHeight: 680 });

test("native task sizing grows only the height and accounts for physical DPI and decorations", () => {
  assert.deepEqual(taskWindowFitTarget(geometry(), request(500)), {
    size: { width: 1360, height: 1000 }, position: null,
  });
  assert.equal(taskWindowFitTarget(geometry(), request(300)), null);
  assert.equal(taskWindowFitTarget(geometry(), request(360)), null);
  assert.equal(taskWindowFitTarget(geometry({ maximized: true }), request(600)), null);
  assert.equal(taskWindowFitTarget(geometry({ fullscreen: true }), request(600)), null);
  assert.equal(taskWindowFitTarget(geometry({ scale: 0 }), request(600)), null);
  assert.equal(taskWindowFitTarget(geometry({ viewportHeight: 0 }), request(600)), null);
  assert.equal(taskWindowFitTarget(geometry(), request(Number.NaN)), null);
  assert.equal(taskWindowFitTarget(geometry(), { ...request(600), maxHeight: 0 }), null);
});

test("native sizing preserves measured chrome inside the native inner size without accumulating it", () => {
  const initial = geometry({ viewportHeight: 328 });
  assert.deepEqual(taskWindowFitTarget(initial, request(500)), {
    size: { width: 1360, height: 1064 }, position: null,
  }, "the 32 logical pixels unavailable to the WebView are measured, not assumed");
  const resized = geometry({ inner: { width: 1360, height: 1064 }, outer: { width: 1360, height: 1120 },
    viewportHeight: 500 });
  assert.equal(taskWindowFitTarget(resized, request(500)), null, "the same content must not grow the window again");
  assert.deepEqual(taskWindowFitTarget(resized, request(620)), {
    size: { width: 1360, height: 1304 }, position: null,
  });
  assert.deepEqual(taskWindowFitTarget(resized, request(900)), {
    size: { width: 1360, height: 1360 }, position: null,
  }, "chrome compensation still obeys the native-height design cap");
  assert.deepEqual(taskWindowFitTarget(geometry({ viewportHeight: 400 }), request(500)), {
    size: { width: 1360, height: 1000 }, position: null,
  }, "a viewport larger than the native size never subtracts required content height");
});

test("fitting respects both the design cap and available monitor work area", () => {
  assert.deepEqual(taskWindowFitTarget(geometry(), request(1200)), {
    size: { width: 1360, height: 1360 }, position: null,
  });
  const smallMonitor = geometry({
    position: { x: -1800, y: 120 },
    workArea: { position: { x: -2000, y: -1000 }, size: { width: 2000, height: 1400 } },
  });
  assert.deepEqual(taskWindowFitTarget(smallMonitor, request(1000)), {
    size: { width: 1360, height: 1248 }, position: { x: -1800, y: -952 },
  });
  assert.equal(taskWindowFitTarget(geometry({
    workArea: { position: { x: 0, y: 0 }, size: { width: 1000, height: 700 } },
  }), request(1000)), null, "a smaller monitor cannot force an existing window to shrink");
});

function deferred() {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
}

function fitterFixture(overrides = {}) {
  let current = geometry();
  const sizes = [];
  const positions = [];
  let reads = 0;
  let fitter;
  const adapter = {
    async read() { reads += 1; return current; },
    async size() { return current.inner; },
    async setSize(size) {
      sizes.push(size);
      const viewportInset = current.inner.height / current.scale - current.viewportHeight;
      current = { ...current, inner: size, outer: { width: size.width, height: size.height + 56 },
        viewportHeight: size.height / current.scale - viewportInset };
      fitter.resized(size);
    },
    async setPosition(position) { positions.push(position); current = { ...current, position }; },
    ...overrides,
  };
  fitter = createTaskWindowFitter(adapter, current.inner);
  return { fitter, adapter, sizes, positions, reads: () => reads,
    current: () => current, setCurrent: (value) => { current = value; } };
}

test("own resize events allow later expansion without shrinking or resize loops", async () => {
  const fixture = fitterFixture();
  fixture.fitter.resized(fixture.current().inner);
  await fixture.fitter.fit(request(500));
  await fixture.fitter.fit(request(500));
  await fixture.fitter.fit(request(450));
  await fixture.fitter.fit(request(620));
  assert.deepEqual(fixture.sizes, [{ width: 1360, height: 1000 }, { width: 1360, height: 1240 }]);
  assert.deepEqual(fixture.positions, []);
  fixture.fitter.destroy();
});

test("manual resizing stops future fits with and without a delivered resize event", async () => {
  for (const deliverEvent of [false, true]) {
    const fixture = fitterFixture();
    await fixture.fitter.fit(request(500));
    const size = { width: 1080, height: 800 };
    fixture.setCurrent({ ...fixture.current(), inner: size });
    if (deliverEvent) fixture.fitter.resized(size);
    await fixture.fitter.fit(request(650));
    await fixture.fitter.fit(request(680));
    assert.deepEqual(fixture.sizes, [{ width: 1360, height: 1000 }]);
    fixture.fitter.destroy();
  }
});

test("a user resize after the geometry snapshot prevents the pending mutation", async () => {
  const fixture = fitterFixture({ size: async () => ({ width: 1360, height: 800 }) });
  await fixture.fitter.fit(request(600));
  assert.deepEqual(fixture.sizes, []);
  await fixture.fitter.fit(request(680));
  assert.equal(fixture.reads(), 1);
  fixture.fitter.destroy();
});

test("destroying during native reads or the final size check prevents late writes", async () => {
  for (const phase of ["read", "size"]) {
    const gate = deferred();
    const fixture = fitterFixture({ [phase]: () => gate.promise });
    const fitting = fixture.fitter.fit(request(600));
    await Promise.resolve();
    fixture.fitter.destroy();
    gate.resolve(phase === "read" ? geometry() : geometry().inner);
    await fitting;
    assert.deepEqual(fixture.sizes, []);
    assert.deepEqual(fixture.positions, []);
  }
});

test("new content replaces an obsolete pending fit and native failures stop retries", async () => {
  const gate = deferred();
  const fixture = fitterFixture({ read: () => gate.promise });
  const fitting = fixture.fitter.fit(request(680));
  await fixture.fitter.fit(request(500));
  gate.resolve(geometry());
  await fitting;
  assert.deepEqual(fixture.sizes, [{ width: 1360, height: 1000 }]);
  fixture.fitter.destroy();
  let attempts = 0;
  const failed = fitterFixture({ setSize: async () => { attempts += 1; throw new Error("window unavailable"); } });
  await failed.fitter.fit(request(500));
  await failed.fitter.fit(request(600));
  assert.equal(attempts, 1);
  failed.fitter.destroy();
});

test("natural card measurement includes the scroll body and excludes transient feedback", () => {
  function element(height, properties = {}, body = false) {
    return {
      offsetHeight: height, scrollHeight: height,
      getClientRects: () => [{}],
      getBoundingClientRect: () => ({ height }),
      classList: { contains: (name) => body && name === "task-modal-body" },
      querySelectorAll: () => [],
      style: { getPropertyValue: (name) => properties[name] ?? "0px" },
    };
  }
  const header = element(55);
  const body = element(170, { "border-top-width": "1px", "border-bottom-width": "1px" }, true);
  body.scrollHeight = 430;
  const footer = element(68);
  const card = {
    children: [header, body, footer],
    style: { getPropertyValue: (name) => ({ "row-gap": "16px", "padding-top": "24px", "padding-bottom": "24px" })[name] ?? "0px" },
    ownerDocument: { defaultView: { getComputedStyle: (node) => node.style } },
  };
  assert.equal(taskWindowContentHeight(card), 635, "fixed card height must not truncate body.scrollHeight");
  const notice = element(30);
  const footerWithNotice = element(106, { "row-gap": "8px" });
  footerWithNotice.querySelectorAll = () => [notice];
  card.children[2] = footerWithNotice;
  assert.equal(taskWindowContentHeight(card), 635, "brief notices do not permanently enlarge a task window");
  const hidden = element(400); hidden.getClientRects = () => [];
  card.children.push(hidden);
  assert.equal(taskWindowContentHeight(card), 635);
});

test("browser previews and non-window surfaces never request native fitting", () => {
  const action = fitTaskWindow({}, false);
  action.update(true);
  action.update(false);
  action.destroy();
});
