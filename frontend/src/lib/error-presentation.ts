import type { ErrorDto } from "./ipc";
import { tFallback } from "./i18n.svelte";

export function errorSummary(error: ErrorDto | null): string {
  if (!error) return tFallback("gui.error.other.title");
  const key = error.key;
  if (key === "error.password_required" || key === "error.wrong_password" || key === "error.cancelled"
    || key === "error.sfx_recovery" || key === "error.recovery_cleanup_output_ready"
    || key === "error.recovery_cleanup_unconfirmed" || key === "error.recovery_cleanup_record"
    || key === "error.unsupported_split_wim_create" || key === "error.unsupported_split_wim"
    || key === "gui.error.corrupt.volume_missing") {
    return tFallback(key, error.params);
  }
  if (key === "error.disk_full") return tFallback("gui.error.disk_full.title");
  if (key === "error.dependency_missing") {
    return tFallback("gui.error.dependency.title");
  }
  if (key === "error.destination_changed") {
    return tFallback("gui.error.destination_changed.title");
  }
  if (key === "error.input_changed") {
    return tFallback("gui.error.input_changed.title");
  }
  if (key === "error.output_exists") {
    return tFallback("gui.error.output_exists.title");
  }
  if (key === "error.io") return tFallback("gui.error.io.title");
  if (key === "error.unsupported") {
    return tFallback("gui.error.unsupported.title");
  }
  if (key === "error.corrupt_archive") return tFallback("gui.error.corrupt.title");
  if (key === "error.path_traversal" || key === "error.symlink_breakout" || key === "error.unsafe_filename") {
    return tFallback("gui.task.error.unsafe_archive");
  }
  if (key === "error.resource_limit") {
    return tFallback("gui.task.error.resource_limit");
  }
  return tFallback("gui.error.other.title");
}
