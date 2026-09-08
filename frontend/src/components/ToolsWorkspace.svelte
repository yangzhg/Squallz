<script lang="ts" module>
  import type { ChecksumAlgorithmId } from "../lib/ui-model";

  export type ToolsWorkspaceKind = "batch" | "checksum" | "duplicates";
  export type ToolsWorkspaceVariant = "modern" | "classic";
  export type ChecksumResultKind = "checksum" | "checksum_check";

  type Tr = (key: string, fallback: string) => string;

  interface ArchiveReturnSurface {
    visible: boolean;
    title: string;
    detail: string;
    contextLabel: string;
    actionLabel: string;
    onReturn: () => void;
  }

  interface ChecksumResultRow {
    name: string;
    size: string;
    digest: string;
    status: string;
  }

  interface ChecksumVerificationRow {
    name: string;
    expected: string;
    actual: string;
    status: string;
  }

  export interface ChecksumWorkspaceSurface {
    kind: "checksum";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    target: {
      name: string;
      label: string;
      currentArchiveDisabledReason: string;
    };
    algorithm: {
      options: ChecksumAlgorithmId[];
      selected: ChecksumAlgorithmId;
      label: string;
      labelFor: (algorithm: ChecksumAlgorithmId) => string;
      hintFor: (algorithm: ChecksumAlgorithmId) => string;
      onSelect: (algorithm: ChecksumAlgorithmId) => void;
    };
    metrics: {
      filesHashed: string;
      bytesHashed: string;
      passed: string;
      checked: string;
      failed: string;
    };
    excludes: {
      value: string;
      rules: string[];
      countLabel: string;
      onInput: (value: string) => void;
    };
    manifestLabel: string;
    result: {
      rows: ChecksumResultRow[];
      state: string;
      feedback: string | null;
      feedbackDanger: boolean;
      onCopy: () => void;
    };
    verification: {
      rows: ChecksumVerificationRow[];
      state: string;
      feedback: string | null;
      feedbackDanger: boolean;
      onCopy: () => void;
    };
    actions: {
      onChooseFile: () => void;
      onChooseFolder: () => void;
      onUseCurrentArchive: () => void;
      onCalculate: () => void;
      onChooseManifest: () => void;
      onVerifyManifest: () => void;
      onOpenDuplicates: () => void;
      onOpenRecovery: () => void;
    };
    onPanelMount: (kind: ChecksumResultKind, node: HTMLElement | null) => void;
  }

  export interface DuplicatesWorkspaceSurface {
    kind: "duplicates";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    target: {
      name: string;
      label: string;
    };
    minimumSize: {
      value: number;
      label: string;
      error: string;
      onInput: (event: Event) => void;
    };
    excludes: {
      value: string;
      rules: string[];
      countLabel: string;
      onInput: (value: string) => void;
    };
    metrics: {
      filesScanned: string;
      bytesScanned: string;
      candidateFiles: string;
      hashedBytes: string;
      duplicateFiles: string;
      duplicateGroups: string;
      reclaimable: string;
      taskState: string;
      reviewState: string;
    };
    actions: {
      onChooseFolder: () => void;
      onUseArchiveFolder: () => void;
      onScan: () => void;
      onOpenCreate: () => void;
      onOpenBatch: () => void;
    };
  }

  export interface BatchWorkspaceSurface {
    kind: "batch";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    rows: Array<{
      name: string;
      format: string;
      entries: string;
      target: string;
      state: string;
      warning: boolean;
    }>;
    warningLabel: string;
    emptyLabel: string;
    actions: {
      onStart: () => void;
      onBack: () => void;
      onResolvePassword: () => void;
    };
  }

  export type ToolsWorkspaceSurface =
    | BatchWorkspaceSurface
    | ChecksumWorkspaceSurface
    | DuplicatesWorkspaceSurface;
</script>

