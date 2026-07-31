<script lang="ts">
  import packageInfo from "../../package.json";
  import {
    checkForSoftwareUpdates,
    updateCheckErrorKey,
    updateCheckLastSuccessAt,
    updateCheckPhase,
    updateCheckResult,
    type UpdateCheckPreview,
  } from "../lib/app-update.svelte";
  import { copyTextToClipboard } from "../lib/clipboard";
  import Icon from "./Icon.svelte";

  let {
    tr,
    preview = null,
    automaticChecksEnabled,
  }: {
    tr: (key: string, fallback: string) => string;
    preview?: UpdateCheckPreview;
    automaticChecksEnabled: boolean;
  } = $props();

  let phase = $derived(updateCheckPhase());
  let result = $derived(updateCheckResult());
  let errorKey = $derived(updateCheckErrorKey());
  let lastSuccessAt = $derived(updateCheckLastSuccessAt());
  let actionMessage = $state("");
  let actionFailed = $state(false);

  function currentVersion(): string {
    return result?.currentVersion ?? packageInfo.version;
  }

  function statusLabel(): string {
    if (phase === "checking") return tr("gui.update.status.checking", "Checking");
    if (phase === "error") return tr("gui.update.status.failed", "Check failed");
    if (!result) {
      if (lastSuccessAt !== null) {
        return tr("gui.update.status.checked_recently", "Checked recently");
      }
      return automaticChecksEnabled
        ? tr("gui.update.status.automatic", "Automatic")
        : tr("gui.update.status.manual", "Manual check");
    }
    if (result.status === "update_available") {
      return tr("gui.update.status.available", "Update available");
    }
    if (result.status === "ahead") {
      return tr("gui.update.status.ahead", "Newer build");
    }
    return tr("gui.update.status.current", "Up to date");
  }

  function statusIcon(): string {
    if (phase === "checking") return "hourglass";
    if (phase === "error") return "alert-triangle";
    if (result?.status === "update_available") return "sparkles";
    if (result) return "check-circle";
    return "shield-check";
  }

  function statusTone(): string {
    if (phase === "error") return "danger";
    if (result?.status === "update_available") return "accent";
    if (result) return "success";
    return "neutral";
  }

  function summary(): string {
    if (phase === "checking") {
      return tr("gui.update.checking_body", "Contacting the stable Squallz release channel on GitHub.");
    }
    if (phase === "error") {
      return tr(errorKey, tr("gui.update.failed_body", "Squallz could not check GitHub Releases. Check your connection and try again."));
    }
    if (result?.status === "update_available") {
      if (result.metadataSource === "latest_release_manifest") {
        return tr("gui.update.available_manifest_body", "Squallz {version} is ready. GitHub limited its release API, so Squallz recovered the exact package and SHA-256 from the published release manifest. Review it before downloading.")
          .replace("{version}", `v${result.latestVersion}`);
      }
      if (result.metadataSource === "latest_release_redirect") {
        return tr("gui.update.available_fallback_body", "Squallz {version} is ready. GitHub limited package details, so review and download it from the release page.")
          .replace("{version}", `v${result.latestVersion}`);
      }
      return tr("gui.update.available_body", "Squallz {version} is ready. Review the package trust state before downloading.")
        .replace("{version}", `v${result.latestVersion}`);
    }
    if (result?.status === "ahead") {
      return tr("gui.update.ahead_body", "This build is newer than the latest stable GitHub Release.");
    }
    if (result) {
      return tr("gui.update.current_body", "You have the latest stable version published on GitHub.");
    }
    if (lastSuccessAt !== null) {
      return automaticChecksEnabled
        ? tr("gui.update.recent_check_body", "The stable release channel was checked successfully within the last 24 hours. You can still check again now.")
        : tr("gui.update.recent_manual_body", "The stable release channel was checked successfully within the last 24 hours. Automatic checks are off.");
    }
    return automaticChecksEnabled
      ? tr("gui.update.automatic_idle_body", "Squallz checks the stable GitHub Releases channel at most once every 24 hours. Updates are never installed silently.")
      : tr("gui.update.idle_body", "Check the stable GitHub Releases channel when you choose. Squallz never installs an update silently.");
  }

  function lastCheckedLabel(): string {
    if (lastSuccessAt === null) return "";
    const checkedAt = new Date(lastSuccessAt);
    if (Number.isNaN(checkedAt.getTime())) return "";
    const locale = typeof document === "undefined"
      ? undefined
      : document.documentElement.lang || undefined;
    const formatted = new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "medium",
    }).format(checkedAt);
    return tr("gui.update.last_successful_check", "Last successful check: {time}")
      .replace("{time}", formatted);
  }

  let lastCheckedText = $derived(lastCheckedLabel());

  function platformLabel(value: string): string {
    if (value === "macos") return "macOS";
    if (value === "windows") return "Windows";
    if (value === "linux") return "Linux";
    return value;
  }

  function packageDetail(): string {
    if (!result?.assetName) {
      if (result?.metadataSource === "latest_release_redirect") {
        return tr("gui.update.package_metadata_limited", "GitHub limited detailed package metadata. Open the release page to choose and verify the package.");
      }
      return tr("gui.update.package_unavailable", "No package is published for this platform and architecture.");
    }
    const size = result.assetSizeBytes === null
      ? ""
      : ` · ${formatBytes(result.assetSizeBytes)}`;
    return `${result.assetName}${size}`;
  }

  function trustLabel(): string {
    if (result?.assetTrust === "developer_id_notarized") {
      return tr("gui.update.trust.notarized", "Developer ID signed, notarized, and accompanied by trust evidence.");
    }
    if (result?.assetTrust === "unsigned_preview") {
      return tr("gui.update.trust.unsigned", "Unsigned package. Verify its SHA-256 and GitHub Artifact Attestation before opening it.");
    }
    return tr("gui.update.trust.unavailable", "Open the release page to review available packages and verification details.");
  }

  function trustTone(): string {
    if (result?.assetTrust === "developer_id_notarized") return "success";
    if (result?.assetTrust === "unsigned_preview") return "warning";
    return "neutral";
  }

  function formatBytes(value: number): string {
    const units = ["B", "KiB", "MiB", "GiB"];
    let size = Math.max(0, value);
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    const digits = unit === 0 || size >= 10 ? 0 : 1;
    return `${size.toFixed(digits)} ${units[unit]}`;
  }

  async function checkForUpdates(): Promise<void> {
    actionMessage = "";
    actionFailed = false;
    await checkForSoftwareUpdates("manual", preview);
  }

  function trustedUpdateUrl(value: string, kind: "download" | "release"): boolean {
    if (!result) return false;
    const tag = `v${result.latestVersion}`;
    if (kind === "release") {
      return value === `https://github.com/yangzhg/Squallz/releases/tag/${tag}`;
    }
    if (!result.assetName) return false;
    return value
      === `https://github.com/yangzhg/Squallz/releases/download/${tag}/${result.assetName}`;
  }

  async function openUpdateUrl(url: string, kind: "download" | "release"): Promise<void> {
    actionMessage = "";
    actionFailed = false;
    if (!trustedUpdateUrl(url, kind)) {
      actionFailed = true;
      actionMessage = tr("gui.update.open_failed", "The verified Squallz release link could not be opened.");
      return;
    }
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(url);
      actionMessage = kind === "download"
        ? tr("gui.update.download_opened", "Opened the package download in your browser. Verify it before installing.")
        : tr("gui.update.release_opened", "Opened the Squallz release page in your browser.");
    } catch {
      actionFailed = true;
      actionMessage = tr("gui.update.open_failed", "The verified Squallz release link could not be opened.");
    }
  }

  function openDownload(): void {
    const url = result?.downloadUrl;
    if (url) void openUpdateUrl(url, "download");
  }

  function openRelease(): void {
    const url = result?.releaseUrl;
    if (url) void openUpdateUrl(url, "release");
  }

  async function copyPackageSha256(): Promise<void> {
    const digest = result?.assetSha256 ?? "";
    actionMessage = "";
    actionFailed = false;
    const copied = await copyTextToClipboard(digest);
    actionFailed = !copied;
    actionMessage = copied
      ? tr("gui.update.sha256_copied", "SHA-256 copied.")
      : tr("gui.update.sha256_copy_failed", "Could not copy the SHA-256.");
  }
