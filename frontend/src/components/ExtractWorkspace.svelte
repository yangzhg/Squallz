<script lang="ts" module>
  import type { ComponentProps } from "svelte";

  type ArchivePresetPickerComponent = typeof import("./ArchivePresetPicker.svelte").default;
  type ExtractPlanSummaryComponent = typeof import("./ExtractPlanSummary.svelte").default;

  export type ExtractWorkspaceVariant = "modern" | "classic";

  export interface ExtractWorkspaceChoice {
    id: string;
    label: string;
    detail: string;
    selected: boolean;
    disabled: boolean;
    title: string;
    ariaLabel: string;
    onSelect: () => void;
  }

  export interface ExtractWorkspaceSurface {
    tr: (key: string, fallback: string) => string;
    title: string;
    start: {
      label: string;
      disabled: boolean;
      title: string;
      ariaLabel: string;
      onSelect: () => void;
    };
    batch: {
      label: string;
      disabled: boolean;
      title: string;
      ariaLabel: string;
      onSelect: () => void;
    };
    destination: {
      label: string;
      path: string;
      choices: ExtractWorkspaceChoice[];
    };
    archive: {
      title: string;
      line: string;
      selection: string;
      password: string;
    };
    plan: ComponentProps<ExtractPlanSummaryComponent>;
    preset: ComponentProps<ArchivePresetPickerComponent>;
    overwrite: {
      label: string;
      choices: ExtractWorkspaceChoice[];
    };
    symlink: {
      label: string;
      choices: ExtractWorkspaceChoice[];
    };
    encoding: {
      label: string;
      detail: string;
    };
    test: {
      disabled: boolean;
      title: string;
      ariaLabel: string;
      onSelect: () => void;
    };
  }
</script>

<script lang="ts">
  import ArchivePresetPicker from "./ArchivePresetPicker.svelte";
  import ExtractPlanSummary from "./ExtractPlanSummary.svelte";
  import Icon from "./Icon.svelte";

  let {
    variant,
    surface,
  }: {
    variant: ExtractWorkspaceVariant;
    surface: ExtractWorkspaceSurface;
  } = $props();
</script>

