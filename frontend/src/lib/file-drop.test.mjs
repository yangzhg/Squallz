import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import { englishTranslate, withTestServer } from "../../tests/runtime.mjs";
import { evaluateFunctions, readSvelteScript } from "../../tests/source.mjs";

function transfer(files, text = {}, types = ["Files"]) {
  return { files, types, getData: (type) => text[type] ?? "" };
}

test("blocked file drops report through the active surface without changing the workspace", async () => {
  const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");
  const expected = englishTranslate("gui.drop.return_to_workspace");
  for (const taskWindowMode of [true, false]) {
    const notices = [];
    const { handleDroppedPaths } = evaluateFunctions(source, ["handleDroppedPaths"], {
      taskWindowMode, blockingModalVisible: () => !taskWindowMode, modeSelectionBlocked: false,
      uniqueNonEmptyPaths: (paths) => paths, tr: englishTranslate, showNotice: (message) => notices.push(message),
    });
    await handleDroppedPaths(["/example.zip"], "native");
    assert.deepEqual(notices, [expected]);
  }
  await withTestServer(async (server) => {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: Editor } = await server.ssrLoadModule("/src/components/ArchiveEntryEditor.svelte");
    const { default: Publisher } = await server.ssrLoadModule("/src/components/SfxPublishDialog.svelte");
    const editor = render(Editor, { props: { notice: expected, error: "Name already exists" } }).body;
    const publisher = render(Publisher, { props: { notice: expected, source: "/input", output: "/output" } }).body;
    for (const body of [editor, publisher]) {
      assert.ok(body.includes(`class="surface-notice" role="status">${expected}</div>`));
    }
    assert.match(editor, /role="alert">Name already exists/u);
  });
});

test("file drops preserve literal paths and accept only complete platform file URLs", () =>
  withTestServer(async (server) => {
    const { domDropPaths } = await server.ssrLoadModule("/src/lib/file-drop.ts");
    for (const platform of ["macos", "linux"]) {
      const data = transfer([{ path: "/archives/%20 literal.sqz " }, { name: "basename.zip" }], {
        "text/uri-list": [
          "# archive list", "file:///archives/first%20file.zip", "file:///archives/bad%ZZ",
          "file://other-host/share/foreign.zip", "https://example.org/remote.zip", "relative.zip",
        ].join("\r\n"),
        "text/plain": "/ignored-fallback.zip",
      });
      assert.deepEqual(domDropPaths(data, platform), ["/archives/%20 literal.sqz ", "/archives/first file.zip"]);
    }
    const data = transfer([{ path: "relative.zip" }], {
      "text/plain": [
        "C:\\files\\local.zip", "file:///D:/files/more%20files.zip", "file://server/share/remote.zip",
        "file:///missing-drive.zip", "file://server/", "\\\\server", "C:relative.zip", "https://remote/path.zip",
      ].join("\n"),
    });
    assert.deepEqual(domDropPaths(data, "windows"), [
      "C:\\files\\local.zip", "D:/files/more files.zip", "//server/share/remote.zip",
    ]);
  }));

