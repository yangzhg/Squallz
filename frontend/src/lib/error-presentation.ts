import type { ErrorDto } from "./ipc";
import { tFallback, tError } from "./i18n.svelte";

function translatedError(error: ErrorDto, fallback: string): string {
  const translated = tError(error);
  return translated === error.key ? tFallback(error.key, fallback, error.params) : translated;
}

export function errorSummary(error: ErrorDto | null): string {
  if (!error) return tFallback("gui.error.other.title", "Operation failed");
  const key = error.key;
  if (key === "error.password_required") return translatedError(error, "A password is required");
  if (key === "error.wrong_password") return translatedError(error, "Wrong password");
  if (key === "error.cancelled") return translatedError(error, "Operation cancelled");
  if (key === "error.disk_full") return tFallback("gui.error.disk_full.title", "Not enough disk space");
  if (key === "error.dependency_missing") {
    return tFallback("gui.error.dependency.title", "Additional component required");
  }
  if (key === "error.sfx_recovery") {
    return translatedError(error, "Self-extractor replacement needs recovery at {target}");
  }
  if (key === "error.recovery_cleanup_output_ready") {
    return translatedError(error, "The repaired copy is ready, but cleanup needs attention");
  }
  if (key === "error.recovery_cleanup_unconfirmed") {
    return translatedError(error, "PAR2 repair cleanup needs attention");
  }
  if (key === "error.recovery_cleanup_record") {
    return translatedError(error, "PAR2 repair recovery record needs attention");
  }
  if (key === "error.destination_changed") {
    return tFallback("gui.error.destination_changed.title", "Destination changed");
  }
  if (key === "error.input_changed") {
    return tFallback("gui.error.input_changed.title", "Archive changed");
  }
  if (key === "error.output_exists") {
    return tFallback("gui.error.output_exists.title", "Output location is occupied");
  }
  if (key === "error.io") return tFallback("gui.error.io.title", "Read/write error");
  if (key === "error.unsupported_split_wim_create") {
    return translatedError(
      error,
      "Creating .swm requires a split size and the Native Split WIM layout.",
    );
  }
  if (key === "error.unsupported_split_wim") {
    return translatedError(
      error,
      "This Split WIM stream has no source folder. Open any .swm member from disk and keep every part together.",
    );
  }
  if (key === "error.unsupported") {
    return tFallback("gui.error.unsupported.title", "Cannot handle this file type");
  }
  if (key === "gui.error.corrupt.volume_missing") {
    return translatedError(error, "Volume {name} is missing. Keep all volumes in the same folder.");
  }
  if (key === "error.corrupt_archive") return tFallback("gui.error.corrupt.title", "Archive is damaged");
  if (key === "error.path_traversal" || key === "error.symlink_breakout" || key === "error.unsafe_filename") {
    return tFallback("gui.task.error.unsafe_archive", "Squallz blocked unsafe archive content");
  }
  if (key === "error.resource_limit") {
    return tFallback("gui.task.error.resource_limit", "The task exceeded a safety limit");
  }
  return tFallback("gui.error.other.title", "Operation failed");
}