{#if variant === "modern"}
  <div class="extract-view modern-extract">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{surface.tr("gui.extract.eyebrow", "Extract")}</span>
        <h1>{surface.title}</h1>
        <p>{surface.tr("gui.extract.safe_subtitle", "Destination preview, smart folder behavior, conflicts, passwords, and safety limits are visible before the job starts.")}</p>
      </div>
      <button
        class="primary sheet-action"
        disabled={surface.start.disabled}
        title={surface.start.title}
        aria-label={surface.start.ariaLabel}
        onclick={surface.start.onSelect}
      ><Icon name="archive" size={17} />{surface.start.label}</button>
    </div>

    <div class="extract-layout">
      <section class="extract-main-panel">
        <div class="extract-essentials-grid">
          <div class="extract-destination-setup">
            <div class="path-decision">
              <span class="block-label">{surface.destination.label}</span>
              <strong>{surface.destination.path}</strong>
              <p>{surface.tr("gui.extract.snapshot_hint", "Smart extract captures the current safety settings when the job starts.")}</p>
            </div>
            <div class="destination-grid">
              {#each surface.destination.choices as choice (choice.id)}
                <button
                  class:selected={choice.selected}
                  aria-pressed={choice.selected}
                  disabled={choice.disabled}
                  title={choice.title}
                  aria-label={choice.ariaLabel}
                  onclick={choice.onSelect}
                >
                  <strong>{choice.label}</strong>
                  <span>{choice.detail}</span>
                </button>
              {/each}
            </div>
            <div class="extract-essentials-facts">
              <div class="extract-essentials-fact">
                <span>{surface.tr("gui.inspector.archive", "Archive")}</span>
                <strong>{surface.archive.title}</strong>
                <p>{surface.archive.line} · {surface.archive.selection}</p>
              </div>
              <div class="extract-essentials-fact">
                <span>{surface.tr("gui.extract.password", "Password")}</span>
                <strong>{surface.archive.password}</strong>
                <p>{surface.tr("gui.extract.snapshot_hint", "Smart extract captures the current safety settings when the job starts.")}</p>
              </div>
            </div>
          </div>
          <ExtractPlanSummary {...surface.plan} />
        </div>

        <ArchivePresetPicker {...surface.preset} />

        <section class="extract-policy-panel" aria-label={surface.tr("gui.extract.safety", "Safety")}>
          <div class="extract-policy-section">
            <div class="extract-policy-heading">
              <span class="block-label">{surface.tr("gui.extract.conflict_policy", "Conflict policy")}</span>
              <strong>{surface.overwrite.label}</strong>
            </div>
            <div class="extract-policy-grid" aria-label={surface.tr("gui.extract.conflict_policy", "Conflict policy")}>
              {#each surface.overwrite.choices as choice (choice.id)}
                <button
                  class:selected={choice.selected}
                  aria-pressed={choice.selected}
                  onclick={choice.onSelect}
                >{choice.label}</button>
              {/each}
            </div>
          </div>
          <div class="extract-policy-section">
            <div class="extract-policy-heading">
              <span class="block-label">{surface.tr("gui.extract.symlink_policy", "Symbolic link policy")}</span>
              <strong>{surface.symlink.label}</strong>
            </div>
            <div
              class="extract-symlink-grid"
              role="group"
              aria-label={surface.tr("gui.extract.symlink_policy", "Symbolic link policy")}
            >
              {#each surface.symlink.choices as choice (choice.id)}
                <button
                  class:selected={choice.selected}
                  aria-pressed={choice.selected}
                  onclick={choice.onSelect}
                >{choice.label}</button>
              {/each}
            </div>
          </div>
        </section>

        <div class="extract-context-strip">
          <div class="extract-context-item">
            <span>{surface.tr("gui.archive.encoding", "Encoding")}</span>
            <strong>{surface.encoding.label}</strong>
            <p>{surface.encoding.detail}</p>
          </div>
          <div class="extract-context-item">
            <span>{surface.tr("gui.extract.blocked_conditions", "Blocked conditions")}</span>
            <strong>{surface.tr("gui.extract.safety_guards_on", "Zip Slip + bomb guards on")}</strong>
            <p>{surface.tr("gui.extract.blocked_conditions_body", "Path traversal, case collision, reserved Windows names, and symlink escapes stop the job before writing.")}</p>
          </div>
        </div>

        <div class="extract-flow-actions">
          <button
            disabled={surface.batch.disabled}
            title={surface.batch.title}
            aria-label={surface.batch.ariaLabel}
            onclick={surface.batch.onSelect}
          ><Icon name="list" size={16} />{surface.batch.label}</button>
        </div>
      </section>
    </div>
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-extract-sheet classic-extract">
      <header>
        <div>
          <h1>{surface.title}</h1>
          <p>{surface.tr("gui.extract.classic_subtitle", "Choose the final folder, preview smart extract behavior, and review conflicts before writing files.")}</p>
        </div>
        <div class="classic-button-row">
          <button onclick={surface.batch.onSelect}>{surface.batch.label}</button>
          <button
            class="classic-primary"
            disabled={surface.start.disabled}
            title={surface.start.title}
            aria-label={surface.start.ariaLabel}
            onclick={surface.start.onSelect}
          >{surface.start.label}</button>
        </div>
      </header>

      <div class="classic-extract-grid">
        <section class="classic-extract-form">
          <h2>{surface.tr("gui.batch.destination", "Destination")}</h2>
          <div class="classic-form-grid compact">
            <div class="classic-label">{surface.tr("gui.batch.destination", "Destination")}</div>
            <div class="classic-input accent classic-extract-destination-path">{surface.destination.path}</div>
            <div class="classic-label">{surface.tr("common.mode", "Mode")}</div>
            <div class="classic-segments">
              {#each surface.destination.choices as choice (choice.id)}
                <button
                  class:active={choice.selected}
                  aria-pressed={choice.selected}
                  disabled={choice.disabled}
                  title={choice.title}
                  aria-label={choice.ariaLabel}
                  onclick={choice.onSelect}
                >{choice.label}</button>
              {/each}
            </div>
            <div class="classic-label">{surface.tr("gui.inspector.archive", "Archive")}</div>
            <div class="classic-input">{surface.archive.line}</div>
            <div class="classic-label">{surface.tr("common.selection", "Selection")}</div>
            <div class="classic-input accent">{surface.archive.selection}</div>
            <div class="classic-label">{surface.tr("gui.extract.password", "Password")}</div>
            <div class="classic-input">{surface.archive.password}</div>
            <div class="classic-label">{surface.tr("gui.extract.conflicts", "Conflicts")}</div>
            <div class="classic-segments">
              {#each surface.overwrite.choices as choice (choice.id)}
                <button
                  class:active={choice.selected}
                  aria-pressed={choice.selected}
                  onclick={choice.onSelect}
                >{choice.label}</button>
              {/each}
            </div>
            <div class="classic-label">{surface.tr("gui.extract.symlink_policy", "Symbolic link policy")}</div>
            <div
              class="classic-segments"
              role="group"
              aria-label={surface.tr("gui.extract.symlink_policy", "Symbolic link policy")}
            >
              {#each surface.symlink.choices as choice (choice.id)}
                <button
                  class:active={choice.selected}
                  aria-pressed={choice.selected}
                  onclick={choice.onSelect}
                >{choice.label}</button>
              {/each}
            </div>
            <div class="classic-label">{surface.tr("gui.archive.encoding", "Encoding")}</div>
            <div class="classic-input">{surface.encoding.label}</div>
            <div class="classic-label">{surface.tr("gui.extract.safety", "Safety")}</div>
            <div class="classic-input accent classic-extract-safety">{surface.tr("gui.extract.safety_blocked", "Zip Slip, bomb ratio, reserved names, symlink escape blocked")}</div>
          </div>
          <div class="classic-extract-actions">
            <button
              disabled={surface.test.disabled}
              title={surface.test.title}
              aria-label={surface.test.ariaLabel}
              onclick={surface.test.onSelect}
            ><Icon name="check-circle" size={15} />{surface.tr("gui.extract.test_first", "Test first")}</button>
          </div>
        </section>

        <aside class="classic-extract-preview">
          <ExtractPlanSummary {...surface.plan} />
        </aside>
      </div>

      <div class="classic-preset-wrap">
        <ArchivePresetPicker {...surface.preset} />
      </div>
    </section>
  </div>
{/if}
