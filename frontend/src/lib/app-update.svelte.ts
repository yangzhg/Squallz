import packageInfo from "../../package.json";
import { ipc, isErrorDto, type AppUpdateCheckDto } from "./ipc";

export type UpdateCheckPreview =
  | "available"
  | "manifest"
  | "current"
  | "ahead"
  | "error"
  | null;
export type UpdateCheckPhase = "idle" | "checking" | "ready" | "error";
export type UpdateCheckReason = "automatic" | "manual";

const LAST_SUCCESS_KEY = "squallz.update.lastSuccess.v1";
const LAST_RESULT_KEY = "squallz.update.lastResult.v1";
export const AUTOMATIC_UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

interface CachedUpdateCheck {
  checkedAt: number;
  result: AppUpdateCheckDto;
}

const cachedCheck = cachedSuccessfulCheck();
const store = $state({
  phase: (cachedCheck ? "ready" : "idle") as UpdateCheckPhase,
  result: cachedCheck?.result ?? null as AppUpdateCheckDto | null,
  errorKey: "",
  lastSuccessAt: cachedCheck?.checkedAt ?? lastSuccessfulCheck(),
});

let pendingCheck: Promise<AppUpdateCheckDto | null> | null = null;

export function updateCheckPhase(): UpdateCheckPhase {
  return store.phase;
}

export function updateCheckResult(): AppUpdateCheckDto | null {
  return store.result;
}

export function updateCheckErrorKey(): string {
  return store.errorKey;
}

export function updateCheckLastSuccessAt(): number | null {
  return store.lastSuccessAt;
}

function lastSuccessfulCheck(): number | null {
  if (typeof window === "undefined") return null;
  try {
    const value = Number(window.localStorage.getItem(LAST_SUCCESS_KEY));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function cachedSuccessfulCheck(): CachedUpdateCheck | null {
  if (typeof window === "undefined") return null;
  try {
    const stored = window.localStorage.getItem(LAST_RESULT_KEY);
    if (stored === null) return null;
    const parsed: unknown = JSON.parse(stored);
    if (!isRecord(parsed) || !recentTimestamp(parsed.checkedAt) || !validUpdateResult(parsed.result)) {
      clearCachedCheck();
      return null;
    }
    return {
      checkedAt: parsed.checkedAt,
      result: parsed.result,
    };
  } catch {
    clearCachedCheck();
    return null;
  }
}

function clearCachedCheck(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(LAST_RESULT_KEY);
    window.localStorage.removeItem(LAST_SUCCESS_KEY);
  } catch {
    // A private WebView profile may reject non-critical preference metadata.
  }
}

function recentTimestamp(value: unknown): value is number {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) return false;
  const age = Date.now() - value;
  return age >= 0 && age < AUTOMATIC_UPDATE_CHECK_INTERVAL_MS;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function validUpdateResult(value: unknown): value is AppUpdateCheckDto {
  if (!isRecord(value)) return false;
  if (
    value.status !== "up_to_date"
    && value.status !== "update_available"
    && value.status !== "ahead"
  ) return false;
  if (
    value.assetTrust !== "developer_id_notarized"
    && value.assetTrust !== "unsigned_preview"
    && value.assetTrust !== "unavailable"
  ) return false;
  if (
    value.metadataSource !== "github_api"
    && value.metadataSource !== "latest_release_redirect"
    && value.metadataSource !== "latest_release_manifest"
  ) {
    return false;
  }
  if (
    value.currentVersion !== packageInfo.version
    || typeof value.latestVersion !== "string"
    || !/^\d+\.\d+\.\d+(?:\+[0-9A-Za-z.-]+)?$/.test(value.latestVersion)
    || typeof value.releaseName !== "string"
    || value.releaseName.length === 0
    || value.releaseName.length > 120
    || typeof value.publishedAt !== "string"
    || value.publishedAt.length > 64
    || !boundedTargetName(value.platform)
    || !boundedTargetName(value.architecture)
  ) return false;

  const tag = `v${value.latestVersion}`;
  const expectedReleaseUrl = `https://github.com/yangzhg/Squallz/releases/tag/${tag}`;
  if (value.releaseUrl !== expectedReleaseUrl) return false;
  if (value.metadataSource === "latest_release_redirect" && value.assetName !== null) return false;
  if (value.metadataSource === "latest_release_manifest" && value.assetName === null) return false;

  if (value.assetName === null) {
    return value.downloadUrl === null
      && value.assetSizeBytes === null
      && value.assetSha256 === null
      && value.assetTrust === "unavailable";
  }
  if (
    typeof value.assetName !== "string"
    || !/^[A-Za-z0-9._-]{1,180}$/.test(value.assetName)
    || typeof value.assetSizeBytes !== "number"
    || !Number.isSafeInteger(value.assetSizeBytes)
    || value.assetSizeBytes < 0
    || (value.assetSha256 !== null
      && (typeof value.assetSha256 !== "string"
        || !/^[0-9a-f]{64}$/.test(value.assetSha256)))
  ) return false;
  const expectedDownloadUrl =
    `https://github.com/yangzhg/Squallz/releases/download/${tag}/${value.assetName}`;
  if (value.downloadUrl !== null && value.downloadUrl !== expectedDownloadUrl) return false;
  if (
    value.metadataSource === "latest_release_redirect"
    && (value.downloadUrl !== null
      || value.assetSha256 !== null
      || value.assetTrust !== "unavailable")
  ) return false;
  if (
    value.metadataSource === "latest_release_manifest"
    && (value.downloadUrl !== expectedDownloadUrl
      || value.assetSha256 === null
      || value.assetTrust !== "unsigned_preview")
  ) return false;
  return true;
}

function boundedTargetName(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 32
    && /^[A-Za-z0-9_-]+$/.test(value);
}

function rememberSuccessfulCheck(result: AppUpdateCheckDto): void {
  const checkedAt = Date.now();
  store.lastSuccessAt = checkedAt;
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      LAST_RESULT_KEY,
      JSON.stringify({ checkedAt, result } satisfies CachedUpdateCheck),
    );
    window.localStorage.setItem(LAST_SUCCESS_KEY, String(checkedAt));
  } catch {
    // A private WebView profile may reject non-critical preference metadata.
  }
}

