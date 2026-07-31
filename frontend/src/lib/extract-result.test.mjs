import assert from "node:assert/strict";
import test from "node:test";

import {
  extractResultNeedsAttention,
  readExtractResultCounts,
  readExtractResultOutcome,
} from "./extract-result.ts";

test("structured extraction counts remain distinct from legacy best-effort problems", () => {
  const result = {
    dest: "/Users/alex/Downloads/Photos",
    best_effort: true,
    skipped: 1,
    problems: ["broken.jpg: CRC mismatch"],
    counts: {
      destination: "/Users/alex/Downloads/Photos",
      selected_entries: 12,
      created: 6,
      directories: 2,
      replaced: 1,
      renamed: 1,
      skipped: 2,
      failed: 1,
      output_bytes: 12_582_912,
    },
  };

  assert.deepEqual(readExtractResultOutcome(result), {
    structured: true,
    skipped: 2,
    failed: 1,
  });
  assert.equal(readExtractResultCounts(result)?.destination, result.dest);
  assert.equal(extractResultNeedsAttention(result), true);
});

test("legacy extraction results keep their historical skipped fallback", () => {
  assert.deepEqual(readExtractResultOutcome({ skipped: 3 }), {
    structured: false,
    skipped: 3,
    failed: 0,
  });
  assert.equal(readExtractResultCounts({ skipped: 3 }), null);
  assert.equal(extractResultNeedsAttention({ skipped: 3 }), false);
});

test("structured skipped entries need attention even when no entry failed", () => {
  assert.equal(extractResultNeedsAttention({
    counts: {
      selected_entries: 3,
      created: 2,
      directories: 0,
      replaced: 0,
      renamed: 0,
      skipped: 1,
      failed: 0,
      output_bytes: 512,
    },
  }), true);
});
