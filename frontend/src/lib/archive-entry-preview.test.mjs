import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { createTestServer, englishTranslate } from "../../tests/runtime.mjs";
import { compileTestScript, readSvelteScript, selectFunctions } from "../../tests/source.mjs";

const server = await createTestServer();
test.after(() => server.close());
const { render } = await server.ssrLoadModule("svelte/server");
const { default: Preview } = await server.ssrLoadModule("/src/components/ArchiveEntryPreview.svelte");
const { default: Icon } = await server.ssrLoadModule("/src/components/Icon.svelte");
const source = readSvelteScript(new URL("../App.svelte", import.meta.url), "App.ts");

function clean(html) { return html.replace(/<!--[^]*?-->/g, "").replace(/\s+/g, " ").trim(); }
function buttons(html) {
  return [...html.matchAll(/<button([^>]*?)>([\s\S]*?)<\/button>/g)].map((match) => ({
    attributes: match[1], body: match[2], text: clean(match[2].replace(/<[^>]+>/g, "")),
  }));
}
function preview(classic, overrides = {}) {
  const view = {
    policyKind: "text", policyCode: "prepared-text", nested: null, title: "Selected item",
    subtitle: "Prepare the selected item", busy: false, failed: false, entry: null,
    canPreview: true, actionLabel: "Open selected item", actionIcon: "eye", disabledReason: "",
    ...overrides,
  };
  return clean(render(Preview, { props: { view, classic, tr: englishTranslate, actions: {} } }).body);
}
function assertActions(html, classic, expected) {
  const rendered = buttons(html).filter((button) => !button.attributes.includes("preview-panel-close"));
  assert.deepEqual(rendered.map((button) => button.text), expected.map(([label]) => label));
  for (const [index, [, icon]] of expected.entries()) {
    const svg = clean(render(Icon, { props: { name: icon, size: classic ? 13 : 14 } }).body);
    assert.ok(clean(rendered[index].body).includes(svg), `${expected[index][0]} retains its ${icon} icon and size`);
  }
}

