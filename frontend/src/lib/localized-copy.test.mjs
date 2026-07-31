import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

test("compact bilingual copy stays within narrow-surface budgets", async () => {
  const [english, chinese] = await Promise.all([
    readFile(path.join(frontendRoot, "../locales/en-US.json"), "utf8").then(JSON.parse),
    readFile(path.join(frontendRoot, "../locales/zh-CN.json"), "utf8").then(JSON.parse),
  ]);

  const budgets = {
    "gui.batch.review_subtitle": [79, 24],
    "gui.convert.single_archive_summary": [31, 14],
    "gui.convert.subtitle": [96, 25],
    "gui.create.content_policy.intro": [43, 16],
    "gui.create.content_policy.keep_all_detail": [82, 33],
    "gui.create.format.7z.recovery": [42, 21],
    "gui.create.format.zip.split": [39, 28],
    "gui.create.name_encryption_zip_visible": [24, 13],
    "gui.create.output.completion.none_detail": [35, 12],
    "gui.create.output.completion.open_detail": [32, 19],
    "gui.create.output.destination.default_directory_missing": [65, 20],
    "gui.create.output.integrity.detail": [92, 33],
    "gui.create.output.intro": [65, 18],
    "gui.create.output.preview_default_missing": [47, 19],
    "gui.create.output.source.keep_detail": [43, 17],
    "gui.create.output.source.trash_disabled_excludes": [76, 29],
    "gui.create.real_preflight_body": [68, 23],
    "gui.create.review_ready_overlap_notice": [75, 31],
    "gui.create.review.overlap_value": [50, 21],
    "gui.create.sfx_current_runtime_note": [81, 30],
    "gui.create.sfx_enable_hint": [50, 21],
    "gui.create.sources.description": [59, 18],
    "gui.duplicates.choose_folder_or_open_archive": [26, 9],
    "gui.duplicates.non_destructive_body": [72, 21],
    "gui.duplicates.smaller_ignored": [25, 8],
    "gui.excludes.duplicate_hint": [34, 11],
    "gui.password.empty_detail": [70, 29],
    "gui.password.no_active_request_body": [82, 27],
    "gui.conflict.no_active_request_body": [64, 25],
    "gui.conflict.real_job_pauses_on_overwrite": [64, 29],
    "gui.recovery.choose_archive_for_capabilities": [44, 22],
    "gui.recovery.par2_protection_body": [74, 28],
    "gui.recovery.recovery_strength_hint": [63, 22],
    "gui.recovery.selected_par2_detail": [74, 24],
    "gui.presets.load_failed": [56, 15],
    "gui.settings.general.boundary_body": [74, 20],
    "gui.settings.section.appearance.detail": [30, 15],
    "gui.settings.section.colors.detail": [28, 15],
    "gui.settings.section.file_associations.detail": [30, 16],
    "gui.settings.section.formats_integration.detail": [31, 12],
    "gui.settings.section.general.detail": [29, 16],
    "gui.settings.section.password_book.detail": [29, 14],
    "gui.settings.section.performance.detail": [27, 14],
    "gui.settings.section.security.detail": [33, 12],
  };

  for (const [key, [englishLimit, chineseLimit]] of Object.entries(budgets)) {
    assert.ok(Array.from(english[key]).length <= englishLimit, `${key} exceeds its English copy budget`);
    assert.ok(Array.from(chinese[key]).length <= chineseLimit, `${key} exceeds its Chinese copy budget`);
  }

  for (const key of [
    "gui.password.no_prompt_pending",
    "gui.password.no_active_request",
    "gui.conflict.no_prompt_pending",
    "gui.conflict.no_active_request",
  ]) {
    assert.doesNotMatch(chinese[key], /没有活动/, `${key} uses mechanical status copy`);
  }

  for (const key of Object.keys(budgets).filter((key) => key.startsWith("gui.settings.section."))) {
    assert.match(chinese[key], /、/, `${key} needs explicit narrow-surface break points`);
  }
});
