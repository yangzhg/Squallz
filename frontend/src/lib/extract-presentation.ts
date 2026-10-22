import type { ExtractWorkspaceSession } from "./extract-workspace.svelte";
import type { ExtractDestinationMode } from "./extract-options-draft.svelte";
import { dirname, formatBytes } from "./format";
import { tFallback as tr } from "./i18n.svelte";
type Metric = { id: string; label: string; value: string; tone?: "default" | "warning" };

/** Read-only labels shared by workspace skins and the archive browser/status bar. */
export class ExtractPresentation {
  constructor(readonly session: ExtractWorkspaceSession, private readonly archiveFolder: () => string) {}
  extractPlanStatusLabel(): string {
    if (this.session.plan.phase === "ready") return tr("gui.extract.plan_ready");
    if (this.session.plan.phase === "blocked") return tr("gui.error.disk_full.title");
    if (this.session.plan.phase === "error") return tr("gui.extract.plan_unavailable");
    if (this.session.plan.phase === "loading") return tr("gui.extract.plan_checking");
    return tr("gui.extract.plan_waiting");
  }

  extractPlanDescription(): string {
    if (!this.session.archive) return this.session.destination;
    if (this.session.plan.phase === "blocked") return this.session.spaceFailure;
    return this.session.plan.phase === "ready"
      ? tr(
          "gui.extract.plan_ready_body",
          "The selected scope, smart layout, and destination below come from the same core plan the task will rebuild before extraction.",
        )
      : tr(
          "gui.extract.plan_checking_body",
          "Reading the archive layout and checking the current destination without writing files.",
        );
  }

  extractPlanLayoutLabel(): string {
    if (!this.session.plan.plan) return tr("gui.extract.layout_direct");
    return this.session.plan.plan.layout === "wrap_in_folder"
      ? tr("gui.extract.layout_wrapped")
      : tr("gui.extract.layout_direct");
  }

  extractPlanMetrics() {
    const plan = this.session.plan.plan;
    if (!plan) return [];
    const linkCount = plan.symlinks + plan.hardlinks;
    const metrics: Metric[] = [
      {
        id: "scope",
        label: tr("gui.extract.plan_scope"),
        value: tr("gui.extract.plan_scope_value")
          .replace("{entries}", plan.entries.toLocaleString())
          .replace("{files}", plan.files.toLocaleString())
          .replace("{folders}", plan.directories.toLocaleString())
          .replace("{links}", linkCount.toLocaleString())
          .replace("{other}", plan.other.toLocaleString()),
      },
      {
        id: "size",
        label: tr("gui.extract.plan_size"),
        value: formatBytes(plan.total_bytes),
      },
    ];
    if (plan.entries > 0) {
      metrics.push(
        {
          id: "required-space",
          label: tr("common.required"),
          value: formatBytes(plan.required_free_bytes),
        },
        {
          id: "available-space",
          label: tr("common.available"),
          value: formatBytes(plan.available_bytes),
          tone: plan.space_ok ? "default" as const : "warning" as const,
        },
      );
    }
    metrics.push(
      {
        id: "layout",
        label: tr("gui.extract.plan_layout"),
        value: this.extractPlanLayoutLabel(),
      },
      {
        id: "conflicts",
        label: tr("gui.extract.plan_conflicts"),
        value: plan.estimated_conflicts.toLocaleString(),
        tone: plan.estimated_conflicts > 0 ? "warning" as const : "default" as const,
      },
    );
    return metrics;
  }

  extractDestinationFieldLabel(): string {
    return this.session.state.destinationMode === "smart"
      ? tr("gui.extract.smart_base")
      : tr("gui.extract.final_destination");
  }

  extractDestinationTitle(mode: ExtractDestinationMode): string {
    if (mode === "archive") return tr("gui.extract.archive_folder");
    if (mode === "same") return tr("gui.extract.same_folder");
    if (mode === "choose") return tr("gui.extract.choose");
    return tr("gui.extract.smart_folder");
  }

  extractDestinationDetail(mode: ExtractDestinationMode): string {
    if (!this.session.archive) return this.session.destination;
    if (mode === "same") return dirname(this.session.archive.path);
    if (mode === "choose") return this.session.state.customDest.trim() || tr("gui.extract.pick_another_folder");
    if (mode === "smart") {
      return tr("gui.extract.smart_destination_value")
        .replace("{base}", this.session.smartBase);
    }
    return this.archiveFolder();
  }

  extractSelectionLabel(): string {
    if (!this.session.archive) return this.session.destination;
    const count = this.session.options.selectionSnapshot()?.length ?? 0;
    return this.session.state.scope === "selection"
      ? tr("gui.selection.count_selected").replace("{count}", count.toLocaleString())
      : tr("gui.selection.all_entries");
  }

  extractActionLabel(): string {
    if (!this.session.archive) return tr("gui.extract.start");
    return this.session.options.selectionSnapshot()
      ? tr("gui.action.extract_selected")
      : tr("gui.action.extract_all");
  }

  extractSafeTitle(): string {
    return this.session.options.selectionSnapshot()
      ? tr("gui.extract.safe_title")
      : tr("gui.extract.safe_title_all");
  }

  extractEncodingLabel(): string {
    if (!this.session.archive) return this.session.destination;
    if (this.session.state.presetEncodingLabel) {
      return tr("gui.presets.encoding_override")
        .replace("{encoding}", this.session.state.presetEncodingLabel.toUpperCase());
    }
    if (this.session.archive.encoding_override) return this.session.archive.encoding_override.toUpperCase();
    if (this.session.archive.suggested_encoding) {
      return tr("gui.archive.encoding_suggested").replace("{encoding}", this.session.archive.suggested_encoding.toUpperCase());
    }
    return tr("gui.archive.encoding_utf8_clean");
  }

  extractDestinationHint(): string {
    return tr("gui.extract.destination_hint")
      .replace("{destination}", this.session.effectiveDestination);
  }

  extractStartBlockedReason(): string {
    const archiveReason = this.session.archiveRequiredReason;
    if (archiveReason) return archiveReason;
    if (this.session.choosing) {
      return tr("gui.extract.destination_picker_wait");
    }
    if (this.session.state.scope === "selection" && this.session.state.selection.length === 0) {
      return tr("gui.precondition.select_before_extract");
    }
    if (this.session.plan.phase === "blocked") return this.session.spaceFailure;
    if (this.session.plan.phase === "error") return this.session.planError;
    if (this.session.plan.phase !== "ready") {
      return tr("gui.extract.plan_wait_before_start");
    }
    return "";
  }

}