for (const classic of [true, false]) {
  const mode = classic ? "classic" : "modern";
  test(`${mode} preview preserves inactive and disabled action labels`, () => {
    const html = preview(classic, { canPreview: false, disabledReason: "Choose one file" });
    assert.match(html, /role="region" aria-label="Entry actions"/);
    assert.match(html, /data-preview-policy="text" data-preview-code="prepared-text"/);
    assert.doesNotMatch(html, /preview-sheet-active|preview-panel-close|style=|gui\./);
    assert.match(html, classic ? /<strong>Entry actions<\/strong>/ : /<span class="block-label">Entry actions<\/span>/);
    assertActions(html, classic, [["Open selected item", "eye"]]);
    const action = buttons(html)[0];
    assert.match(action.attributes, /disabled="" aria-busy="false" title="Choose one file"/);
    assert.ok(action.attributes.includes(`aria-label="Open selected item${classic ? " · " : " — "}Choose one file"`));
    const ready = preview(classic);
    assert.doesNotMatch(buttons(ready)[0].attributes, /disabled=/);
    assert.match(buttons(ready)[0].attributes, /aria-label="Open selected item"/);
  });

  test(`${mode} preview loading exposes live status and close without premature actions`, () => {
    const html = preview(classic, { busy: true, subtitle: "Preparing readme.txt" });
    assert.match(html, /preview-sheet-active/);
    assert.match(html, /role="status" aria-live="polite"/);
    assert.match(html, /Preparing item/);
    assert.match(html, /Preparing readme\.txt/);
    assert.equal(html.includes("preview-loading compact"), classic);
    assert.equal(buttons(html).length, 1);
    assert.match(buttons(html)[0].attributes, /class="preview-panel-close" aria-label="Close item actions" title="Close item actions"/);
  });

  test(`${mode} preview failures and prepared files retain distinct recovery actions`, () => {
    const failed = preview(classic, { failed: true, title: "Could not prepare item", subtitle: "Extract this item to continue" });
    assert.match(failed, /role="status"/);
    assert.match(failed, /Could not prepare item/);
    assertActions(failed, classic, [["Retry", "rotate-cw"], ["Extract instead", "archive"]]);
    const ready = preview(classic, { entry: { outer_path: "archive.zip", entry_path: "readme.txt",
      display_name: "readme.txt", preview_id: "prepared-file", size: 1024, archive_like: false } });
    assert.match(ready, /preview-system-action/);
    assertActions(ready, classic, [["Open", "external-link"], ["Reveal", "folder-open"]]);
    assert.equal(buttons(ready).length, 3);
    assert.doesNotMatch(ready, /prepared-file|archive\.zip/);
  });

  test(`${mode} nested preview retains its list and actions while a nested request is busy`, () => {
    const rows = [
      { display: "folder", entry_type: "dir", size: 0, encrypted: false },
      { display: "readme.txt", entry_type: "file", size: 1024, encrypted: false },
      { display: "linked.txt", entry_type: "symlink", size: 0, encrypted: true },
      { display: "hard.txt", entry_type: "hardlink", size: 0, encrypted: false },
    ];
    const html = preview(classic, { busy: true, nested: { title: "inner.zip · ZIP", subtitle: "4 entries", rows } });
    assert.match(html, /inner\.zip · ZIP/);
    assert.match(html, /4 entries/);
    assert.doesNotMatch(html, /preview-loading/);
    assertActions(html, classic, [["Open", "folder-open"], ["Extract", "archive"]]);
    if (classic) {
      for (const row of rows) assert.ok(html.includes(`<em>${row.display}</em>`));
      assert.doesNotMatch(html, /nested-preview-list|role="img"/);
    } else {
      assert.match(html, /nested-preview-list/);
      assert.match(html, /<span>DIR<\/span>/);
      assert.match(html, /<span>FILE<\/span>/);
      assert.match(html, /1\.0 KB/);
      assert.match(html, /role="img" aria-label="Symbolic link · Encrypted" title="Symbolic link · Encrypted"/);
      assert.match(html, /role="img" aria-label="Hard link" title="Hard link"/);
    }
  });
}

test("shared preview actions preserve close focus, queue routing, and successful-open cleanup", async () => {
  const [declaration] = selectFunctions(source, ["archivePreviewActions"]);
  const calls = [];
  let opened = false;
  const context = {
    openNestedPreviewArchive: () => calls.push(["open-nested"]),
    extractNestedPreviewArchive: () => calls.push(["extract-nested"]),
    clearEntryPreviewState: (restoreFocus) => calls.push(["clear", restoreFocus]),
    retryEntryPreview: () => calls.push(["retry"]),
    extractEntryPreviewFailure: () => calls.push(["extract-failure"]),
    openEntryPreview: async () => opened,
    revealEntryPreview: () => calls.push(["reveal"]),
    runAppAction: (action) => calls.push(["app", action]),
  };
  const actions = vm.runInNewContext(`${compileTestScript(declaration.getText(source))}\narchivePreviewActions()`, context);
  actions.onOpenNestedPreview();
  actions.onExtractNestedPreview();
  actions.onClearPreview(true);
  actions.onClearPreview(false);
  actions.onRetryPreview();
  actions.onExtractPreviewFailure();
  actions.onRevealPreview();
  actions.onPreviewSelection();
  assert.deepEqual(calls, [["open-nested"], ["extract-nested"], ["clear", true], ["clear", false],
    ["retry"], ["extract-failure"], ["reveal"], ["app", "preview_entry"]]);
  actions.onOpenPreview();
  await Promise.resolve();
  assert.equal(calls.length, 8, "failed OS open keeps the prepared item available for retry");
  opened = true;
  actions.onOpenPreview();
  await Promise.resolve();
  assert.deepEqual(calls.at(-1), ["clear", undefined]);
});
