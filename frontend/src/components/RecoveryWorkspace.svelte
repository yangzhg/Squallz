<script lang="ts">
  import Icon from "./Icon.svelte";
  import RecoveryTargetPicker from "./RecoveryTargetPicker.svelte";
  import type {
    RecoveryWorkspaceActions,
    RecoveryWorkspaceTranslate,
    RecoveryWorkspaceVariant,
    RecoveryWorkspaceView,
  } from "../lib/recovery-workspace";

  let {
    variant,
    view,
    actions,
    tr,
  }: {
    variant: RecoveryWorkspaceVariant;
    view: RecoveryWorkspaceView;
    actions: RecoveryWorkspaceActions;
    tr: RecoveryWorkspaceTranslate;
  } = $props();

  type WorkspaceAction = Readonly<{
    id: string;
    label: string;
    icon: string;
    disabledReason: string;
    recommended: boolean;
    run: () => void;
  }>;

  const recoveryRedundancyPresets = [5, 10, 20, 30] as const;

  let primaryActions = $derived<WorkspaceAction[]>([
    {
      id: "protect",
      label: tr("gui.recovery.protect_archive", "Protect archive"),
      icon: "shield-alert",
      disabledReason: view.protectDisabledReason,
      recommended: false,
      run: actions.protect,
    },
    {
      id: "verify",
      label: tr("gui.recovery.verify_with_par2", "Verify recovery capacity"),
      icon: "check-circle",
      disabledReason: view.verifyDisabledReason,
      recommended: view.verifyRecommended,
      run: actions.verify,
    },
    {
      id: "repair",
      label: tr("gui.recovery.repair_with_par2", "Repair with PAR2"),
      icon: "rotate-cw",
      disabledReason: view.repairDisabledReason,
      recommended: view.repairRecommended,
      run: actions.repair,
    },
    ...(view.beyondCapacity
      ? [{
          id: "extract-readable",
          label: tr("gui.recovery.extract_readable_files", "Extract readable files"),
          icon: "folder-open",
          disabledReason: view.bestEffortDisabledReason,
          recommended: true,
          run: actions.extractReadable,
        }]
      : []),
  ]);

  let formatActions = $derived<WorkspaceAction[]>([
    {
      id: "repair-zip",
      label: tr("gui.recovery.repair_zip_index", "Repair ZIP index"),
      icon: "rotate-cw",
      disabledReason: view.zipDisabledReason,
      recommended: false,
      run: actions.repairZip,
    },
    {
      id: "repair-sqz",
      label: tr("gui.recovery.repair_sqz", "Repair SQZ"),
      icon: "shield-alert",
      disabledReason: view.sqzRepairDisabledReason,
      recommended: false,
      run: actions.repairSqz,
    },
    {
      id: "export-sqz",
      label: tr("gui.recovery.export_sqz", "Export SQZ"),
      icon: "external-link",
      disabledReason: view.sqzExportDisabledReason,
      recommended: false,
      run: actions.exportSqz,
    },
    ...(!view.beyondCapacity
      ? [{
          id: "extract-readable",
          label: tr("gui.recovery.extract_readable_files", "Extract readable files"),
          icon: "folder-open",
          disabledReason: view.bestEffortDisabledReason,
          recommended: false,
          run: actions.extractReadable,
        }]
      : []),
  ]);

  function labelWithReason(action: WorkspaceAction): string {
    return action.disabledReason ? `${action.label} · ${action.disabledReason}` : action.label;
  }

  function redundancyPresetLabel(percent: number): string {
    return tr("gui.recovery.use_redundancy_percent", "Use {percent}% redundancy")
      .replace("{percent}", percent.toLocaleString());
  }

  function protectionScopeLabel(): string {
    return view.protectedSourceCount > 1
      ? tr(
          "gui.recovery.protection_scope_volumes",
          "All {count} volume files · one recovery set",
        ).replace("{count}", view.protectedSourceCount.toLocaleString())
      : tr("gui.recovery.protection_scope_single", "This archive file");
  }
</script>

<section
  class="recovery-workspace recovery-view"
  class:modern-recovery={variant === "modern"}
  class:classic-recovery-sheet={variant === "classic"}
  data-recovery-variant={variant}
