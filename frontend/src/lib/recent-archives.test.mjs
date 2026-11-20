import assert from "node:assert/strict";
import test from "node:test";
import { withTestServer } from "../../tests/runtime.mjs";

test("recent archives tolerate unavailable storage and retain literal paths", t => withTestServer(async server => {
  const { recentFiles, rememberRecent } = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
  const { toasts, removeToastByKey } = await server.ssrLoadModule("/src/lib/toasts.svelte.ts");
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  let stored = "{}";
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: () => stored,
    setItem: (_key, value) => { stored = value; },
  } });
  t.after(() => {
    removeToastByKey("recent-save");
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else delete globalThis.localStorage;
  });
  assert.deepEqual(recentFiles(), []);
  stored = JSON.stringify([null, "/A ", "/A ", "", "/B", "/C", "/D", "/E", "/F"]);
  assert.deepEqual(recentFiles(), ["/A ", "/B", "/C", "/D", "/E"]);
  rememberRecent("/D");
  assert.deepEqual(JSON.parse(stored), ["/D", "/A ", "/B", "/C", "/E"]);
  Object.defineProperty(globalThis, "localStorage", { get() { throw new Error("Storage unavailable"); } });
  rememberRecent("/G");
  assert.equal(toasts().at(-1)?.title, "Archive opened. Could not save it to recent files.");
}));

test("the current archive appears once and returning to it is distinct from reopening a file", () => withTestServer(async server => {
  const { render } = await server.ssrLoadModule("svelte/server");
  const { default: Recent } = await server.ssrLoadModule("/src/components/RecentWorkspace.svelte");
  const { body } = render(Recent, { props: {
    current: { path: "C:/Work/Saved.zip", name: "Saved.zip" },
    paths: ["c:\\work\\saved.zip", "C:/Backups/Saved.zip"],
    platform: "windows", canOpen: true, opening: false,
    onOpen() {}, onReopen() {}, onReturn() {},
  } });
  assert.equal((body.match(/class="recent-file-copy"/gu) ?? []).length, 2);
  assert.match(body, /Back to current archive/u);
  assert.match(body, /C:\/Backups\/Saved\.zip/u);
  assert.doesNotMatch(body, /c:\\work/u);
}));
