<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    variant,
    archiveName,
    par2Name,
    currentArchiveAvailable,
    usesCurrentArchive,
    usesDefaultPar2,
    chooseArchiveDisabled = false,
    chooseArchiveTitle = "",
    choosePar2Disabled = false,
    choosePar2Title = "",
    useCurrentArchiveDisabled = false,
    useCurrentArchiveTitle = "",
    useDefaultPar2Disabled = false,
    useDefaultPar2Title = "",
    testArchiveDisabled = false,
    testArchiveTitle = "",
    tr,
    onChooseArchive,
    onChoosePar2,
    onUseCurrentArchive,
    onUseDefaultPar2,
    onTestArchive,
  }: {
    variant: "modern" | "classic";
    archiveName: string | null;
    par2Name: string | null;
    currentArchiveAvailable: boolean;
    usesCurrentArchive: boolean;
    usesDefaultPar2: boolean;
    chooseArchiveDisabled?: boolean;
    chooseArchiveTitle?: string;
    choosePar2Disabled?: boolean;
    choosePar2Title?: string;
    useCurrentArchiveDisabled?: boolean;
    useCurrentArchiveTitle?: string;
    useDefaultPar2Disabled?: boolean;
    useDefaultPar2Title?: string;
    testArchiveDisabled?: boolean;
    testArchiveTitle?: string;
    tr: (key: string, fallback: string) => string;
    onChooseArchive: () => void;
    onChoosePar2: () => void;
    onUseCurrentArchive: () => void;
    onUseDefaultPar2: () => void;
    onTestArchive: () => void;
  } = $props();

  function fileNameOnly(value: string | null): string | null {
    if (!value) return null;
    const parts = value.trim().replaceAll("\\", "/").split("/").filter(Boolean);
    const name = parts[parts.length - 1];
    return name && name !== "." && name !== ".." ? name : null;
  }

  function labelWithReason(label: string, disabled: boolean, reason: string): string {
    return disabled && reason ? `${label} · ${reason}` : label;
  }

  let safeArchiveName = $derived(fileNameOnly(archiveName));
  let safePar2Name = $derived(fileNameOnly(par2Name));
  let headingId = $derived(`${variant}-recovery-target-picker-title`);
  let useCurrentDisabled = $derived(!currentArchiveAvailable || useCurrentArchiveDisabled);
  let useCurrentReason = $derived(
    useCurrentArchiveTitle ||
      (!currentArchiveAvailable
        ? tr("gui.recovery.no_current_archive", "No open archive is available")
        : usesCurrentArchive && useCurrentArchiveDisabled
          ? tr("gui.recovery.already_using_current_archive", "Already using the current archive")
          : ""),
  );
  let useDefaultDisabled = $derived(!safeArchiveName || useDefaultPar2Disabled);
  let useDefaultReason = $derived(
    useDefaultPar2Title ||
      (!safeArchiveName
        ? tr("gui.recovery.choose_archive_before_default_par2", "Choose an archive before using its default PAR2 path")
        : usesDefaultPar2 && useDefaultPar2Disabled
          ? tr("gui.recovery.already_using_default_par2", "Already using the default PAR2 path")
          : ""),
  );
  let testDisabled = $derived(!safeArchiveName || testArchiveDisabled);
  let testReason = $derived(
    testArchiveTitle ||
      (!safeArchiveName ? tr("gui.recovery.choose_archive_before_test", "Choose an archive before testing") : ""),
  );
</script>

<section
  class="recovery-target-picker"
  class:modern={variant === "modern"}
  class:classic={variant === "classic"}
  aria-labelledby={headingId}
