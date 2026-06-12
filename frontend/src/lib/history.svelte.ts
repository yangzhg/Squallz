export type OperationStatus = "queued" | "done" | "failed" | "info";

export interface OperationRecord {
  id: string;
  time: number;
  status: OperationStatus;
  title: string;
  detail: string;
}

const HISTORY_KEY = "squallz.operationHistory.v1";
const MAX_HISTORY = 80;

let records = $state<OperationRecord[]>(loadHistory());

function loadHistory(): OperationRecord[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(HISTORY_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as OperationRecord[];
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((item) => item && typeof item.title === "string" && typeof item.time === "number")
      .slice(0, MAX_HISTORY);
  } catch {
    return [];
  }
}

function persist() {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(HISTORY_KEY, JSON.stringify(records.slice(0, MAX_HISTORY)));
  } catch {
    // History is non-critical; quota/private-mode failures should not block jobs.
  }
}

export function operationHistory(): OperationRecord[] {
  return records;
}

export function recordOperation(input: Omit<OperationRecord, "id" | "time">) {
  records = [
    {
      ...input,
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      time: Date.now(),
    },
    ...records,
  ].slice(0, MAX_HISTORY);
  persist();
}

export function clearOperationHistory() {
  records = [];
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(HISTORY_KEY);
  } catch {
    // Best-effort clear for dev preview/private-mode.
  }
}
