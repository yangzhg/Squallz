import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("actionable toasts stay visible while timed toasts and the queue keep working", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

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
        ["actionable", "danger", "timed info"],
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
        ["actionable", "danger", "queued warning"],
      );
      assert.deepEqual(
        timers.map((timer) => timer.delay),
        [4000, 6000],
      );

      pushToast({ kind: "success", title: "queued success" });
      infoTimer.callback();
      assert.deepEqual(
        Array.from(toasts(), (toast) => toast.title),
        ["actionable", "danger", "queued warning"],
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
        ["second persistent result", "third persistent result", "urgent recovery"],
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