>
  <header class="recovery-workspace-head" class:sheet-head={variant === "modern"}>
    <div class="recovery-workspace-heading">
      <span class="eyebrow">{tr("gui.recovery.title", "Recovery")}</span>
      <h1>{tr("gui.recovery.protect_repair_title", "Protect and repair archives")}</h1>
      <p>{tr("gui.recovery.protect_repair_body", "PAR2 protects standard archives. ZIP index rebuild handles missing central directories when payloads are intact.")}</p>
      <div class="recovery-safety-strip" aria-label={tr("gui.recovery.safety_summary", "Recovery action safety summary")}>
        <span><Icon name="archive" size={14} />{tr("gui.recovery.source_unchanged", "Source files stay unchanged")}</span>
        <span><Icon name="shield-alert" size={14} />{tr("gui.recovery.verify_capacity_first", "Verify capacity before repair")}</span>
        <span><Icon name="check-circle" size={14} />{tr("gui.recovery.requires_existing_data", "Repair requires PAR2 or SQZ data")}</span>
      </div>
    </div>
    <div
      class="recovery-workspace-primary-actions"
      class:sheet-action-row={variant === "modern"}
      class:classic-button-row={variant === "classic"}
      aria-label={tr("gui.recovery.primary_actions", "Recovery actions")}
    >
      {#each primaryActions as action (action.id)}
        <button
          type="button"
          class:sheet-action={variant === "modern"}
          class:primary-lite={action.recommended && variant === "modern"}
          class:secondary-lite={!action.recommended && variant === "modern"}
          class:classic-primary={action.recommended && variant === "classic"}
          disabled={Boolean(action.disabledReason)}
          title={action.disabledReason || undefined}
          aria-label={labelWithReason(action)}
          data-recovery-action={action.id}
          onclick={action.run}
        ><Icon name={action.icon} size={16} />{action.label}</button>
      {/each}
    </div>
  </header>

  <RecoveryTargetPicker
    {variant}
    archiveName={view.archiveName}
    par2Name={view.par2Name}
    currentArchiveAvailable={view.currentArchiveAvailable}
    usesCurrentArchive={view.usesCurrentArchive}
    usesDefaultPar2={view.usesDefaultPar2}
    chooseArchiveDisabled={view.pickerBusy}
    chooseArchiveTitle={view.pickerBusyReason}
    choosePar2Disabled={view.pickerBusy}
    choosePar2Title={view.pickerBusyReason}
    useCurrentArchiveDisabled={view.pickerBusy || view.usesCurrentArchive}
    useCurrentArchiveTitle={view.pickerBusyReason}
    useDefaultPar2Disabled={view.pickerBusy || view.usesDefaultPar2}
    useDefaultPar2Title={view.pickerBusyReason}
    testArchiveDisabled={view.pickerBusy || Boolean(view.testDisabledReason)}
    testArchiveTitle={view.pickerBusyReason || view.testDisabledReason}
    {tr}
    onChooseArchive={actions.chooseArchive}
    onChoosePar2={actions.choosePar2}
    onUseCurrentArchive={actions.useCurrentArchive}
    onUseDefaultPar2={actions.useDefaultPar2}
    onTestArchive={actions.testArchive}
  />

  <div class="recovery-layout recovery-workspace-layout">
    <section class="recovery-main recovery-workspace-main">
      <div class="panel-title"><Icon name="archive" size={16} />{tr("gui.recovery.par2_protection", "PAR2 protection")}</div>
      <div class="recovery-protection-summary">
        <div>
          <span class="block-label">{tr("gui.recovery.current_workflow", "Current workflow")}</span>
          <strong>{tr("gui.recovery.par2_sidecar", "PAR2 sidecar")}</strong>
          <p>{tr("gui.recovery.par2_protection_body", "Protect writes new PAR2 recovery data beside the archive. Choose how much additional recovery data to create; Verify reports measured repair capacity.")}</p>
          <div class="recovery-strength-control">
            <div class="recovery-strength-heading">
              <span>{tr("gui.recovery.recovery_strength", "Recovery strength")}</span>
              <small id={`${variant}-recovery-strength-hint`}>
                {tr("gui.recovery.recovery_strength_hint", "More redundancy survives more damage, but uses more disk space.")}
              </small>
            </div>
            <div class="recovery-strength-options">
              <div
                class="recovery-strength-presets"
                role="group"
                aria-label={tr("gui.recovery.redundancy_presets", "Recovery strength presets")}
              >
                {#each recoveryRedundancyPresets as percent}
                  <button
                    type="button"
                    aria-pressed={view.redundancyDraft.trim() === String(percent)}
                    aria-label={redundancyPresetLabel(percent)}
                    data-recovery-redundancy-preset={percent}
                    onclick={() => actions.setRedundancy(String(percent))}
                  >{percent}%</button>
                {/each}
              </div>
              <label class="recovery-strength-custom">
                <span>{tr("gui.recovery.custom_redundancy", "Custom")}</span>
                <span class="recovery-strength-input">
                  <input
                    type="number"
                    min="1"
                    max="100"
                    step="1"
                    inputmode="numeric"
                    autocomplete="off"
                    value={view.redundancyDraft}
                    aria-invalid={view.redundancyError ? "true" : undefined}
                    aria-describedby={view.redundancyError
                      ? `${variant}-recovery-strength-error`
                      : `${variant}-recovery-strength-hint`}
                    oninput={(event) => actions.setRedundancy(event.currentTarget.value)}
                  />
                  <b aria-hidden="true">%</b>
                </span>
              </label>
            </div>
            {#if view.redundancyError}
              <p
                id={`${variant}-recovery-strength-error`}
                class="recovery-strength-error"
                role="alert"
              >{view.redundancyError}</p>
            {/if}
          </div>
        </div>
        <dl class="recovery-fact-list">
          <div><dt>{tr("common.target", "Target")}</dt><dd>{view.sourceName}</dd></div>
          <div><dt>{tr("gui.recovery.requested_redundancy", "Requested redundancy")}</dt><dd>{view.requestedRedundancy}</dd></div>
          <div><dt>{tr("gui.recovery.protection_scope", "Protection scope")}</dt><dd>{protectionScopeLabel()}</dd></div>
          <div><dt>{tr("gui.recovery.repair_capacity", "Repair capacity")}</dt><dd>{view.repairCapacity}</dd></div>
          <div><dt>{tr("gui.recovery.repair_output", "Repair output")}</dt><dd>{view.repairOutputMode}</dd></div>
          <div><dt>{tr("gui.recovery.planned_index", "Planned PAR2 index")}</dt><dd>{view.plannedIndex}</dd></div>
        </dl>
      </div>

      <section class={`verify-card tone-${view.resultTone}`} aria-labelledby={`${variant}-recovery-result-title`}>
        <div class="recovery-result-status" role="status" aria-live="polite" aria-atomic="true">
          <div class="verify-score">
            <span id={`${variant}-recovery-result-title`}>{tr("gui.recovery.verify_result", "Verify result")}</span>
            <strong>{view.resultTitle}</strong>
          </div>
          {#if view.metrics}
            <div class="block-math">
              <div><b>{view.metrics.blocksNeeded}</b><span>{tr("gui.recovery.blocks_needed", "damaged or missing blocks")}</span></div>
              <div><b>{view.metrics.recoveryBlocksAvailable}</b><span>{tr("gui.recovery.recovery_blocks_available", "recovery blocks available")}</span></div>
              <div><b>{view.metrics.remainingMargin}</b><span>{tr("gui.recovery.remaining_margin", "remaining margin")}</span></div>
            </div>
            <p>{view.resultExplanation}</p>
          {:else}
            <p>{view.resultDetail}</p>
          {/if}
          {#if view.resultAvailable}
            <p class="recovery-result-footer">{view.resultFooter}</p>
          {/if}
        </div>
      </section>
    </section>

    <aside class="recovery-side recovery-workspace-side">
      <section class="sqz-recovery-card">
        <span class="block-label">{tr("gui.recovery.format_workflow", "Format workflow")}</span>
        <strong>{view.formatWorkflowTitle}</strong>
        <p>{view.formatWorkflowBody}</p>
        <div class="recovery-format-actions" aria-label={tr("gui.recovery.format_specific_tools", "Format-specific tools")}>
          {#each formatActions as action (action.id)}
            <button
              type="button"
              disabled={Boolean(action.disabledReason)}
              title={action.disabledReason || undefined}
              aria-label={labelWithReason(action)}
              data-recovery-action={action.id}
              onclick={action.run}
            ><Icon name={action.icon} size={15} />{action.label}</button>
          {/each}
        </div>
      </section>
    </aside>
  </div>
</section>