>
  <header class="recovery-target-picker-head">
    <div class="recovery-target-picker-copy">
      <h2 id={headingId}>{tr("gui.recovery.targets_title", "Recovery files")}</h2>
      <p>{tr("gui.recovery.targets_body", "Choose the archive, test its integrity, then add PAR2 to verify repair capacity.")}</p>
    </div>
    <span class="recovery-target-privacy"><Icon name="lock" />{tr("gui.recovery.filename_privacy", "File names only")}</span>
  </header>

  <div class="recovery-target-grid">
    <div class="recovery-target-card">
      <header>
        <span class="recovery-target-kind"><Icon name="archive" />{tr("gui.recovery.archive_source", "Archive")}</span>
        <span class="recovery-target-state" class:selected={Boolean(safeArchiveName)}>
          {safeArchiveName
            ? usesCurrentArchive
              ? tr("gui.recovery.current_archive", "Current archive")
              : tr("gui.recovery.selected_archive", "Selected archive")
            : tr("gui.recovery.no_archive_selected", "No archive selected")}
        </span>
      </header>
      <strong class="recovery-target-name" aria-live="polite">{safeArchiveName ?? tr("gui.recovery.choose_archive_prompt", "Choose an archive")}</strong>
      <p>{tr("gui.recovery.archive_safe_detail", "Test checks the archive itself. PAR2 verification measures repair capacity; repair writes a new copy.")}</p>
      <div class="recovery-target-actions">
        <button
          type="button"
          class="main-action"
          disabled={chooseArchiveDisabled}
          title={chooseArchiveTitle || undefined}
          aria-label={labelWithReason(tr("gui.recovery.choose_archive", "Choose archive"), chooseArchiveDisabled, chooseArchiveTitle)}
          onclick={onChooseArchive}
        ><Icon name="folder-open" />{tr("gui.recovery.choose_archive", "Choose archive")}</button>
        <button
          type="button"
          class:active={usesCurrentArchive}
          disabled={useCurrentDisabled}
          title={useCurrentReason || undefined}
          aria-label={labelWithReason(tr("gui.recovery.use_current_archive", "Use current archive"), useCurrentDisabled, useCurrentReason)}
          aria-pressed={usesCurrentArchive}
          onclick={onUseCurrentArchive}
        ><Icon name="archive" />{tr("gui.recovery.use_current_archive", "Use current archive")}</button>
        <button
          type="button"
          disabled={testDisabled}
          title={testReason || undefined}
          aria-label={labelWithReason(tr("gui.recovery.test_selected_archive", "Test archive integrity"), testDisabled, testReason)}
          onclick={onTestArchive}
        ><Icon name="check-circle" />{tr("gui.recovery.test_selected_archive", "Test archive integrity")}</button>
      </div>
    </div>

    <div class="recovery-target-card">
      <header>
        <span class="recovery-target-kind"><Icon name="shield-alert" />{tr("gui.recovery.par2_index", "PAR2 file")}</span>
        <span class="recovery-target-state" class:selected={Boolean(safePar2Name) && !usesDefaultPar2}>
          {safePar2Name
            ? usesDefaultPar2
              ? tr("gui.recovery.default_par2", "Default path")
              : tr("gui.recovery.selected_par2", "Selected PAR2")
            : tr("gui.recovery.no_par2_selected", "No PAR2 selected")}
        </span>
      </header>
      <strong class="recovery-target-name" aria-live="polite">{safePar2Name ?? tr("gui.recovery.choose_par2_prompt", "Choose a PAR2 file")}</strong>
      <p>
        {usesDefaultPar2
          ? tr("gui.recovery.default_par2_detail", "Verify recovery capacity with the .par2 file beside the selected archive.")
          : tr("gui.recovery.selected_par2_detail", "Verify this PAR2 set to confirm it matches the archive and measure repair capacity.")}
      </p>
      <div class="recovery-target-actions">
        <button
          type="button"
          class="main-action"
          disabled={choosePar2Disabled}
          title={choosePar2Title || undefined}
          aria-label={labelWithReason(tr("gui.recovery.choose_par2", "Choose PAR2"), choosePar2Disabled, choosePar2Title)}
          onclick={onChoosePar2}
        ><Icon name="folder-open" />{tr("gui.recovery.choose_par2", "Choose PAR2")}</button>
        <button
          type="button"
          class:active={usesDefaultPar2}
          disabled={useDefaultDisabled}
          title={useDefaultReason || undefined}
          aria-label={labelWithReason(tr("gui.recovery.use_default_par2", "Use default PAR2"), useDefaultDisabled, useDefaultReason)}
          aria-pressed={usesDefaultPar2}
          onclick={onUseDefaultPar2}
        ><Icon name="rotate-cw" />{tr("gui.recovery.use_default_par2", "Use default PAR2")}</button>
      </div>
    </div>
  </div>
</section>