function browser(context, native = false) {
  const saved = ["window", "Element", "isTauri"].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]);
  context.after(() => {
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  globalThis.window = new EventTarget();
  globalThis.Element = class { closest() { return this; } };
  globalThis.isTauri = native;
  const calls = { drop: [], hover: [], missing: 0 };
  const options = {
    platform: () => "macos",
    available: () => true,
    drop: (...args) => calls.drop.push(args),
    hover: (active) => calls.hover.push(active),
    missingPath: () => calls.missing++,
  };
  function drag(type, dataTransfer, target) {
    const event = Object.assign(new Event(type, { cancelable: true }), { dataTransfer });
    if (target) Object.defineProperty(event, "target", { value: target });
    window.dispatchEvent(event);
    return event;
  }
  return { options, calls, drag };
}

test("DOM file dragging reports missing paths, preserves editable text drops and removes listeners", (context) =>
  withTestServer(async (server) => {
    const { options, calls, drag } = browser(context);
    const { listenForFileDrops } = await server.ssrLoadModule("/src/lib/file-drop.ts");
    const dispose = listenForFileDrops(options);
    const files = transfer([{ name: "archive.zip" }]);
    const filePaths = transfer([{ path: "/archive.zip" }]);
    assert.equal(drag("dragover", files).defaultPrevented, true);
    assert.deepEqual(calls.hover, [true]);
    drag("drop", files);
    assert.equal(calls.missing, 1);
    const text = transfer([], { "text/plain": "/entry.zip" }, ["text/plain"]);
    assert.equal(drag("drop", text, new Element()).defaultPrevented, false);
    assert.deepEqual(calls.drop, []);
    drag("drop", filePaths, new Element());
    assert.deepEqual(calls.drop, [[["/archive.zip"], "dom"]]);
    options.available = () => false;
    drag("dragover", files);
    assert.equal(files.dropEffect, "none");
    drag("drop", files);
    assert.equal(calls.missing, 1);
    assert.equal(calls.hover.at(-1), false);
    dispose();
    assert.equal(drag("drop", filePaths).defaultPrevented, false);
    assert.equal(calls.drop.length, 1);
  }));

function nativeEvents(onRegister) {
  window.crypto = webcrypto;
  mockWindows("main");
  const notify = new Map();
  const removed = [];
  const registered = Promise.withResolvers();
  mockIPC(async (command, args) => {
    if (command === "plugin:event|unlisten") {
      removed.push(args.event);
      return;
    }
    assert.equal(command, "plugin:event|listen");
    assert.deepEqual(args.target, { kind: "WebviewWindow", label: "main" });
    assert.equal(notify.has(args.event), false);
    notify.set(args.event, window.__TAURI_INTERNALS__.callbacks.get(args.handler));
    if (args.event === "tauri://drag-drop") registered.resolve();
    await onRegister?.(args.event);
    return args.handler;
  });
  return { notify, removed, registered: registered.promise };
}

test("the native drop owns a Tauri gesture and ignores DOM duplicates and late events", (context) =>
  withTestServer(async (server) => {
    const { options, calls, drag } = browser(context, true);
    const { notify, removed, registered } = nativeEvents();
    const { listenForFileDrops } = await server.ssrLoadModule("/src/lib/file-drop.ts");
    const dispose = listenForFileDrops(options);
    await registered;
    await setImmediate();
    const paths = ["/folder/archive.zip"];
    notify.get("tauri://drag-enter")({ payload: { paths } });
    assert.deepEqual(calls.hover, [true]);
    assert.equal(drag("drop", transfer([{ path: paths[0] }])).defaultPrevented, true);
    notify.get("tauri://drag-drop")({ payload: { paths } });
    assert.deepEqual(calls.drop, [[paths, "native"]]);
    assert.equal(calls.hover.at(-1), false);
    dispose();
    await setImmediate();
    assert.deepEqual(removed.sort(), [...notify.keys()].sort());
    notify.get("tauri://drag-drop")({ payload: { paths } });
    assert.equal(calls.drop.length, 1);
  }));

test("a partially registered native drop releases its listeners before using the DOM fallback", (context) =>
  withTestServer(async (server) => {
    const { options, calls, drag } = browser(context, true);
    const { notify, removed, registered } = nativeEvents((event) => {
      if (event === "tauri://drag-drop") throw new Error("registration failed");
    });
    const { listenForFileDrops } = await server.ssrLoadModule("/src/lib/file-drop.ts");
    const dispose = listenForFileDrops(options);
    await registered;
    await setImmediate();
    assert.deepEqual(removed.sort(), ["tauri://drag-enter", "tauri://drag-leave"]);
    const paths = ["/folder/archive.zip"];
    notify.get("tauri://drag-enter")({ payload: { paths } });
    notify.get("tauri://drag-drop")({ payload: { paths } });
    assert.deepEqual(calls.hover, [false]);
    drag("drop", transfer([{ path: paths[0] }]));
    assert.deepEqual(calls.drop, [[paths, "dom"]]);
    dispose();
    assert.equal(removed.length, 2);
  }));

test("unmounting during native registration also releases the in-flight listener", (context) =>
  withTestServer(async (server) => {
    const { options, calls } = browser(context, true);
    const waiting = Promise.withResolvers();
    const finish = Promise.withResolvers();
    const { notify, removed } = nativeEvents((event) => {
      if (event === "tauri://drag-leave") {
        waiting.resolve();
        return finish.promise;
      }
    });
    const { listenForFileDrops } = await server.ssrLoadModule("/src/lib/file-drop.ts");
    const dispose = listenForFileDrops(options);
    await waiting.promise;
    dispose();
    finish.resolve();
    await setImmediate();
    assert.deepEqual(removed.sort(), ["tauri://drag-enter", "tauri://drag-leave"]);
    assert.equal(notify.has("tauri://drag-drop"), false);
    notify.get("tauri://drag-enter")({ payload: { paths: ["/archive.zip"] } });
    assert.deepEqual(calls.hover, []);
  }));
