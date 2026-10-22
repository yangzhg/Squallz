import type { CreateDraftIssue, CreateOptionsDraft } from "./create-options-draft.svelte";
import { maxArchivePresetExcludeRuleBytes, maxArchivePresetExcludeRules } from "./archive-presets.svelte";
import { tFallback as tr } from "./i18n.svelte";

export function createPresetSaveIssue(options: CreateOptionsDraft, issueMessage: (issue: CreateDraftIssue) => string): string {
  if (options.state.postSuccess === "trash_source" && options.trashExcluded) return issueMessage("trash_excluded");
  if (options.splitIssue) return issueMessage(options.splitIssue);
  const rules = options.state.contentPolicy === "custom" ? options.excludeRules : [];
  if (rules.length > maxArchivePresetExcludeRules) {
    return tr("gui.presets.exclude_limit")
      .replace("{count}", String(maxArchivePresetExcludeRules));
  }
  const oversized = rules.find((rule) => new TextEncoder().encode(rule).length > maxArchivePresetExcludeRuleBytes);
  return oversized
    ? tr("gui.presets.exclude_rule_too_long")
      .replace("{rule}", oversized.slice(0, 24)).replace("{count}", String(maxArchivePresetExcludeRuleBytes))
    : "";
}
