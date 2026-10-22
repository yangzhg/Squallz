import assert from "node:assert/strict";
import test from "node:test";

import { createTestServer } from "../../tests/runtime.mjs";

test("modal views hide toast controls without discarding their notifications", async () => {
  const server = await createTestServer();
  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Host } = await server.ssrLoadModule("/src/components/ToastHost.svelte");
    const { pushToast, toasts } = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    pushToast({ kind: "warning", title: "Archive needs review", action: { label: "Review", run() {} } });
    const body = (blocked) => render(Host, { props: { rootClass: "themed-root", blocked } }).body;
    assert.match(body(false), /Archive needs review/);
    assert.doesNotMatch(body(true), /<button|Archive needs review/);
    assert.equal(toasts().length, 1);
    assert.match(body(false), /Archive needs review/);
  } finally {
    await server.close();
  }
});

test("actionable toasts stay visible while timed toasts and the queue keep working", async (context) => {
  const server = await createTestServer();
  context.mock.method(performance, "now", () => 0);

  try {
    const { dismissToast, pushToast, removeToast, toasts } = await server.ssrLoadModule(
      "/src/lib/toasts.svelte.ts",
    );
    const originalSetTimeout = globalThis.setTimeout;
    const originalClearTimeout = globalThis.clearTimeout;
    const timers = [];
    const clearedTimers = [];
    globalThis.setTimeout = (callback, delay = 0) => {
      assert.equal(typeof callback, "function");
      timers.push({ callback, delay: Number(delay) });
      return timers.length;
    };
    globalThis.clearTimeout = (timer) => {
      clearedTimers.push(timer);
    };

    try {
      pushToast({
        kind: "success",
        title: "actionable",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({ kind: "danger", title: "danger" });
      pushToast({ kind: "info", title: "timed info" });

      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["danger", "actionable", "timed info"],
      );
      assert.deepEqual(
        timers.map((timer) => timer.delay),
        [4000],
      );

      pushToast({ kind: "warning", title: "queued warning" });
      const infoTimer = timers[0];
      infoTimer.callback();

      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["danger", "queued warning", "actionable"],
      );
      assert.deepEqual(
        timers.map((timer) => timer.delay),
        [4000, 6000],
      );

      pushToast({ kind: "success", title: "queued success" });
      infoTimer.callback();
      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["danger", "queued warning", "actionable"],
      );
      assert.deepEqual(
        timers.map((timer) => timer.delay),
        [4000, 6000],
      );

      const actionable = toasts().find((toast) => toast.title === "actionable");
      assert.ok(actionable);
      dismissToast(actionable.id);
      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["danger", "queued warning", "timed info"],
      );
      assert.deepEqual(
        timers.map((timer) => timer.delay),
        [4000, 6000, 4000],
      );

      while (toasts().length > 0) dismissToast(toasts()[0].id);
      pushToast({
        kind: "success",
        title: "first persistent result",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({
        kind: "success",
        title: "second persistent result",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({
        kind: "info",
        title: "third persistent result",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({ kind: "danger", title: "urgent recovery" });

      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["urgent recovery", "second persistent result", "third persistent result"],
      );

      const urgent = toasts().find((toast) => toast.title === "urgent recovery");
      assert.ok(urgent);
      dismissToast(urgent.id);
      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["second persistent result", "third persistent result", "first persistent result"],
      );

      while (toasts().length > 0) dismissToast(toasts()[0].id);
      pushToast({
        kind: "success",
        title: "async result",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({
        kind: "success",
        title: "neighbor result",
        action: { label: "Reveal", run: () => undefined },
      });
      pushToast({
        kind: "info",
        title: "third result",
        action: { label: "Reveal", run: () => undefined },
      });
      const asyncResult = toasts().find((toast) => toast.title === "async result");
      assert.ok(asyncResult);
      pushToast({ kind: "danger", title: "recovery during action" });
      removeToast(asyncResult.id);
      const recoveryDuringAction = toasts().find(
        (toast) => toast.title === "recovery during action",
      );
      assert.ok(recoveryDuringAction);
      dismissToast(recoveryDuringAction.id);
      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["neighbor result", "third result"],
      );
      assert.ok(clearedTimers.length >= 2);
    } finally {
      globalThis.setTimeout = originalSetTimeout;
      globalThis.clearTimeout = originalClearTimeout;
    }
  } finally {
    await server.close();
  }
});

async function withToastClock(context, run) {
  const server = await createTestServer();
  let notices;
  try {
    notices = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
    let now = 0;
    let nextHandle = 1;
    const pending = new Map();
    context.mock.method(performance, "now", () => now);
    context.mock.method(globalThis, "setTimeout", (callback, delay) => {
      const handle = nextHandle++;
      pending.set(handle, { at: now + delay, callback });
      return handle;
    });
    context.mock.method(globalThis, "clearTimeout", (handle) => pending.delete(handle));
    const advance = (elapsed) => {
      const end = now + elapsed;
      while (true) {
        const next = [...pending].sort((a, b) => a[1].at - b[1].at)[0];
        if (!next || next[1].at > end) break;
        now = next[1].at;
        pending.delete(next[0]);
        next[1].callback();
      }
      now = end;
    };
    await run({ ...notices, advance, pending });
  } finally {
    while (notices?.toasts().length) notices.dismissToast(notices.toasts()[0].id);
    context.mock.restoreAll();
    await server.close();
  }
}

test("blocked notifications retain their remaining visible time and newly blocked notices get their full time", (context) =>
  withToastClock(context, ({ pushToast, toasts, setToastsBlocked, advance, pending }) => {
    pushToast({ kind: "info", title: "partly read" });
    advance(1500);
    const staleCallback = [...pending.values()][0].callback;
    setToastsBlocked(true);
    pushToast({ kind: "warning", title: "arrived while blocked" });
    advance(60_000);
    staleCallback();
    assert.deepEqual(toasts().map((toast) => toast.title), ["arrived while blocked", "partly read"]);
    assert.equal(pending.size, 0);

    setToastsBlocked(false);
    advance(2499);
    assert.equal(toasts().length, 2);
    advance(1);
    assert.deepEqual(toasts().map((toast) => toast.title), ["arrived while blocked"]);
    advance(3499);
    assert.equal(toasts().length, 1);
    advance(1);
    assert.equal(toasts().length, 0);
  }));

test("priority preemption and keyed removal preserve blocked queues and visible-time budgets", (context) =>
  withToastClock(context, ({ pushToast, toasts, dismissToast, removeToastByKey, setToastsBlocked, advance }) => {
    pushToast({ kind: "info", title: "partly read", key: "ordinary" });
    pushToast({ kind: "success", title: "action", action: { label: "Show", run() {} } });
    pushToast({ kind: "danger", title: "recovery" });
    advance(1000);
    setToastsBlocked(true);
    pushToast({ kind: "warning", title: "needs attention", key: "attention" });
    pushToast({ kind: "info", title: "queued result", key: "consumed-result" });
    removeToastByKey("consumed-result");
    advance(60_000);
    setToastsBlocked(false);
    assert.deepEqual(toasts().map((toast) => toast.title), ["recovery", "needs attention", "action"]);
    dismissToast(toasts().find((toast) => toast.title === "action").id);
    advance(2999);
    assert.ok(toasts().some((toast) => toast.title === "partly read"));
    advance(1);
    assert.deepEqual(toasts().map((toast) => toast.title), ["recovery", "needs attention"]);
    advance(3000);
    assert.deepEqual(toasts().map((toast) => toast.title), ["recovery"]);
    dismissToast(toasts()[0].id);
    assert.equal(toasts().length, 0);
  }));
