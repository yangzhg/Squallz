<script lang="ts" module>
  let rememberedIdentity = "";
  let rememberedProfile = "";
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import SfxPublishDialog, {
    type SfxPublishRequest,
  } from "./SfxPublishDialog.svelte";
  import type { CssVariableMap } from "../lib/css-variables";
  import type { DesktopPathPlatform } from "../lib/desktop-path";
  import type { JobSpec } from "../lib/ipc";
  import {
    macosSfxPublishJobSpec,
    prepareMacosSfxPublish,
    suggestedMacosSfxPublishPath,
    taskCanPublishMacosSfx,
  } from "../lib/macos-sfx-publish";
  import {
    taskOutputPath,
    type TaskDialogModel,
  } from "../lib/task-model";
  import { tFallback as tr } from "../lib/i18n.svelte";

  let {
    task,
    rootClass,
    rootVariables,
    returnFocus = null,
    platform,
    previewSkipSave,
    chooseOutput,
    submitJob,
    formatSubmitError,
    onNotice,
    onClose,
  }: {
    task: TaskDialogModel;
    rootClass: string;
    rootVariables: CssVariableMap;
    returnFocus?: HTMLElement | null;
    platform: DesktopPathPlatform;
    previewSkipSave: boolean;
    chooseOutput: (suggested: string) => Promise<string | null>;
    submitJob: (spec: JobSpec) => Promise<number>;
    formatSubmitError: (error: unknown) => string | null;
    onNotice: (message: string) => void;
    onClose: () => void;
  } = $props();

  let source = $state("");
  let output = $state("");
  let initialIdentity = $state(rememberedIdentity);
  let initialProfile = $state(rememberedProfile);


  onMount(() => {
    let active = true;
    void prepare().then((draft) => {
      if (!active || !draft) return;
      source = draft.source;
      output = draft.output;
    });
    return () => {
      active = false;
    };
  });

  async function prepare(): Promise<Readonly<{ source: string; output: string }> | null> {
    const sourcePath = task.spec.kind === "publish_macos_sfx"
      ? task.spec.source
      : taskOutputPath(task);
    if (!sourcePath || !taskCanPublishMacosSfx(task)) {
      onClose();
      return null;
    }
    const suggested = suggestedMacosSfxPublishPath(sourcePath, platform);
    let selected: string | null;
    try {
      selected = previewSkipSave ? suggested : await chooseOutput(suggested);
    } catch {
      onNotice(
        tr(
          "gui.sfx_publish.save_dialog_failed",
        ),
      );
      onClose();
      return null;
    }
    if (!selected) {
      onClose();
      return null;
    }
    const prepared = prepareMacosSfxPublish(sourcePath, selected, platform);
    if (prepared.kind === "same_output") {
      onNotice(
        tr(
          "gui.sfx_publish.separate_output_required",
        ),
      );
      onClose();
      return null;
    }
    return { source: sourcePath, output: prepared.output };
  }

  async function submit(request: SfxPublishRequest): Promise<string | null> {
    try {
      await submitJob(macosSfxPublishJobSpec(request));
      rememberedIdentity = request.identity;
      rememberedProfile = request.notaryProfile;
      initialIdentity = request.identity;
      initialProfile = request.notaryProfile;
      onClose();
      onNotice(
        tr(
          "gui.sfx_publish.queued",
        ),
      );
      return null;
    } catch (error) {
      return formatSubmitError(error) ?? tr(
        "gui.sfx_publish.submit_failed",
      );
    }
  }
</script>

{#if source && output}
  <SfxPublishDialog
    {rootClass}
    {rootVariables}
    {returnFocus}
    {source}
    {output}
    {initialIdentity}
    {initialProfile}
    onSubmit={submit}
    onCancel={onClose}
  />
{/if}
