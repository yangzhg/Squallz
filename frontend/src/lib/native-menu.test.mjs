import assert from "node:assert/strict";
import test, { after, before } from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

let server;
let createNativeMenuPublisher;
before(async () => {
  server = await createTestServer();
  ({ createNativeMenuPublisher } = await server.ssrLoadModule("/src/lib/native-menu.ts"));
});
after(async () => server?.close());

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

test("menu publication serializes updates and skips superseded pending selections", async () => {
  const first = deferred();
  const sent = [];
  const publisher = createNativeMenuPublisher(async (snapshot) => {
    sent.push(snapshot);
    if (sent.length === 1) await first.promise;
  }, () => assert.fail("unexpected menu failure"));
  const empty = { language: "en-US", enabled: [] };
  const selected = { language: "en-US", enabled: ["rename_entry"] };
  const modal = { language: "zh-CN", enabled: ["select_all"] };
  publisher.publish(empty);
  publisher.publish(selected);
  publisher.publish(modal);
  assert.deepEqual(sent, [empty]);
  first.resolve();
  await new Promise(setImmediate);
  assert.deepEqual(sent, [empty, modal]);
  publisher.publish(modal);
  assert.equal(sent.length, 2);
  publisher.dispose();
});

test("failed menu updates remain retryable without changing application state", async () => {
  let sends = 0;
  let failures = 0;
  const publisher = createNativeMenuPublisher(async () => {
    sends += 1;
    if (sends === 1) throw new Error("unavailable");
  }, () => { failures += 1; });
  const snapshot = { language: "en-US", enabled: ["open_archive"] };
  publisher.publish(snapshot);
  await new Promise(setImmediate);
  assert.equal(failures, 1);
  publisher.publish(snapshot);
  await new Promise(setImmediate);
  assert.equal(sends, 2);
  publisher.publish(snapshot, true);
  await new Promise(setImmediate);
  assert.equal(sends, 3);
  publisher.dispose();
});

test("closing a window drops pending menu state and late failure feedback", async () => {
  const first = deferred();
  let sends = 0;
  const publisher = createNativeMenuPublisher(async () => { sends += 1; await first.promise; }, () => assert.fail("feedback after disposal"));
  publisher.publish({ language: "en-US", enabled: ["delete_entries"] });
  publisher.publish({ language: "en-US", enabled: [] });
  publisher.dispose();
  first.reject(new Error("window closed"));
  await new Promise(setImmediate);
  assert.equal(sends, 1);
});