</script>

<section class={`update-check-card state-${statusTone()}`} aria-live="polite" aria-busy={phase === "checking"}>
  <div class="update-check-heading">
    <div class="update-check-mark">
      <Icon name={statusIcon()} size={18} />
    </div>
    <div class="update-check-title">
      <strong>{tr("gui.update.title", "Software updates")}</strong>
      <span>{summary()}</span>
      {#if lastCheckedText}
        <small class="update-check-last-checked">{lastCheckedText}</small>
      {/if}
    </div>
    <span class={`update-check-status tone-${statusTone()}`}>{statusLabel()}</span>
  </div>

  <div class="update-check-version-grid">
    <div>
      <span>{tr("gui.update.installed_version", "Installed")}</span>
      <strong>v{currentVersion()}</strong>
    </div>
    <div>
      <span>{tr("gui.update.stable_version", "Latest stable")}</span>
      <strong>{result ? `v${result.latestVersion}` : "—"}</strong>
    </div>
    <div>
      <span>{tr("gui.update.channel", "Channel")}</span>
      <strong>{tr("gui.update.channel_stable", "Stable · GitHub")}</strong>
    </div>
  </div>

  {#if result?.status === "update_available"}
    <div class="update-check-package">
      <div>
        <span>{tr("gui.update.package", "Package")}</span>
        <strong>{platformLabel(result.platform)} · {result.architecture}</strong>
        <small>{packageDetail()}</small>
      </div>
      <div class={`update-check-trust tone-${trustTone()}`}>
        <Icon name={result.assetTrust === "developer_id_notarized" ? "shield-check" : "alert-triangle"} size={16} />
        <span>{trustLabel()}</span>
      </div>
      {#if result.assetSha256}
        <div class="update-check-digest">
          <div>
            <span>{tr("gui.update.sha256", "Package SHA-256")}</span>
            <code>{result.assetSha256}</code>
          </div>
          <button type="button" class="secondary-lite" onclick={() => void copyPackageSha256()}>
            <Icon name="copy" size={15} />{tr("gui.update.copy_sha256", "Copy SHA-256")}
          </button>
        </div>
      {/if}
    </div>
  {/if}

  <div class="update-check-actions">
    <button
      type="button"
      class={result?.status === "update_available" ? "secondary-lite" : "primary-lite"}
      disabled={phase === "checking"}
      onclick={() => void checkForUpdates()}
    >
      <Icon name={phase === "checking" ? "hourglass" : "rotate-cw"} size={15} />
      {phase === "checking"
        ? tr("gui.update.checking", "Checking…")
        : result || phase === "error"
          ? tr("gui.update.check_again", "Check again")
          : tr("gui.update.check", "Check for updates")}
    </button>
    {#if result?.status === "update_available" && result.downloadUrl}
      <button type="button" class="primary-lite" onclick={openDownload}>
        <Icon name="external-link" size={15} />{tr("gui.update.download", "Download package")}
      </button>
    {/if}
    {#if result}
      <button type="button" class="secondary-lite" onclick={openRelease}>
        <Icon name="external-link" size={15} />{tr("gui.update.release_notes", "Release details")}
      </button>
    {/if}
  </div>

  {#if actionMessage}
    <p class:failed={actionFailed} class="update-check-feedback" role="status">{actionMessage}</p>
  {/if}
</section>
