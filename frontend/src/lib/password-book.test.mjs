import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const info = (id = 1, read_only = false) => ({
  id, name: `archive-${id}.zip`, path: `/archives/${id}.zip`, source: `/archives/${id}.zip`,
  format: "zip", entry_count: 1, read_only, encoding_override: null,
  volumes: null, non_utf8_name_count: 0, garbled_count: 0, suggested_encoding: null,
});
const saved = { session: true, available: true, saved: true, error: null };
const empty = { session: false, available: true, saved: false, error: null };
const wrong = { key: "error.wrong_password", params: {}, detail: "" };
const cancelled = { key: "error.cancelled", params: {}, detail: "" };

async function withBook(run) {
  const server = await createTestServer();
  try {
    const book = await server.ssrLoadModule("/src/lib/archive.svelte.ts");
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const requests = [];
    const cancellations = [];
    ipc.archivePasswordStatus = async () => empty;
    ipc.cancelPasswordSave = async (id) => { cancellations.push(id); };
    ipc.rememberArchivePassword = (...args) => {
      const result = deferred();
      requests.push({ args, ...result });
      return result.promise;
    };
    book.installArchivePreview(info(), []);
    await run({ book, ipc, requests, cancellations, save: (password = "correct") => book.rememberArchivePassword(info().path, password, "utf-8") });
  } finally {
    await server.close();
  }
}

test("Password Book verifies once, reports failures, and preserves saved credentials during replacement", () => withBook(async ({ book, requests, save }) => {
  const first = save("wrong");
  assert.equal(book.archivePasswordSave().phase, "verifying");
  assert.equal(book.archivePasswordBookStatus().saved, false);
  assert.equal(await save(), false, "duplicate submission is rejected");
  assert.equal(await book.forgetCurrentArchivePassword(), false, "forget cannot race save");
  assert.equal(requests.length, 1);
  assert.deepEqual(requests[0].args.slice(0, 3), [info().path, "wrong", "utf-8"]);
  assert.equal(typeof requests[0].args[3], "string");
  assert.equal(JSON.stringify(book.archivePasswordSave()).includes("wrong"), false);
  requests[0].reject(wrong);
  assert.equal(await first, false);
  assert.equal(book.archivePasswordSave().error.key, wrong.key);
  const retry = save();
  requests[1].resolve(saved);
  assert.equal(await retry, true);
  assert.equal(book.archivePasswordSave().phase, "saved");
  assert.equal(book.archivePasswordBookStatus().saved, true);
  assert.equal(book.archivePasswordBookStatus().session, true);
  const replacement = save("wrong");
  requests[2].reject(wrong);
  assert.equal(await replacement, false);
  assert.equal(book.archivePasswordBookStatus().saved, true);
}));

test("cancellation waits for the save result and honors an already committed write", () => withBook(async ({ book, requests, cancellations, save }) => {
  const first = save();
  await book.cancelArchivePasswordSave();
  assert.deepEqual(cancellations, [requests[0].args[3]]);
  assert.equal(book.archivePasswordSave().phase, "cancelling");
  requests[0].reject(cancelled);
  assert.equal(await first, false);
  assert.equal(book.archivePasswordSave().phase, "cancelled");
  assert.equal(book.archivePasswordBookStatus().saved, false);
  const retry = save();
  await book.cancelArchivePasswordSave();
  requests[1].resolve(saved);
  assert.equal(await retry, true);
  assert.equal(book.archivePasswordSave().phase, "saved");
}));

test("failed cancellation can be retried without overriding a completed save", () => withBook(async ({ book, ipc, requests, save }) => {
  const first = save();
  ipc.cancelPasswordSave = async () => { throw new Error("disconnected"); };
  await book.cancelArchivePasswordSave();
  assert.equal(book.archivePasswordSave().phase, "verifying");
  assert.equal(book.archivePasswordSave().cancelFailed, true);
  const cancelResult = deferred();
  ipc.cancelPasswordSave = () => cancelResult.promise;
  const retryCancel = book.cancelArchivePasswordSave();
  requests[0].resolve(saved);
  assert.equal(await first, true);
  cancelResult.reject(new Error("late cancellation failure"));
  await retryCancel;
  assert.equal(book.archivePasswordSave().phase, "saved");
  assert.equal(book.archivePasswordSave().cancelFailed, undefined);
}));

