export function archiveKeyboardTarget(key: string, index: number, total: number, pageRows: number): number | null {
  if (!Number.isInteger(index) || index < 0 || index >= total) return null;
  const page = Math.max(1, Math.floor(pageRows));
  let target: number;
  switch (key) {
    case "ArrowUp": target = index - 1; break;
    case "ArrowDown": target = index + 1; break;
    case "Home": target = 0; break;
    case "End": target = total - 1; break;
    case "PageUp": target = index - page; break;
    case "PageDown": target = index + page; break;
    default: return null;
  }
  return Math.max(0, Math.min(total - 1, target));
}
