import assert from "node:assert/strict";
import test from "node:test";

import { createTestServer } from "../../tests/runtime.mjs";

test("archive inspector presents metadata and integrity-test availability", async () => {
  const server = await createTestServer();

  try {
    const { render } = await server.ssrLoadModule("svelte/server");
    const { default: ModernInspector } = await server.ssrLoadModule(
      "/src/components/ModernInspector.svelte",
    );

    for (const archive of [
      null,
      { format: "ZIP", entries: 16384, encoding: "UTF-8 clean", volumes: "Single" },
    ]) {
      const { body } = render(ModernInspector, {
        props: {
          tr: (_key, fallback) => fallback,
          view: {
            kind: "archive",
            archive,
            preview: {
              policyKind: "none",
              policyCode: "",
              nested: null,
              title: "Open or preview",
              subtitle: "Select one entry",
              busy: false,
              failed: false,
              entry: null,
              canPreview: false,
              actionLabel: "Open or preview",
              actionIcon: "eye",
              disabledReason: "Select one entry",
            },
            canRename: false,
            canMove: false,
            openArchiveFirst: "Open an archive first",
            archiveActionDisabledReason: archive ? "" : "Open an archive first",
            selectionSummary: "No entries selected",
            copyOutDisabledReason: "Select entries first",
          },
        },
      });
      const testButton = body.match(/<button(?=[^>]*aria-label="Test archive)[^>]*>/)?.[0];
      assert.ok(testButton);
      assert.equal(testButton.includes("disabled"), archive === null);
      assert.doesNotMatch(body, /<progress\b/);

      if (archive) {
        const text = body.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
        assert.ok(text.includes(`Format ${archive.format}`));
        assert.ok(text.includes(`Entries ${archive.entries.toLocaleString()}`));
        assert.ok(text.includes(`Encoding ${archive.encoding}`));
        assert.match(text, /Recovery status not checked/);
      }
    }
  } finally {
    await server.close();
  }
});