test("leaving or switching archives clears pending feedback and suppresses stale results", () => withBook(async ({ book, requests, cancellations, save }) => {
  const first = save();
  book.clearArchivePasswordSave();
  assert.equal(book.archivePasswordSave().phase, "idle");
  assert.deepEqual(cancellations, [requests[0].args[3]]);
  requests[0].resolve(saved);
  assert.equal(await first, false, "no completion feedback on an abandoned form");
  assert.equal(book.archivePasswordSave().phase, "idle");
  assert.equal(book.archivePasswordBookStatus().saved, true, "same archive status follows an already committed write");
  const second = save();
  book.installArchivePreview(info(2), []);
  requests[1].resolve(saved);
  await second;
  assert.equal(book.archivePasswordBookStatus().saved, false);
  assert.equal(book.archivePasswordSave().phase, "idle");
  const third = book.rememberArchivePassword(info(2).path, "correct");
  book.installArchivePreview(info(3), []);
  requests[2].reject(wrong);
  await third;
  assert.equal(book.archivePasswordSave().phase, "idle");
}));

test("only a ready, writable archive can save; refresh and forget cannot race verification", () => withBook(async ({ book, ipc, requests, save }) => {
  book.installArchivePreview(info(1, true), []);
  assert.equal(await save(), false);
  book.installArchivePreview(info(), []);
  ipc.archivePasswordStatus = async () => ({ session: false, available: false, saved: null, error: null });
  await book.refreshArchivePasswordBookStatus();
  assert.equal(await save(), false);
  book.installArchivePreview(info(), []);
  assert.equal(await save(""), false);
  const refresh = deferred();
  ipc.archivePasswordStatus = () => refresh.promise;
  const checking = book.refreshArchivePasswordBookStatus();
  assert.equal(await save(), false);
  refresh.resolve(empty);
  await checking;
  const first = save();
  ipc.archivePasswordStatus = () => { assert.fail("refresh must wait for verification"); };
  await book.refreshArchivePasswordBookStatus();
  requests[0].resolve(saved);
  await first;
}));

test("status refresh observes newly cached and invalidated passwords, ignoring older responses", () => withBook(async ({ book, ipc }) => {
  ipc.archivePasswordStatus = async () => ({ ...empty, session: true });
  await book.refreshArchivePasswordBookStatus();
  assert.equal(book.archivePasswordBookStatus().session, true);
  const previous = deferred();
  ipc.archivePasswordStatus = () => previous.promise;
  const checking = book.refreshArchivePasswordBookStatus();
  ipc.archivePasswordStatus = async () => empty;
  await book.refreshArchivePasswordBookStatus();
  previous.resolve(saved);
  await checking;
  assert.equal(book.archivePasswordBookStatus().session, false);
  assert.equal(book.archivePasswordBookStatus().saved, false);
  const oldArchive = deferred();
  ipc.archivePasswordStatus = () => oldArchive.promise;
  const oldCheck = book.refreshArchivePasswordBookStatus();
  book.installArchivePreview(info(2), []);
  oldArchive.resolve(saved);
  await oldCheck;
  assert.equal(book.archivePasswordBookStatus().session, false);
}));

test("system-store failure preserves confirmed session status, while transport failure makes it unknown", () => withBook(async ({ book, ipc }) => {
  const failure = { key: "error.secret_store", params: {}, detail: "locked" };
  ipc.archivePasswordStatus = async () => ({ session: true, available: true, saved: null, error: failure });
  await assert.rejects(book.refreshArchivePasswordBookStatus(), (error) => error === failure);
  assert.deepEqual(book.archivePasswordBookStatus(), { state: "error", available: true, saved: null, session: true });
  ipc.archivePasswordStatus = async () => { throw new Error("disconnected"); };
  await assert.rejects(book.refreshArchivePasswordBookStatus(), /disconnected/);
  assert.equal(book.archivePasswordBookStatus().session, null);
  assert.equal(book.archivePasswordBookStatus().saved, null);
}));

test("forget reports partial system-store failure and permits session clearing without a store", () => withBook(async ({ book, ipc }) => {
  for (const available of [false, true]) {
    const error = available ? { key: "error.secret_store", params: {}, detail: "locked" } : null;
    ipc.archivePasswordStatus = async () => ({ session: true, available, saved: null, error });
    await book.refreshArchivePasswordBookStatus().catch(() => {});
    ipc.forgetArchivePassword = async () => ({ session: false, available, saved: null, error });
    assert.equal(await book.forgetCurrentArchivePassword(), false, "persistent removal is unconfirmed");
    assert.equal(book.archivePasswordBookStatus().session, false);
    assert.equal(book.archivePasswordBookStatus().saved, null);
  }
  ipc.archivePasswordStatus = async () => ({ ...saved, session: false });
  await book.refreshArchivePasswordBookStatus();
  ipc.forgetArchivePassword = async () => empty;
  assert.equal(await book.forgetCurrentArchivePassword(), true);
  assert.equal(book.archivePasswordBookStatus().saved, false);
  ipc.forgetArchivePassword = async () => { throw new Error("disconnected"); };
  assert.equal(await book.forgetCurrentArchivePassword(), false);
  assert.equal(book.archivePasswordBookStatus().session, null);
}));
