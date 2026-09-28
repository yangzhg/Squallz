import assert from "node:assert/strict";
import test from "node:test";
import { createTestServer } from "../../tests/runtime.mjs";

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