export function automaticUpdateCheckDue(now = Date.now()): boolean {
  const lastCheck = store.lastSuccessAt;
  if (lastCheck === null) return true;
  const elapsed = now - lastCheck;
  return elapsed < 0 || elapsed >= AUTOMATIC_UPDATE_CHECK_INTERVAL_MS;
}

function previewResult(kind: Exclude<UpdateCheckPreview, "error" | null>): AppUpdateCheckDto {
  const updateAvailable = kind === "available" || kind === "manifest";
  const latestVersion = updateAvailable
    ? "0.2.0"
    : kind === "ahead"
      ? "0.0.9"
      : packageInfo.version;
  const status = updateAvailable
    ? "update_available" as const
    : kind === "ahead"
      ? "ahead" as const
      : "up_to_date" as const;
  const signedPackage = kind === "available";
  const extension = signedPackage ? "dmg" : "app.zip";
  const assetName = `Squallz-v${latestVersion}-macos-arm64.${extension}`;
  return {
    status,
    currentVersion: packageInfo.version,
    latestVersion,
    releaseName: `Squallz v${latestVersion}`,
    releaseUrl: `https://github.com/yangzhg/Squallz/releases/tag/v${latestVersion}`,
    publishedAt: "2026-07-28T12:00:00Z",
    platform: "macos",
    architecture: "arm64",
    assetName,
    downloadUrl: `https://github.com/yangzhg/Squallz/releases/download/v${latestVersion}/${assetName}`,
    assetSizeBytes: 12_582_912,
    assetSha256: "a".repeat(64),
    assetTrust: signedPackage ? "developer_id_notarized" : "unsigned_preview",
    metadataSource: kind === "manifest" ? "latest_release_manifest" : "github_api",
  };
}

async function runCheck(preview: UpdateCheckPreview): Promise<AppUpdateCheckDto | null> {
  store.phase = "checking";
  store.result = null;
  store.errorKey = "";

  if (preview === "error") {
    store.errorKey = "error.update.network";
    store.phase = "error";
    return null;
  }
  if (preview !== null) {
    store.result = previewResult(preview);
    store.lastSuccessAt = Date.now();
    store.phase = "ready";
    return store.result;
  }

  try {
    const result = await ipc.checkForUpdates();
    store.result = result;
    store.phase = "ready";
    rememberSuccessfulCheck(result);
    return result;
  } catch (error) {
    store.errorKey = isErrorDto(error) ? error.key : "error.update.unavailable";
    store.phase = "error";
    return null;
  }
}

export function checkForSoftwareUpdates(
  reason: UpdateCheckReason,
  preview: UpdateCheckPreview = null,
): Promise<AppUpdateCheckDto | null> {
  if (reason === "automatic" && preview === null && !automaticUpdateCheckDue()) {
    return Promise.resolve(store.result);
  }
  if (pendingCheck) return pendingCheck;
  pendingCheck = runCheck(preview).finally(() => {
    pendingCheck = null;
  });
  return pendingCheck;
}
