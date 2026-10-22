import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

test("initial copy comes from the catalog while raw error lookup retains its missing-key signal", async () => {
  const server = await createTestServer();
  try {
    const { t, tError, tFallback, i18nReady } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    assert.equal(i18nReady(), false);
    assert.equal(tFallback("gui.preview.loading"), "Preparing item");
    assert.equal(tFallback("gui.preview.loading", "Local wording"), "Local wording");
    assert.equal(t("gui.preview.loading"), "gui.preview.loading");
    assert.equal(tError({ key: "error.cancelled", params: {}, detail: "" }), "error.cancelled");
    assert.equal(tFallback("missing.catalog.key"), "missing.catalog.key");
    const { jobTitleFor } = await server.ssrLoadModule("/src/lib/job-title.ts");
    assert.equal(jobTitleFor({ kind: "convert", src: "/in/{to}$&.zip", dest: "/out/result.7z" }),
      "Convert {to}$&.zip -> result.7z");
  } finally {
    await server.close();
  }
});

test("identity translations fall back, empty translations remain valid, and interpolation is single-pass", async () => {
  const server = await createTestServer();
  try {
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const { loadLocale, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    ipc.getLocaleTable = async () => ({ lang: "en-US", table: {
      "gui.preview.loading": "gui.preview.loading", identity: "{value}", empty: "",
    } });
    await loadLocale("en-US");
    assert.equal(tFallback("gui.preview.loading"), "Preparing item");
    assert.equal(tFallback("gui.preview.loading", "Local wording"), "Local wording");
    assert.equal(tFallback("identity", "Fallback", { value: "identity" }), "Fallback");
    assert.equal(tFallback("empty", "Fallback"), "");
    assert.equal(tFallback("gui.task.job.convert", { from: "{to}$&", to: "done" }), "Convert {to}$& → done");
  } finally {
    await server.close();
  }
});

test("a late locale response cannot replace the newest language or its fallback", async () => {
  const server = await createTestServer();
  try {
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const { loadLocale, tFallback, currentLang, listBundledLanguages } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    let resolveOld;
    ipc.getLocaleTable = (lang) => lang === "old"
      ? new Promise((resolve) => { resolveOld = resolve; })
      : Promise.reject(new Error("No desktop bridge"));
    const old = loadLocale("old");
    await loadLocale("zh-cn");
    const chinese = tFallback("gui.preview.loading");
    assert.equal(currentLang(), "zh-CN");
    assert.notEqual(chinese, "Preparing item");
    resolveOld({ lang: "old", table: { "gui.preview.loading": "Stale" } });
    await old;
    assert.equal(currentLang(), "zh-CN");
    assert.equal(tFallback("gui.preview.loading"), chinese);
    await loadLocale("unknown-language");
    assert.equal(currentLang(), "en-US");
    assert.equal(tFallback("gui.preview.loading"), "Preparing item");
    const tags = (await listBundledLanguages()).map(({ tag }) => tag);
    assert.equal(tags.filter((tag) => tag === "en-US").length, 1);
    assert.ok(tags.includes("zh-CN"));
  } finally {
    await server.close();
  }
});

test("translated and fallback messages preserve literal placeholder names in parameter values", async () => {
  const server = await createTestServer();
  try {
    const { ipc } = await server.ssrLoadModule("/src/lib/ipc.ts");
    const { loadLocale, t, tError, tFallback } = await server.ssrLoadModule("/src/lib/i18n.svelte.ts");
    const template = "The folder {folder} is no longer available. Showing {parent}.";
    ipc.getLocaleTable = async () => ({
      lang: "en-US",
      table: { folder: template, count: "{count}/{count} {unknown}" },
    });
    await loadLocale("en-US");
    const params = { folder: "docs/{parent}/$&", parent: "docs/{folder}" };
    const expected = "The folder docs/{parent}/$& is no longer available. Showing docs/{folder}.";
    assert.equal(t("folder", params), expected);
    assert.equal(tError({ key: "folder", params, detail: "" }), expected);
    assert.equal(tFallback("missing", template, params), expected);
    assert.equal(t("count", { count: 0 }), "0/0 {unknown}");
    assert.equal(t("folder"), template);
  } finally {
    await server.close();
  }
});