<script lang="ts">
  import ArchiveReturnStrip from "./ArchiveReturnStrip.svelte";
  import ChecksumAlgorithmPicker from "./ChecksumAlgorithmPicker.svelte";
  import ExcludeRulesEditor from "./ExcludeRulesEditor.svelte";
  import Icon from "./Icon.svelte";

  let {
    surface,
  }: {
    surface: ToolsWorkspaceSurface;
  } = $props();

  function registerChecksumPanel(node: HTMLElement, kind: ChecksumResultKind) {
    if (surface.kind !== "checksum") return;
    surface.onPanelMount(kind, node);
    return {
      destroy() {
        if (surface.kind === "checksum") {
          surface.onPanelMount(kind, null);
        }
      },
    };
  }
</script>

{#if surface.kind === "checksum"}
  {#if surface.variant === "modern"}
    <div class="settings-view modern-checksum">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.checksum.eyebrow", "Tools / Checksum")}</span>
          <h1>{surface.tr("gui.checksum.title", "Verify files without changing them")}</h1>
          <p>{surface.tr("gui.checksum.modern_subtitle", "Compute SHA-256, SHA-512, SHA-1, MD5, BLAKE3, or CRC32 with the shared core engine, or verify a checksum manifest.")}</p>
        </div>
        <button class="primary sheet-action" onclick={surface.actions.onCalculate}><Icon name="check-circle" size={17} />{surface.tr("gui.checksum.calculate", "Calculate checksum")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="settings-metric-grid">
            <div><span>{surface.tr("common.target", "Target")}</span><strong>{surface.target.name}</strong><small>{surface.target.label}</small></div>
            <div><span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span><strong>{surface.algorithm.label}</strong><small>{surface.tr("gui.checksum.matches_cli_algorithm", "Matches sqz checksum --algorithm")}</small></div>
            <div><span>{surface.tr("gui.checksum.latest_hashed", "Latest hashed")}</span><strong>{surface.metrics.filesHashed}</strong><small>{surface.metrics.bytesHashed}</small></div>
            <div><span>{surface.tr("gui.checksum.manifest_check", "Manifest check")}</span><strong>{surface.metrics.passed} / {surface.metrics.checked}</strong><small>{surface.tr("gui.checksum.failed_count", "{count} failed").replace("{count}", surface.metrics.failed)}</small></div>
          </div>

          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.checksum.target", "Checksum target")}</strong><span>{surface.target.label}</span></div>
            <div class="path-preview">{surface.target.label}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={surface.actions.onChooseFile}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_file", "Choose file")}</button>
              <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
              <button
                disabled={Boolean(surface.target.currentArchiveDisabledReason)}
                title={surface.target.currentArchiveDisabledReason}
                aria-label={surface.target.currentArchiveDisabledReason
                  ? `${surface.tr("gui.checksum.use_current_archive", "Use current archive")}: ${surface.target.currentArchiveDisabledReason}`
                  : surface.tr("gui.checksum.use_current_archive", "Use current archive")}
                onclick={surface.actions.onUseCurrentArchive}
              >{surface.tr("gui.checksum.use_current_archive", "Use current archive")}</button>
              <button class="primary-lite" onclick={surface.actions.onCalculate}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.calculate_now", "Calculate now")}</button>
            </div>
            <div class="algorithm-field worker-field" role="group" aria-label={surface.tr("gui.checksum.algorithm", "Algorithm")}>
              <span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span>
              <ChecksumAlgorithmPicker
                algorithms={surface.algorithm.options}
                selected={surface.algorithm.selected}
                labelFor={surface.algorithm.labelFor}
                hintFor={surface.algorithm.hintFor}
                onSelect={surface.algorithm.onSelect}
              />
            </div>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title", "Excludes")}
            hint={surface.tr("gui.excludes.folder_scan_hint", "Applied only when the target is a folder.")}
            countLabel={surface.excludes.countLabel}
            value={surface.excludes.value}
            placeholder={surface.tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
            ariaLabel={surface.tr("gui.checksum.exclude_rules", "Checksum exclude rules")}
            rules={surface.excludes.rules}
            emptyLabel={surface.tr("gui.create.no_rules", "No rules")}
            onInput={surface.excludes.onInput}
          />

          <div class="level-control settings-slider checksum-manifest-card">
            <div><strong>{surface.tr("gui.checksum.manifest_verification", "Manifest verification")}</strong><span>{surface.manifestLabel}</span></div>
            <div class="path-preview">{surface.manifestLabel}</div>
            <div class="settings-actions-row checksum-manifest-actions">
              <button class="primary-lite" onclick={surface.actions.onChooseManifest}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
              <button class="primary-lite" onclick={surface.actions.onVerifyManifest}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
            </div>
          </div>

          <section
            class="checksum-result-panel"
            use:registerChecksumPanel={"checksum"}
            tabindex="-1"
            aria-label={surface.tr("gui.checksum.result", "Checksum result")}
          >
            <div class="checksum-result-actions">
              <div class="checksum-result-title">
                <strong>{surface.tr("gui.checksum.result", "Checksum result")}</strong>
                <span>{surface.tr("gui.checksum.result_rows", "{count} rows").replace("{count}", surface.result.rows.length.toLocaleString())}</span>
              </div>
              <div class="checksum-result-copy">
                {#if surface.result.feedback}
                  <span class="checksum-copy-status" class:danger={surface.result.feedbackDanger} role="status">{surface.result.feedback}</span>
                {/if}
                <button type="button" class="primary-lite" disabled={surface.result.rows.length === 0} onclick={surface.result.onCopy}><Icon name="list" size={14} />{surface.tr("gui.checksum.copy_results", "Copy results")}</button>
              </div>
            </div>
            <div class="limits-table checksum-result-table">
              <div><b>{surface.tr("gui.checksum.result", "Checksum result")}</b><b>{surface.tr("gui.table.size", "Size")}</b><b>{surface.tr("gui.checksum.digest", "Digest")}</b><b>{surface.tr("common.status", "Status")}</b></div>
              {#each surface.result.rows as row}
                <div><span>{row.name}</span><span>{row.size}</span><code class="checksum-digest">{row.digest}</code><strong>{row.status}</strong></div>
              {:else}
                <div><span>{surface.tr("gui.checksum.no_result_yet", "No checksum result yet")}</span><span>-</span><span>-</span><strong>{surface.result.state}</strong></div>
              {/each}
            </div>
          </section>

          <section
            class="checksum-result-panel"
            use:registerChecksumPanel={"checksum_check"}
            tabindex="-1"
            aria-label={surface.tr("gui.checksum.verification_result", "Verification result")}
          >
            <div class="checksum-result-actions">
              <div class="checksum-result-title">
                <strong>{surface.tr("gui.checksum.verification_result", "Verification result")}</strong>
                <span>{surface.tr("gui.checksum.result_rows", "{count} rows").replace("{count}", surface.verification.rows.length.toLocaleString())}</span>
              </div>
              <div class="checksum-result-copy">
                {#if surface.verification.feedback}
                  <span class="checksum-copy-status" class:danger={surface.verification.feedbackDanger} role="status">{surface.verification.feedback}</span>
                {/if}
                <button type="button" class="primary-lite" disabled={surface.verification.rows.length === 0} onclick={surface.verification.onCopy}><Icon name="list" size={14} />{surface.tr("gui.checksum.copy_results", "Copy results")}</button>
              </div>
            </div>
            <div class="limits-table checksum-result-table checksum-verify-table">
              <div><b>{surface.tr("gui.checksum.verification_result", "Verification result")}</b><b>{surface.tr("gui.checksum.expected", "Expected")}</b><b>{surface.tr("gui.checksum.actual", "Actual")}</b><b>{surface.tr("common.status", "Status")}</b></div>
              {#each surface.verification.rows as row}
                <div><span>{row.name}</span><code class="checksum-digest">{row.expected}</code><code class="checksum-digest">{row.actual}</code><strong>{row.status}</strong></div>
              {:else}
                <div><span>{surface.tr("gui.checksum.no_manifest_result_yet", "No manifest result yet")}</span><span>-</span><span>-</span><strong>{surface.verification.state}</strong></div>
              {/each}
            </div>
          </section>
        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="check-circle" size={16} />{surface.tr("gui.checksum.verification_contract", "Verification scope")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.checksum.shared_with_cli", "Shared with CLI")}</strong>
            <span>{surface.tr("gui.checksum.cli_contract_body", "This page maps to sqz checksum and sqz checksum --check; JSON result fields stay aligned with automation output.")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={surface.actions.onOpenDuplicates}>
              <Icon name="search" size={16} />
              <span><strong>{surface.tr("gui.screen.duplicates", "Duplicate Finder")}</strong><small>{surface.tr("gui.duplicates.route_from_checksum", "Find identical local files with BLAKE3")}</small></span>
            </button>
            <button class="settings-route-card" onclick={surface.actions.onOpenRecovery}>
              <Icon name="shield-alert" size={16} />
              <span><strong>{surface.tr("gui.recovery.title", "Recovery")}</strong><small>{surface.tr("gui.recovery.route_from_checksum", "Test, protect, repair, and export archives")}</small></span>
            </button>
          </div>
        </aside>
      </div>
    </div>
  {:else}
    <div class="classic-dialog-body" class:with-archive-return={surface.archiveReturn.visible}>
      {#if surface.archiveReturn.visible}
        <ArchiveReturnStrip
          title={surface.archiveReturn.title}
          detail={surface.archiveReturn.detail}
          contextLabel={surface.archiveReturn.contextLabel}
          actionLabel={surface.archiveReturn.actionLabel}
          buttonClass="classic-primary"
          iconSize={15}
          onReturn={surface.archiveReturn.onReturn}
        />
      {/if}
      <section class="classic-extract-sheet classic-checksum">
        <header>
          <div>
            <h1>{surface.title}</h1>
            <p>{surface.tr("gui.checksum.subtitle", "Calculate local file digests or verify a manifest with the same engine exposed by sqz checksum.")}</p>
          </div>
          <div class="classic-button-row">
            <button onclick={surface.actions.onChooseFile}>{surface.tr("gui.checksum.choose_file", "Choose file")}</button>
            <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
            <button class="classic-primary" onclick={surface.actions.onCalculate}>{surface.tr("gui.checksum.calculate", "Calculate checksum")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.checksum.calculate_title", "Calculate")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target", "Target")}</div><div class="classic-input accent">{surface.target.label}</div>
              <div class="classic-label">{surface.tr("gui.checksum.algorithm", "Algorithm")}</div>
              <ChecksumAlgorithmPicker
                algorithms={surface.algorithm.options}
                selected={surface.algorithm.selected}
                labelFor={surface.algorithm.labelFor}
                hintFor={surface.algorithm.hintFor}
                onSelect={surface.algorithm.onSelect}
                className="classic-algorithm-grid"
              />
              <div class="classic-label">{surface.tr("gui.create.excludes", "Excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.excludes.value}
                aria-label={surface.tr("gui.checksum.exclude_rules", "Checksum exclude rules")}
                oninput={(event) => surface.excludes.onInput(event.currentTarget.value)}
              ></textarea>
            </div>
            <button
              class="classic-color-route"
              disabled={Boolean(surface.target.currentArchiveDisabledReason)}
              title={surface.target.currentArchiveDisabledReason}
              aria-label={surface.target.currentArchiveDisabledReason
                ? `${surface.tr("gui.checksum.use_current_archive", "Use current archive")}: ${surface.target.currentArchiveDisabledReason}`
                : surface.tr("gui.checksum.use_current_archive", "Use current archive")}
              onclick={surface.actions.onUseCurrentArchive}
            ><Icon name="archive" size={15} />{surface.tr("gui.checksum.use_current_archive", "Use current archive")}</button>
          </section>
          <aside>
            <h2>{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</h2>
            <div class="classic-form-grid compact no-pad">
              <div class="classic-label">{surface.tr("gui.checksum.manifest", "Manifest")}</div><div class="classic-input">{surface.manifestLabel}</div>
              <div class="classic-label">{surface.tr("gui.checksum.passed", "Passed")}</div><div class="classic-input success">{surface.metrics.passed}</div>
              <div class="classic-label">{surface.tr("gui.checksum.failed", "Failed")}</div><div class="classic-input danger">{surface.metrics.failed}</div>
              <div class="classic-label">{surface.tr("gui.checksum.checked", "Checked")}</div><div class="classic-input">{surface.metrics.checked}</div>
            </div>
            <div class="classic-button-row checksum-manifest-actions">
              <button onclick={surface.actions.onChooseManifest}>{surface.tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
              <button class="classic-primary" onclick={surface.actions.onVerifyManifest}>{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
            </div>
          </aside>
        </div>

        <section
          class="checksum-result-panel classic-checksum-result-panel"
          use:registerChecksumPanel={"checksum"}
          tabindex="-1"
          aria-label={surface.tr("gui.checksum.result", "Checksum result")}
        >
          <div class="checksum-result-actions">
            <div class="checksum-result-title">
              <strong>{surface.tr("gui.checksum.result", "Checksum result")}</strong>
              <span>{surface.tr("gui.checksum.result_rows", "{count} rows").replace("{count}", surface.result.rows.length.toLocaleString())}</span>
            </div>
            <div class="checksum-result-copy">
              {#if surface.result.feedback}
                <span class="checksum-copy-status" class:danger={surface.result.feedbackDanger} role="status">{surface.result.feedback}</span>
              {/if}
              <button type="button" class="classic-primary" disabled={surface.result.rows.length === 0} onclick={surface.result.onCopy}>{surface.tr("gui.checksum.copy_results", "Copy results")}</button>
            </div>
          </div>
          <div class="classic-form-grid compact checksum-result-summary">
            <div class="classic-label">{surface.tr("gui.checksum.latest_files", "Latest files")}</div><div class="classic-input">{surface.metrics.filesHashed}</div>
            <div class="classic-label">{surface.tr("gui.checksum.latest_bytes", "Latest bytes")}</div><div class="classic-input">{surface.metrics.bytesHashed}</div>
            <div class="classic-label">{surface.tr("gui.checksum.latest_state", "Latest state")}</div><div class="classic-input accent">{surface.result.state}</div>
          </div>
          <div class="classic-checksum-table">
            <div><b>{surface.tr("gui.checksum.result", "Checksum result")}</b><b>{surface.tr("gui.checksum.digest", "Digest")}</b><b>{surface.tr("common.status", "Status")}</b></div>
            {#each surface.result.rows as row}
              <div><span>{row.name}</span><code class="checksum-digest">{row.digest}</code><strong>{row.status}</strong></div>
            {:else}
              <div><span>{surface.tr("gui.checksum.no_result_yet", "No checksum result yet")}</span><code>-</code><strong>{surface.result.state}</strong></div>
            {/each}
          </div>
        </section>
      </section>
    </div>
  {/if}
{:else if surface.kind === "duplicates"}
  {#if surface.variant === "modern"}
    <div class="settings-view modern-duplicates">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.duplicates.eyebrow", "Tools / Duplicate Finder")}</span>
          <h1>{surface.tr("gui.duplicates.title", "Find duplicate local files")}</h1>
          <p>{surface.tr("gui.duplicates.modern_subtitle", "BLAKE3 hashes are computed by the shared core engine; this scan never deletes, moves, links, or modifies files.")}</p>
          <div class="duplicate-safety-strip" aria-label={surface.tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
            <span><Icon name="search" size={14} />{surface.tr("gui.duplicates.cli_contract", "CLI parity: sqz duplicates")}</span>
            <span><Icon name="list" size={14} />{surface.tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
            <span><Icon name="check-circle" size={14} />{surface.tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
          </div>
        </div>
        <button class="primary sheet-action" onclick={surface.actions.onScan}><Icon name="search" size={17} />{surface.tr("gui.duplicates.scan", "Scan duplicates")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="settings-metric-grid">
            <div><span>{surface.tr("common.target", "Target")}</span><strong>{surface.target.name}</strong><small>{surface.target.label}</small></div>
            <div><span>{surface.tr("gui.duplicates.minimum_size", "Minimum size")}</span><strong>{surface.minimumSize.label}</strong><small>{surface.tr("gui.duplicates.smaller_ignored", "Smaller files are ignored before hashing")}</small></div>
            <div><span>{surface.tr("gui.duplicates.latest_groups", "Latest groups")}</span><strong>{surface.metrics.duplicateGroups}</strong><small>{surface.tr("gui.duplicates.duplicate_files_count", "{count} duplicate files").replace("{count}", surface.metrics.duplicateFiles)}</small></div>
            <div><span>{surface.tr("gui.duplicates.reclaimable", "Reclaimable")}</span><strong>{surface.metrics.reclaimable}</strong><small>{surface.tr("gui.duplicates.potential_space", "Potential space if one copy per group remains")}</small></div>
          </div>

          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.duplicates.scan_target", "Scan target")}</strong><span>{surface.target.label}</span></div>
            <div class="path-preview">{surface.target.label}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={surface.actions.onChooseFolder}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
              <button onclick={surface.actions.onUseArchiveFolder}>{surface.tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
              <button class="primary-lite" onclick={surface.actions.onScan}><Icon name="search" size={15} />{surface.tr("gui.duplicates.scan_now", "Scan now")}</button>
            </div>
            <label class="number-field worker-field">
              <span>{surface.tr("gui.duplicates.minimum_hashed_size_bytes", "Minimum hashed size in bytes")}</span>
              <input
                type="number"
                min="0"
                step="1"
                value={surface.minimumSize.value}
                class:invalid={surface.minimumSize.error.length > 0}
                aria-label={surface.tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
                aria-invalid={surface.minimumSize.error ? "true" : "false"}
                aria-describedby={surface.minimumSize.error ? "duplicate-min-size-error-modern" : undefined}
                oninput={surface.minimumSize.onInput}
              />
              {#if surface.minimumSize.error}
                <small id="duplicate-min-size-error-modern" class="duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.minimumSize.error}</small>
              {/if}
            </label>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title", "Excludes")}
            hint={surface.tr("gui.excludes.duplicate_hint", "Skip noisy folders before duplicate hashing.")}
            countLabel={surface.excludes.countLabel}
            value={surface.excludes.value}
            placeholder={surface.tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
            ariaLabel={surface.tr("gui.duplicates.exclude_rules", "Duplicate scan exclude rules")}
            rules={surface.excludes.rules}
            emptyLabel={surface.tr("gui.create.no_rules", "No rules")}
            onInput={surface.excludes.onInput}
          />

          <div class="limits-table">
            <div><b>{surface.tr("gui.duplicates.result", "Result")}</b><b>{surface.tr("gui.duplicates.count", "Count")}</b><b>{surface.tr("gui.duplicates.bytes", "Bytes")}</b><b>{surface.tr("common.status", "Status")}</b></div>
            <div><span>{surface.tr("gui.duplicates.files_scanned", "Files scanned")}</span><span>{surface.metrics.filesScanned}</span><span>{surface.metrics.bytesScanned}</span><strong>{surface.metrics.taskState}</strong></div>
            <div><span>{surface.tr("gui.duplicates.candidates_hashed", "Candidates hashed")}</span><span>{surface.metrics.candidateFiles}</span><span>{surface.metrics.hashedBytes}</span><strong>BLAKE3</strong></div>
            <div><span>{surface.tr("gui.duplicates.duplicate_groups", "Duplicate groups")}</span><span>{surface.metrics.duplicateGroups}</span><span>{surface.metrics.reclaimable}</span><strong>{surface.metrics.reviewState}</strong></div>
          </div>
        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="search" size={16} />{surface.tr("gui.duplicates.scan_contract", "Safe scan scope")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.duplicates.non_destructive", "Reads and marks duplicates only")}</strong>
            <span>{surface.tr("gui.duplicates.non_destructive_body", "The scan never cleans up, hard-links, deletes, moves, or modifies files automatically.")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={surface.actions.onOpenCreate}>
              <Icon name="sparkles" size={16} />
              <span><strong>{surface.tr("gui.nav.create", "Create")}</strong><small>{surface.tr("gui.duplicates.route_to_create", "Use the same exclude semantics before compression")}</small></span>
            </button>
            <button class="settings-route-card" onclick={surface.actions.onOpenBatch}>
              <Icon name="list" size={16} />
              <span><strong>{surface.tr("gui.screen.batch", "Batch")}</strong><small>{surface.tr("gui.duplicates.route_to_batch", "Start archive work after reviewing targets")}</small></span>
            </button>
          </div>
        </aside>
      </div>
    </div>
  {:else}
    <div class="classic-dialog-body" class:with-archive-return={surface.archiveReturn.visible}>
      {#if surface.archiveReturn.visible}
        <ArchiveReturnStrip
          title={surface.archiveReturn.title}
          detail={surface.archiveReturn.detail}
          contextLabel={surface.archiveReturn.contextLabel}
          actionLabel={surface.archiveReturn.actionLabel}
          buttonClass="classic-primary"
          iconSize={15}
          onReturn={surface.archiveReturn.onReturn}
        />
      {/if}
      <section class="classic-extract-sheet classic-duplicates">
        <header>
          <div>
            <h1>{surface.title}</h1>
            <p>{surface.tr("gui.duplicates.subtitle", "Scan local folders with the same BLAKE3 duplicate detector exposed by sqz duplicates; no cleanup action is run.")}</p>
            <div class="duplicate-safety-strip classic-duplicate-safety" aria-label={surface.tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
              <span><Icon name="search" size={13} />{surface.tr("gui.duplicates.cli_contract", "CLI parity: sqz duplicates")}</span>
              <span><Icon name="list" size={13} />{surface.tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
              <span><Icon name="check-circle" size={13} />{surface.tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
            </div>
          </div>
          <div class="classic-button-row">
            <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
            <button onclick={surface.actions.onUseArchiveFolder}><Icon name="archive" size={15} />{surface.tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
            <button class="classic-primary" onclick={surface.actions.onScan}>{surface.tr("gui.duplicates.scan", "Scan duplicates")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.duplicates.scan_setup", "Scan setup")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target", "Target")}</div><div class="classic-input accent">{surface.target.label}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.min_size", "Min size")}</div>
              <input
                class="classic-input"
                class:invalid={surface.minimumSize.error.length > 0}
                type="number"
                min="0"
                step="1"
                value={surface.minimumSize.value}
                oninput={surface.minimumSize.onInput}
                aria-label={surface.tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
                aria-invalid={surface.minimumSize.error ? "true" : "false"}
                aria-describedby={surface.minimumSize.error ? "duplicate-min-size-error-classic" : undefined}
              />
              {#if surface.minimumSize.error}
                <div></div>
                <small id="duplicate-min-size-error-classic" class="classic-input duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.minimumSize.error}</small>
              {/if}
              <div class="classic-label">{surface.tr("gui.create.excludes", "Excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.excludes.value}
                aria-label={surface.tr("gui.duplicates.exclude_rules", "Duplicate exclude rules")}
                oninput={(event) => surface.excludes.onInput(event.currentTarget.value)}
              ></textarea>
            </div>
          </section>
          <aside>
            <h2>{surface.tr("gui.duplicates.latest_result", "Latest result")}</h2>
            <div class="classic-form-grid compact no-pad">
              <div class="classic-label">{surface.tr("common.status", "State")}</div><div class="classic-input">{surface.metrics.taskState}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.files", "Files")}</div><div class="classic-input">{surface.metrics.filesScanned}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.groups", "Groups")}</div><div class="classic-input accent">{surface.metrics.duplicateGroups}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.reclaimable", "Reclaimable")}</div><div class="classic-input accent">{surface.metrics.reclaimable}</div>
            </div>
          </aside>
        </div>
      </section>
    </div>
  {/if}
{:else}
  {#if surface.variant === "modern"}
    <div class="batch-view modern-batch">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.batch.review", "Batch extract review")}</span>
          <h1>{surface.tr("gui.batch.review_count_title", "Review {count} archives before extraction").replace("{count}", String(surface.rows.length))}</h1>
          <p>{surface.tr("gui.batch.review_subtitle", "Every target folder is previewed before work starts. Password or volume issues block only the affected archive.")}</p>
        </div>
        <button
          class="primary sheet-action"
          disabled={surface.rows.length === 0}
          title={surface.rows.length === 0 ? surface.emptyLabel : ""}
          onclick={surface.actions.onStart}
        ><Icon name="archive" size={17} />{surface.tr("gui.batch.start_batch", "Start batch")}</button>
      </div>
      <div class="batch-summary-strip">
        <div><span>{surface.tr("gui.batch.target_rule", "Target rule")}</span><strong>{surface.tr("gui.batch.each_archive_folder", "Each archive folder")}</strong></div>
        <div><span>{surface.tr("gui.extract.smart_mode", "Smart extract")}</span><strong>{surface.tr("common.on", "On")}</strong></div>
        <div><span>{surface.tr("gui.extract.conflicts", "Conflicts")}</span><strong>{surface.tr("gui.batch.ask_before_replace", "Ask before replace")}</strong></div>
        <div><span>{surface.tr("gui.batch.warnings", "Warnings")}</span><strong>{surface.warningLabel}</strong></div>
      </div>
      <div class="batch-card-list">
        {#each surface.rows as row}
          <section class:warning={row.warning} class="batch-card">
            <div>
              <strong>{row.name}</strong>
              <span>{row.format} · {surface.tr("gui.archive.entry_count", "{count} entries").replace("{count}", row.entries)}</span>
            </div>
            <div><span>{surface.tr("common.target", "Target")}</span><strong>{row.target}</strong></div>
            <em>{row.state}</em>
          </section>
        {:else}
          <section class="batch-card">
            <div>
              <strong>{surface.emptyLabel}</strong>
              <span>{surface.tr("gui.batch.no_archives_queued", "No archives selected")}</span>
            </div>
            <div><span>{surface.tr("common.target", "Target")}</span><strong>-</strong></div>
            <em>{surface.tr("gui.task.idle", "Idle")}</em>
          </section>
        {/each}
      </div>
    </div>
  {:else}
    <div class="classic-dialog-body">
      <section class="classic-extract-sheet classic-batch">
        <header>
          <div>
            <h1>{surface.tr("gui.batch.review", "Batch Extract Review")}</h1>
            <p>{surface.tr("gui.batch.classic_subtitle", "Review every archive, target folder, password state, and blocked item before tasks start.")}</p>
          </div>
          <div class="classic-button-row">
            <button onclick={surface.actions.onBack}>{surface.tr("gui.nav.back", "Back")}</button>
            <button class="classic-primary" disabled={surface.rows.length === 0} onclick={surface.actions.onStart}>{surface.tr("gui.batch.start_batch", "Start batch")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.nav.archives", "Archives")}</h2>
            <div class="classic-batch-table">
              <div><b>{surface.tr("gui.inspector.archive", "Archive")}</b><b>{surface.tr("common.format", "Format")}</b><b>{surface.tr("gui.table.entries", "Entries")}</b><b>{surface.tr("common.target", "Target")}</b><b>{surface.tr("common.status", "Status")}</b></div>
              {#each surface.rows as row}
                <div class:warning={row.warning}>
                  <strong>{row.name}</strong><span>{row.format}</span><span>{row.entries}</span><span>{row.target}</span><em>{row.state}</em>
                </div>
              {:else}
                <div>
                  <strong>{surface.emptyLabel}</strong><span>-</span><span>0</span><span>-</span><em>{surface.tr("gui.batch.no_archives_queued", "No archives selected")}</em>
                </div>
              {/each}
            </div>
          </section>
          <aside>
            <h2>{surface.tr("gui.batch.policy", "Batch policy")}</h2>
            <div class="classic-form-grid compact no-pad">
              <div class="classic-label">{surface.tr("gui.batch.target_rule", "Target rule")}</div><div class="classic-input accent">{surface.tr("gui.batch.each_archive_folder", "Each archive folder")}</div>
              <div class="classic-label">{surface.tr("gui.extract.smart_mode", "Smart extract")}</div><div class="classic-input">{surface.tr("gui.batch.smart_per_archive", "On · per archive root analysis")}</div>
              <div class="classic-label">{surface.tr("gui.extract.conflicts", "Conflicts")}</div><div class="classic-input">{surface.tr("gui.batch.ask_before_replace", "Ask before replace")}</div>
              <div class="classic-label">{surface.tr("gui.batch.failure_mode", "Failure mode")}</div><div class="classic-input accent">{surface.tr("gui.batch.continue_ready_hold_blocked", "Continue ready archives, hold blocked archive")}</div>
            </div>
            <button class="classic-color-route" onclick={surface.actions.onResolvePassword}><Icon name="lock" size={15} />{surface.tr("gui.batch.resolve_missing_password", "Resolve missing password")}</button>
          </aside>
        </div>
      </section>
    </div>
  {/if}
{/if}
