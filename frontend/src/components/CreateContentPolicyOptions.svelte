<script lang="ts">
  import type { CreateContentPolicy } from "../lib/ipc";
  import ExcludeRulesEditor from "./ExcludeRulesEditor.svelte";
  import Icon from "./Icon.svelte";

  let {
    variant,
    classicSectionId,
    value,
    rulesText,
    rules,
    disabled = false,
    disabledReason = "",
    tr,
    onChange,
    onRulesInput,
  }: {
    variant: "modern" | "classic";
    classicSectionId?: string;
    value: CreateContentPolicy;
    rulesText: string;
    rules: string[];
    disabled?: boolean;
    disabledReason?: string;
    tr: (key: string, fallback?: string) => string;
    onChange: (value: CreateContentPolicy) => void;
    onRulesInput: (value: string) => void;
  } = $props();

  const policyIds: CreateContentPolicy[] = ["cross_platform_clean", "keep_all_files", "custom"];

  function policyTitle(policy: CreateContentPolicy): string {
    if (policy === "cross_platform_clean") {
      return tr("gui.create.content_policy.clean");
    }
    if (policy === "keep_all_files") {
      return tr("gui.create.content_policy.keep_all");
    }
    return tr("gui.create.content_policy.custom");
  }

  function policyDetail(policy: CreateContentPolicy): string {
    if (policy === "cross_platform_clean") {
      return tr(
        "gui.create.content_policy.clean_detail",
      );
    }
    if (policy === "keep_all_files") {
      return tr(
        "gui.create.content_policy.keep_all_detail",
      );
    }
    return tr("gui.create.content_policy.custom_detail");
  }
</script>

{#snippet policyFields()}
  <div class="content-policy-options" role="radiogroup" aria-label={tr("gui.create.content_policy.title")}>
    {#each policyIds as policy}
      <label class="content-policy-option" class:selected={value === policy} class:disabled title={disabledReason}>
        <input
          type="radio"
          name={`${variant}-create-content-policy`}
          value={policy}
          checked={value === policy}
          {disabled}
          aria-describedby={`${variant}-content-policy-${policy}-detail`}
          aria-label={disabledReason ? `${policyTitle(policy)} · ${disabledReason}` : policyTitle(policy)}
          onchange={() => onChange(policy)}
        />
        <span class="content-policy-copy">
          <strong>
            {policyTitle(policy)}
            {#if policy === "cross_platform_clean"}
              <em>{tr("gui.create.content_policy.recommended")}</em>
            {/if}
          </strong>
          <small id={`${variant}-content-policy-${policy}-detail`}>{policyDetail(policy)}</small>
        </span>
      </label>
    {/each}
  </div>

  {#if value === "custom"}
    <ExcludeRulesEditor
      title={tr("gui.excludes.title")}
      hint={tr("gui.excludes.create_hint")}
      countLabel={tr("gui.excludes.count").replace("{count}", String(rules.length))}
      value={rulesText}
      placeholder={tr("gui.excludes.placeholder", ".git\nnode_modules\n*.tmp")}
      ariaLabel={tr("gui.create.exclude_glob_rules")}
      {rules}
      emptyLabel={tr("gui.create.no_rules")}
      {disabled}
      {disabledReason}
      onInput={onRulesInput}
    />
  {/if}
{/snippet}

{#if variant === "modern"}
  <section class="content-policy-card">
    <h2><Icon name="folder-open" size={16} />{tr("gui.create.content_policy.title")}</h2>
    <p>{tr("gui.create.content_policy.intro", "Choose what to do with platform-specific helper files.")}</p>
    {@render policyFields()}
  </section>
{:else}
  <div id={classicSectionId} class="classic-label classic-create-section-target">{tr("gui.create.content_policy.title")}</div>
  <div class="classic-input content-policy-classic">{@render policyFields()}</div>
{/if}
