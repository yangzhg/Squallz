import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createServer } from "vite";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

const noop = () => {};
const actions = {
  chooseArchive: noop,
  choosePar2: noop,
  useCurrentArchive: noop,
  useDefaultPar2: noop,
  testArchive: noop,
  setRedundancy: noop,
  protect: noop,
  verify: noop,
  repair: noop,
  repairZip: noop,
  repairSqz: noop,
  exportSqz: noop,
  extractReadable: noop,
};

function workspaceView(beyondCapacity, overrides = {}) {
  return {
    archiveName: "/Users/example/Archive.zip",
    par2Name: "/Users/example/Archive.zip.par2",
    currentArchiveAvailable: true,
    usesCurrentArchive: true,
    usesDefaultPar2: true,
    pickerBusy: false,
    pickerBusyReason: "",
    testDisabledReason: "",
    sourceName: "Archive.zip",
    requestedRedundancy: "10% requested redundancy",
    redundancyDraft: "10",
    redundancyError: "",
    protectedSourceCount: 4,
    repairCapacity: "3 blocks needed · 8 recovery blocks available",
    repairOutputMode: "New folder with all 4 protected files",
    plannedIndex: "Archive.zip.par2",
    resultTone: beyondCapacity ? "danger" : "warning",
    resultTitle: beyondCapacity ? "Not repairable" : "Repairable",
    resultDetail: "Recovery capacity checked",
    resultExplanation: "Recovery data was measured from the selected PAR2 set.",
    resultFooter: "Verification completed",
    resultAvailable: true,
    metrics: {
      blocksNeeded: beyondCapacity ? "12" : "3",
      recoveryBlocksAvailable: beyondCapacity ? "4" : "8",
      remainingMargin: beyondCapacity ? "0" : "5",
    },
    beyondCapacity,
    formatWorkflowTitle: "PAR2 sidecar workflow",
    formatWorkflowBody: "Use PAR2 for damage recovery.",
    protectDisabledReason: "",
    verifyDisabledReason: "",
    repairDisabledReason: "Insufficient blocks",
    zipDisabledReason: "",
    sqzRepairDisabledReason: "",
    sqzExportDisabledReason: "",
    bestEffortDisabledReason: "",
    verifyRecommended: true,
    repairRecommended: false,
    ...overrides,
  };
}

function actionIds(html) {
  return [...html.matchAll(/data-recovery-action="([^"]+)"/g)].map((match) => match[1]);
}

function actionTag(html, action) {
  return html.match(new RegExp(`<button(?=[^>]*data-recovery-action="${action}")[^>]*>`))?.[0] ?? "";
}

function redundancyPresetTag(html, percent) {
  return html.match(
    new RegExp(`<button(?=[^>]*data-recovery-redundancy-preset="${percent}")[^>]*>`),
  )?.[0] ?? "";
}

test("modern and classic recovery workspaces expose the same semantic actions", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: RecoveryWorkspace } = await server.ssrLoadModule(
      "/src/components/RecoveryWorkspace.svelte",
    );
    const tr = (_key, fallback) => fallback;

    for (const beyondCapacity of [false, true]) {
      const rendered = new Map();
      for (const variant of ["modern", "classic"]) {
        const { body } = render(RecoveryWorkspace, {
          props: {
            variant,
            view: workspaceView(beyondCapacity),
            actions,
            tr,
          },
        });
        rendered.set(variant, body);
      }

      const expected = beyondCapacity
        ? ["protect", "verify", "repair", "extract-readable", "repair-zip", "repair-sqz", "export-sqz"]
        : ["protect", "verify", "repair", "repair-zip", "repair-sqz", "export-sqz", "extract-readable"];
      const modernActions = actionIds(rendered.get("modern"));
      const classicActions = actionIds(rendered.get("classic"));

      assert.deepEqual(modernActions, expected);
      assert.deepEqual(classicActions, expected);
      assert.deepEqual(modernActions, classicActions);
      assert.equal(modernActions.filter((action) => action === "extract-readable").length, 1);

      for (const html of rendered.values()) {
        const repair = actionTag(html, "repair");
        assert.match(repair, /disabled=""/);
        assert.match(repair, /title="Insufficient blocks"/);
        assert.match(repair, /aria-label="Repair with PAR2 · Insufficient blocks"/);
        assert.match(html, /role="status" aria-live="polite" aria-atomic="true"/);
        assert.equal(
          [...html.matchAll(/data-recovery-redundancy-preset="(\d+)"/g)]
            .map((match) => Number(match[1]))
            .join(","),
          "5,10,20,30",
        );
        assert.match(redundancyPresetTag(html, 10), /aria-pressed="true"/);
        assert.match(redundancyPresetTag(html, 20), /aria-pressed="false"/);
        assert.match(html, /type="number" min="1" max="100" step="1"/);
        assert.match(html, /value="10"/);
        assert.match(html, /All 4 volume files · one recovery set/);
        assert.match(html, /Repair output/);
        assert.match(html, /New folder with all 4 protected files/);
      }

      assert.match(actionTag(rendered.get("modern"), "verify"), /class="[^"]*primary-lite[^"]*"/);
      assert.match(actionTag(rendered.get("classic"), "verify"), /class="[^"]*classic-primary[^"]*"/);
    }
  } finally {
    await server.close();
  }
});

test("invalid recovery strength is explained and blocks protection", async () => {
  const server = await createServer({
    appType: "custom",
    logLevel: "silent",
    root: frontendRoot,
    server: { hmr: false, middlewareMode: true },
  });

  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: RecoveryWorkspace } = await server.ssrLoadModule(
      "/src/components/RecoveryWorkspace.svelte",
    );
    const error = "Enter a whole percentage from 1% to 100%.";
    const { body } = render(RecoveryWorkspace, {
      props: {
        variant: "modern",
        view: workspaceView(false, {
          redundancyDraft: "0",
          redundancyError: error,
          requestedRedundancy: error,
          protectDisabledReason: error,
        }),
        actions,
        tr: (_key, fallback) => fallback,
      },
    });

    assert.match(actionTag(body, "protect"), /disabled=""/);
    assert.match(actionTag(body, "protect"), /title="Enter a whole percentage from 1% to 100%\."/);
    assert.match(body, /type="number"[^>]*aria-invalid="true"/);
    assert.match(body, /class="recovery-strength-error" role="alert"/);
    assert.match(body, /Enter a whole percentage from 1% to 100%\./);
  } finally {
    await server.close();
  }
});
