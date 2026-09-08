<script lang="ts">
  import type {
    CreateCompletionAction,
    CreateDestinationBase,
    PostSuccessAction,
  } from "../lib/ipc";
  import Icon from "./Icon.svelte";

  let {
    instanceId,
    variant,
    destination,
    completion,
    postSuccess,
    testAfterCreate,
    testAfterCreateRequired,
    outputPreview,
    defaultFolder,
    fileManager,
    trashName,
    disabled = false,
    disabledReason = "",
    openDisabledReason = "",
    trashDisabledReason = "",
    tr,
    onDestinationChange,
    onCompletionChange,
    onPostSuccessChange,
    onTestAfterCreateChange,
  }: {
    instanceId: string;
    variant: "modern" | "classic";
    destination: CreateDestinationBase;
    completion: CreateCompletionAction;
    postSuccess: PostSuccessAction;
    testAfterCreate: boolean;
    testAfterCreateRequired: boolean;
    outputPreview: string;
    defaultFolder: string;
    fileManager: string;
    trashName: string;
    disabled?: boolean;
    disabledReason?: string;
    openDisabledReason?: string;
    trashDisabledReason?: string;
    tr: (key: string, fallback: string) => string;
    onDestinationChange: (value: CreateDestinationBase) => void;
    onCompletionChange: (value: CreateCompletionAction) => void;
    onPostSuccessChange: (value: PostSuccessAction) => void;
    onTestAfterCreateChange: (value: boolean) => void;
  } = $props();

  const destinationOptions: CreateDestinationBase[] = ["ask", "source_parent", "default_directory"];
  const completionOptions: CreateCompletionAction[] = ["none", "reveal_output", "open_in_squallz"];
  const postSuccessOptions: PostSuccessAction[] = ["keep_source", "trash_source"];

  function destinationTitle(value: CreateDestinationBase): string {
    if (value === "source_parent") return tr("gui.create.output.destination.source_parent", "Next to the sources");
    if (value === "default_directory") return tr("gui.create.output.destination.default_directory", "Default create folder");
    return tr("gui.create.output.destination.ask", "Choose every time");
  }

  function destinationDetail(value: CreateDestinationBase): string {
    if (value === "source_parent") {
      return tr(
        "gui.create.output.destination.source_parent_detail",
        "Uses the shared parent folder. If sources come from different folders, Squallz asks where to save.",
      );
    }
    if (value === "default_directory") {
      return defaultFolder
        ? tr("gui.create.output.destination.default_directory_detail", "Save automatically in {folder}.").replace("{folder}", defaultFolder)
        : tr("gui.create.output.destination.default_directory_missing", "No default create folder is set; Squallz will ask where to save.");
    }
    return tr("gui.create.output.destination.ask_detail", "Review the name and location in the save panel before compression starts.");
  }

  function completionTitle(value: CreateCompletionAction): string {
    if (value === "reveal_output") {
      return tr("gui.create.output.completion.reveal", "Reveal in {fileManager}").replace("{fileManager}", fileManager);
    }
    if (value === "open_in_squallz") return tr("gui.create.output.completion.open", "Open in Squallz");
    return tr("gui.create.output.completion.none", "Do nothing");
  }

  function completionDetail(value: CreateCompletionAction): string {
    if (value === "reveal_output") {
      return tr("gui.create.output.completion.reveal_detail", "Select the finished output in {fileManager}.").replace("{fileManager}", fileManager);
    }
    if (value === "open_in_squallz") {
      return tr("gui.create.output.completion.open_detail", "Open the finished standard archive in the Squallz browser.");
    }
    return tr("gui.create.output.completion.none_detail", "Stay on the current screen when the task finishes.");
  }

  function postSuccessTitle(value: PostSuccessAction): string {
    return value === "trash_source"
      ? tr("gui.create.output.source.trash", "Move originals to {trash}").replace("{trash}", trashName)
      : tr("gui.create.output.source.keep", "Keep originals");
  }

  function postSuccessDetail(value: PostSuccessAction): string {
    return value === "trash_source"
      ? tr("gui.create.output.source.trash_detail", "After the archive is committed, verify that every source is unchanged, then move it to {trash}.").replace("{trash}", trashName)
      : tr("gui.create.output.source.keep_detail", "Leave every source file and folder where it is.");
  }

  function optionDisabled(value: CreateCompletionAction): boolean {
    return disabled || (value === "open_in_squallz" && openDisabledReason.length > 0);
  }

  function optionReason(value: CreateCompletionAction): string {
    return value === "open_in_squallz" ? openDisabledReason || disabledReason : disabledReason;
  }

  function sourceOptionDisabled(value: PostSuccessAction): boolean {
    return disabled || (value === "trash_source" && trashDisabledReason.length > 0);
  }

  function sourceOptionReason(value: PostSuccessAction): string {
    return value === "trash_source" ? trashDisabledReason || disabledReason : disabledReason;
  }

  function integrityDetail(): string {
    if (testAfterCreateRequired) {
      return tr(
        "gui.create.output.integrity.required_detail",
        "Required before originals can move to {trash}. Squallz reopens the committed output and reads every entry.",
      ).replace("{trash}", trashName);
    }
    return tr(
      "gui.create.output.integrity.detail",
      "Reopen the output and read every entry to catch write or storage errors. Adds one full read.",
    );
  }
</script>

