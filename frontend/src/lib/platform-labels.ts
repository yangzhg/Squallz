import { tFallback } from "./i18n.svelte";
import {
  targetDesktopPlatform,
  type DesktopPathPlatform,
} from "./desktop-path";

export function platformTrashName(
  platform: DesktopPathPlatform = targetDesktopPlatform(),
): string {
  return platform === "windows"
    ? tFallback("gui.platform.trash.windows")
    : tFallback("gui.platform.trash.default");
}

export function platformFileManagerName(
  platform: DesktopPathPlatform = targetDesktopPlatform(),
): string {
  return tFallback(`gui.platform.file_manager.${platform}`);
}
