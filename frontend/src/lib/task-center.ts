import type { Task } from "./jobs.svelte";
import { taskOutcomeNeedsAttention } from "./task-model";

export interface TaskCenterCounts {
  active: number;
  waiting: number;
  attention: number;
  completed: number;
  clearable: number;
  total: number;
}

export interface TaskQueuePosition {
  position: number;
  aheadInQueue: number;
  canMoveEarlier: boolean;
  canMoveLater: boolean;
}

export type TaskQueueDropEdge = "before" | "after";

export type TaskSubmissionBlockReason =
  | "starting"
  | "task-window-busy"
  | "replace-existing";

export interface TaskSubmissionContext {
  submitInFlight: boolean;
  taskWindowMode: boolean;
  hasActiveTask: boolean;
  replacesExistingOutput: boolean;
}

function isActive(task: Task): boolean {
  return task.state === "running" || task.state === "paused";
}

function isTerminal(task: Task): boolean {
  return task.state === "done" || task.state === "failed" || task.state === "cancelled";
}

function needsAttention(task: Task): boolean {
  return task.interaction !== null || task.state === "failed" || taskOutcomeNeedsAttention(task);
}

function rowPriority(task: Task): number {
  if (task.interaction !== null) return 0;
  if (needsAttention(task)) return 1;
  if (task.state === "running") return 2;
  if (task.state === "paused") return 3;
  if (task.state === "queued") return 4;
  if (task.state === "done") return 5;
  return 6;
}

function reorderableQueuedTasks(tasks: readonly Task[]): Task[] {
  return tasks
    .map((task, index) => ({ task, index }))
    .filter(({ task }) => task.state === "queued" && task.queuePosition !== null)
    .sort((left, right) => {
      const leftPosition = left.task.queuePosition ?? Number.MAX_SAFE_INTEGER;
      const rightPosition = right.task.queuePosition ?? Number.MAX_SAFE_INTEGER;
      return leftPosition - rightPosition || left.task.id - right.task.id || left.index - right.index;
    })
    .map(({ task }) => task);
}

/** Stable task-center order: attention, active work, backend queue order, then newest completed work. */
export function taskCenterRows(tasks: readonly Task[]): Task[] {
  return tasks
    .map((task, index) => ({ task, index }))
    .sort((left, right) => {
      const priority = rowPriority(left.task) - rowPriority(right.task);
      if (priority !== 0) return priority;
      if (left.task.state === "queued") {
        const leftPosition = left.task.queuePosition ?? Number.MAX_SAFE_INTEGER;
        const rightPosition = right.task.queuePosition ?? Number.MAX_SAFE_INTEGER;
        return leftPosition - rightPosition || left.task.id - right.task.id || left.index - right.index;
      }
      if (isTerminal(left.task)) {
        return right.task.id - left.task.id || right.index - left.index;
      }
      return left.index - right.index;
    })
    .map(({ task }) => task);
}

export function taskCenterCounts(tasks: readonly Task[]): TaskCenterCounts {
  let active = 0;
  let waiting = 0;
  let attention = 0;
  let completed = 0;
  let clearable = 0;

  for (const task of tasks) {
    if (isActive(task)) active += 1;
    if (task.state === "queued") waiting += 1;
    if (needsAttention(task)) attention += 1;
    if (isTerminal(task)) completed += 1;
    if (isTerminal(task) && !needsAttention(task)) clearable += 1;
  }

  return { active, waiting, attention, completed, clearable, total: tasks.length };
}

export function taskCenterActionableCount(tasks: readonly Task[]): number {
  return tasks.filter((task) => isActive(task) || task.state === "queued" || needsAttention(task)).length;
}

/** Position in the shared FIFO queue across app and file-manager entry points. */
export function taskQueuePosition(
  tasks: readonly Task[],
  taskId: number,
): TaskQueuePosition | null {
  const waiting = reorderableQueuedTasks(tasks);
  const index = waiting.findIndex((task) => task.id === taskId);
  if (index < 0) return null;
  return {
    position: index + 1,
    aheadInQueue: tasks.filter(isActive).length + index,
    canMoveEarlier: index > 0,
    canMoveLater: index < waiting.length - 1,
  };
}

/**
 * Resolves a visual before/after drop into the backend's move-before contract.
 * `null` means queue tail; `undefined` means invalid or already in that slot.
 */
export function taskQueueDropBeforeId(
  tasks: readonly Task[],
  sourceId: number,
  targetId: number,
  edge: TaskQueueDropEdge,
): number | null | undefined {
  if (sourceId === targetId) return undefined;
  const original = reorderableQueuedTasks(tasks).map((task) => task.id);
  const sourceIndex = original.indexOf(sourceId);
  if (sourceIndex < 0) return undefined;

  const reordered = original.filter((id) => id !== sourceId);
  const targetIndex = reordered.indexOf(targetId);
  if (targetIndex < 0) return undefined;
  const insertionIndex = targetIndex + (edge === "after" ? 1 : 0);
  reordered.splice(insertionIndex, 0, sourceId);
  if (reordered.every((id, index) => id === original[index])) return undefined;
  return reordered[insertionIndex + 1] ?? null;
}

export function clearableTaskIds(tasks: readonly Task[]): number[] {
  return tasks
    .filter((task) => isTerminal(task) && !needsAttention(task))
    .map((task) => task.id);
}

export function taskSubmissionBlockReason({
  submitInFlight,
  taskWindowMode,
  hasActiveTask,
  replacesExistingOutput,
}: TaskSubmissionContext): TaskSubmissionBlockReason | null {
  if (submitInFlight) return "starting";
  if (taskWindowMode && hasActiveTask) return "task-window-busy";
  if (!taskWindowMode && hasActiveTask && replacesExistingOutput) {
    return "replace-existing";
  }
  return null;
}