{#snippet destinationFields()}
  <fieldset class="create-output-fieldset" disabled={disabled} title={disabledReason}>
    <legend>{tr("gui.create.output.destination.title", "Save location")}</legend>
    <div class="create-output-options" role="radiogroup">
      {#each destinationOptions as value}
        <label class="create-output-choice" class:selected={destination === value} class:disabled>
          <input
            type="radio"
            name={`${instanceId}-create-destination`}
            value={value}
            checked={destination === value}
            {disabled}
            aria-describedby={`${instanceId}-destination-${value}-detail`}
            onchange={() => onDestinationChange(value)}
          />
          <span>
            <strong>{destinationTitle(value)}</strong>
            <small
              id={`${instanceId}-destination-${value}-detail`}
              class:create-output-dynamic-path={value === "default_directory" && defaultFolder.length > 0}
            >{destinationDetail(value)}</small>
          </span>
        </label>
      {/each}
    </div>
  </fieldset>
{/snippet}

{#snippet completionFields()}
  <fieldset class="create-output-fieldset" disabled={disabled} title={disabledReason}>
    <legend>{tr("gui.create.output.completion.title", "When finished")}</legend>
    <div class="create-output-options" role="radiogroup">
      {#each completionOptions as value}
        <label
          class="create-output-choice"
          class:selected={completion === value}
          class:disabled={optionDisabled(value)}
          title={optionReason(value)}
        >
          <input
            type="radio"
            name={`${instanceId}-create-completion`}
            value={value}
            checked={completion === value}
            disabled={optionDisabled(value)}
            aria-label={optionReason(value) ? `${completionTitle(value)} · ${optionReason(value)}` : completionTitle(value)}
            aria-describedby={`${instanceId}-completion-${value}-detail`}
            onchange={() => onCompletionChange(value)}
          />
          <span>
            <strong>{completionTitle(value)}</strong>
            <small id={`${instanceId}-completion-${value}-detail`}>{optionReason(value) || completionDetail(value)}</small>
          </span>
        </label>
      {/each}
    </div>
  </fieldset>
{/snippet}

{#snippet integrityFields()}
  <fieldset class="create-output-fieldset" disabled={disabled} title={disabledReason}>
    <legend>{tr("gui.create.output.integrity.title", "Archive integrity")}</legend>
    <div class="create-output-options create-output-options-compact">
      <label
        class="create-output-choice"
        class:selected={testAfterCreate}
        class:disabled={disabled || testAfterCreateRequired}
        title={testAfterCreateRequired ? integrityDetail() : disabledReason}
      >
        <input
          type="checkbox"
          name={`${instanceId}-create-integrity-test`}
          checked={testAfterCreate}
          disabled={disabled || testAfterCreateRequired}
          aria-describedby={`${instanceId}-integrity-test-detail`}
          onchange={(event) => onTestAfterCreateChange((event.currentTarget as HTMLInputElement).checked)}
        />
        <span>
          <strong>{tr("gui.create.output.integrity.test_after_create", "Test archive after creation")}</strong>
          <small id={`${instanceId}-integrity-test-detail`}>{integrityDetail()}</small>
        </span>
      </label>
    </div>
  </fieldset>
{/snippet}

{#snippet sourceFields()}
  <fieldset class="create-output-fieldset" disabled={disabled} title={disabledReason}>
    <legend>{tr("gui.create.output.source.title", "Original sources")}</legend>
    <div class="create-output-options create-output-options-compact" role="radiogroup">
      {#each postSuccessOptions as value}
        <label
          class="create-output-choice"
          class:selected={postSuccess === value}
          class:warning={value === "trash_source"}
          class:disabled={sourceOptionDisabled(value)}
          title={sourceOptionReason(value)}
        >
          <input
            type="radio"
            name={`${instanceId}-create-source-action`}
            value={value}
            checked={postSuccess === value}
            disabled={sourceOptionDisabled(value)}
            aria-label={sourceOptionReason(value) ? `${postSuccessTitle(value)} · ${sourceOptionReason(value)}` : postSuccessTitle(value)}
            aria-describedby={`${instanceId}-source-${value}-detail`}
            onchange={() => onPostSuccessChange(value)}
          />
          <span>
            <strong>{postSuccessTitle(value)}</strong>
            <small id={`${instanceId}-source-${value}-detail`}>{sourceOptionReason(value) || postSuccessDetail(value)}</small>
          </span>
        </label>
      {/each}
    </div>
    {#if postSuccess === "trash_source"}
      <p class="create-output-warning" role="status">
        <Icon name="alert-triangle" size={15} />
        <span>{tr("gui.create.output.source.trash_warning", "Check the archive before emptying {trash}. If Squallz cannot move every source, it leaves the rest in place and reports the result.").replace("{trash}", trashName)}</span>
      </p>
    {/if}
  </fieldset>
{/snippet}

{#if variant === "modern"}
  <section class="create-output-card">
    <header>
      <div>
        <h2><Icon name="archive" size={16} />{tr("gui.create.output.title", "Output")}</h2>
        <p>{tr("gui.create.output.intro", "Choose where the archive goes and what Squallz does after a successful task.")}</p>
      </div>
      <div class="create-output-preview">
        <span>{tr("gui.create.output.preview", "Output preview")}</span>
        <strong>{outputPreview}</strong>
      </div>
    </header>
    {@render destinationFields()}
    {@render completionFields()}
    {@render integrityFields()}
    {@render sourceFields()}
  </section>
{:else}
  <div class="classic-label classic-create-section-target">{tr("gui.create.output.destination.title", "Save location")}</div>
  <div class="classic-input create-output-classic">{@render destinationFields()}</div>
  <div class="classic-label">{tr("gui.create.output.completion.title", "When finished")}</div>
  <div class="classic-input create-output-classic">{@render completionFields()}</div>
  <div class="classic-label">{tr("gui.create.output.integrity.title", "Archive integrity")}</div>
  <div class="classic-input create-output-classic">{@render integrityFields()}</div>
  <div class="classic-label">{tr("gui.create.output.source.title", "Original sources")}</div>
  <div class="classic-input create-output-classic">{@render sourceFields()}</div>
{/if}
