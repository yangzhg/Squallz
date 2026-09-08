import assert from "node:assert/strict";
import test from "node:test";

import { createTestServer } from "../../tests/runtime.mjs";

test("late system-open responses cannot revive a dismissed or replaced preview", async () => {
  const server = await createTestServer();

  try {
    const { previewResponseIsCurrent } = await server.ssrLoadModule(
      "/src/lib/preview-response.ts",
    );
    const expected = {
      previewGeneration: 7,
      actionGeneration: 3,
      previewId: "preview-a",
      archiveSource: "/archives/a.zip",
    };

    assert.equal(previewResponseIsCurrent(expected, expected), true);
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, previewGeneration: 8 }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, actionGeneration: 4 }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, previewId: "preview-b" }),
      false,
    );
    assert.equal(
      previewResponseIsCurrent(expected, { ...expected, archiveSource: "/archives/b.zip" }),
      false,
    );
  } finally {
    await server.close();
  }
});
