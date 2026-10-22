import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { compileTestScript, readSvelteScript, selectFunctions, selectMountCallbacks } from "../../tests/source.mjs";

function harness() {
  const source = readSvelteScript(new URL("../components/SfxPublishDialog.svelte", import.meta.url), "SfxPublishDialog.ts");
  const functions = selectFunctions(source, ["onKeydown", "refreshStatus"]);
  const [mount] = selectMountCallbacks(source, ["refreshStatus"]);
  const ticks = [];
  const document = { activeElement: null };
  const element = () => ({ ownerDocument: document, focus() { document.activeElement = this; } });
  const card = element();
  const identitySelect = element();
  const profileInput = element();
  const closeButton = element();
  let resolveStatus;
  const context = {
    card, identitySelect, profileInput, initialIdentity: "", initialProfile: "", returnFocus: element(),
    statusGeneration: 0, publisherState: "checking", statusError: null, identities: [], identity: "", notaryProfile: "",
    submitting: false, cancels: 0, trapped: [], restored: [],
    onCancel: () => { context.cancels += 1; },
    trapModalFocus: (...args) => context.trapped.push(args),
    restoreModalFocus: (...args) => context.restored.push(args),
    tick: () => new Promise((resolve) => ticks.push(resolve)),
    previewStatus: () => null,
    ipc: { getMacosSfxPublisherStatus: () => new Promise((resolve) => { resolveStatus = resolve; }) },
    isErrorDto: () => false, tr: (key) => key,
  };
  vm.createContext(context);
  const api = vm.runInContext(compileTestScript(functions.map((node) => node.getText(source)).join("\n")
    + `\nconst mount = ${mount.getText(source)};`)
    + "\n({ onKeydown, refreshStatus, mount })", context);
  return {
    api, context, document, card, identitySelect, profileInput, closeButton,
    resolve: async (identities = ["identity-one"]) => { resolveStatus({ available: true, status: "available", identities }); await new Promise(setImmediate); },
    flush: async () => { ticks.splice(0).forEach((resolve) => resolve()); await new Promise(setImmediate); },
  };
}

test("SFX Escape always stops at the dialog and only cancels while idle", () => {
  for (const submitting of [false, true]) {
    const run = harness();
    run.context.submitting = submitting;
    let prevented = false;
    let stopped = false;
    run.api.onKeydown({ key: "Escape", preventDefault: () => { prevented = true; }, stopPropagation: () => { stopped = true; } });
    assert.equal(prevented, true);
    assert.equal(stopped, true);
    assert.equal(run.context.cancels, submitting ? 0 : 1);
    const tab = { key: "Tab" };
    run.api.onKeydown(tab);
    assert.deepEqual(run.context.trapped, [[tab, run.card]]);
  }
});

test("publisher status picks an initial input but preserves a user-selected control", async () => {
  for (const focus of ["initial", "close", "profile"]) {
    const run = harness();
    run.card.focus();
    const pending = run.api.refreshStatus();
    if (focus === "close") run.closeButton.focus();
    if (focus === "profile") run.profileInput.focus();
    await run.resolve(["identity-one", "identity-two"]);
    await run.flush();
    await pending;
    assert.equal(run.document.activeElement, focus === "initial" ? run.identitySelect : focus === "close" ? run.closeButton : run.profileInput);
  }
});

test("publisher response cannot focus a dialog destroyed while awaiting the DOM update", async () => {
  const run = harness();
  const destroy = run.api.mount();
  await run.flush();
  await run.resolve();
  destroy();
  await run.flush();
  assert.equal(run.document.activeElement, run.card);
  assert.deepEqual(run.context.restored, [[run.card, run.context.returnFocus]]);
});

test("mount focus preserves newer input and teardown retains the original panel reference", async () => {
  const run = harness();
  const destroy = run.api.mount();
  run.closeButton.focus();
  await run.flush();
  assert.equal(run.document.activeElement, run.closeButton);
  run.context.card = null;
  destroy();
  assert.deepEqual(run.context.restored, [[run.card, run.context.returnFocus]]);
});
