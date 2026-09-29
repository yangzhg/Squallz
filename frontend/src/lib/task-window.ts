import {
  externalOpenAction,
  externalOpenActionCopy,
  isExternalOpenAction,
  type ExternalOpenAction,
} from "./external-tasks";
import type { JobSpec } from "./ipc";
import type { JobSnapshotStatus, Task } from "./jobs.svelte";

export const taskWindowQuery = {
  mode: "taskWindow",
  modeValue: "1",
  action: "externalTask",
  path: "externalPath",
  output: "externalOutput",
} as const;

export interface TaskWindowLaunchRequest {
  action: ExternalOpenAction;
  paths: string[];
  output: string | null;
}

export interface TaskWindowLaunchState {
  mode: boolean;
  pendingAction: ExternalOpenAction | null;
  launch: TaskWindowLaunchRequest | null;
  status: TaskWindowShellStatus;
}

export type TaskWindowTranslate = (key: string, fallback: string) => string;
export type TaskWindowShellStatus =
  | "waiting"
  | "starting"
  | "started"
  | "no-selection"
  | "preset-error"
  | "requires-desktop-service"
  | "reconnecting"
  | "reconnect-error"
  | "task-unavailable"
  | "busy";

export function taskWindowTask<T extends Pick<Task, "id" | "state" | "ownedByRequester">>(
  tasks: T[],
  rememberedId: number | null,
): T | null {
  if (rememberedId !== null) {
    return tasks.find((task) => task.id === rememberedId && task.ownedByRequester) ?? null;
  }
  let latest: T | null = null;
  for (const task of tasks) {
    if (task.ownedByRequester && (!latest || task.id > latest.id)) latest = task;
  }
  return latest;
}

export function taskWindowRecoveryState(connection: JobSnapshotStatus): TaskWindowLaunchState {
  return taskWindowShellStatusState(null, connection === "loading" ? "reconnecting"
    : connection === "ready" ? "task-unavailable" : "reconnect-error");
}

export interface TaskWindowSubmitTransition {
  state: TaskWindowLaunchState;
  notice: string | null;
}

export interface TaskWindowSubmitPlan {
  starting: TaskWindowSubmitTransition;
  jobSpec: JobSpec | null;
  noSelection: TaskWindowSubmitTransition;
}

function taskWindowActionFromParams(params: URLSearchParams): ExternalOpenAction | null {
  return externalOpenAction(params.get(taskWindowQuery.action));
}

function taskWindowPathsFromParams(params: URLSearchParams): string[] {
  return params.getAll(taskWindowQuery.path).filter((path) => path.length > 0);
}

function taskWindowLaunchRequest(
  action: ExternalOpenAction | null,
  params: URLSearchParams,
): TaskWindowLaunchRequest | null {
  const paths = taskWindowPathsFromParams(params);
  if (!action || paths.length === 0) return null;
  return {
    action,
    paths,
    output: params.get(taskWindowQuery.output),
  };
}

export function taskWindowModeFromParams(params: URLSearchParams): boolean {
  return (
    params.get(taskWindowQuery.mode) === taskWindowQuery.modeValue ||
    isExternalOpenAction(params.get(taskWindowQuery.action))
  );
}

export function taskWindowPendingActionFromParams(params: URLSearchParams): ExternalOpenAction | null {
  return taskWindowActionFromParams(params);
}

export function taskWindowLaunchRequestFromParams(params: URLSearchParams): TaskWindowLaunchRequest | null {
  return taskWindowLaunchRequest(taskWindowActionFromParams(params), params);
}

export function taskWindowLaunchStateFromParams(params: URLSearchParams): TaskWindowLaunchState {
  const pendingAction = taskWindowActionFromParams(params);
  const launch = taskWindowLaunchRequest(pendingAction, params);
  return {
    mode: params.get(taskWindowQuery.mode) === taskWindowQuery.modeValue || pendingAction !== null,
    pendingAction,
    launch,
    status: launch ? "starting" : "waiting",
  };
}

export function taskWindowShellStatusState(
  action: ExternalOpenAction | null,
  status: TaskWindowShellStatus,
): TaskWindowLaunchState {
  return {
    mode: true,
    pendingAction: action,
    launch: null,
    status,
  };
}

export function activeTaskWindowLaunchState(action: ExternalOpenAction): TaskWindowLaunchState {
  return taskWindowShellStatusState(action, "starting");
}

export function taskWindowActionLabel(action: ExternalOpenAction, translate: TaskWindowTranslate): string {
  const copy = externalOpenActionCopy(action);
  return translate(copy.labelKey, copy.fallbackLabel);
}

export function taskWindowWaitingMessage(
  action: ExternalOpenAction | null,
  translate: TaskWindowTranslate,
): string {
  if (!action) {
    return translate("gui.external_task.waiting_body", "Tasks started from Finder or a file manager will appear here.");
  }
  return translate("gui.external_task.waiting_action", "Waiting for {action} to start")
    .replace("{action}", taskWindowActionLabel(action, translate));
}

export function taskWindowShellTitle(
  state: TaskWindowLaunchState,
  translate: TaskWindowTranslate,
): string {
  switch (state.status) {
    case "reconnecting":
      return translate("gui.external_task.title_reconnecting", "Reconnecting to your task");
    case "reconnect-error":
      return translate("gui.external_task.title_reconnect_error", "Task connection unavailable");
    case "task-unavailable":
      return translate("gui.external_task.title_task_unavailable", "Task unavailable");
    case "starting":
      return taskWindowActionStatusMessage(
        "gui.external_task.title_starting_action",
        "Starting {action}",
        state.pendingAction,
        translate,
      );
    case "started":
      return translate("gui.external_task.title_started", "Task started");
    case "no-selection":
      return translate("gui.external_task.title_no_selection", "No file selected");
    case "preset-error":
      return translate("gui.external_task.title_preset_error", "Preset needs attention");
    case "requires-desktop-service":
      return translate("gui.external_task.title_requires_desktop_service", "Desktop service unavailable");
    case "busy":
      return translate("gui.external_task.title_busy", "Task already running");
    case "waiting":
      if (state.pendingAction) {
        return taskWindowActionStatusMessage(
          "gui.external_task.title_waiting_action",
          "Waiting for {action}",
          state.pendingAction,
          translate,
        );
      }
      return translate("gui.external_task.title_waiting", "Ready for a task");
  }
}

function taskWindowActionStatusMessage(
  key: string,
  fallback: string,
  action: ExternalOpenAction | null,
  translate: TaskWindowTranslate,
): string {
  if (!action) return translate(key, fallback);
  return translate(key, fallback).replace("{action}", taskWindowActionLabel(action, translate));
}

export function taskWindowShellMessage(
  state: TaskWindowLaunchState,
  translate: TaskWindowTranslate,
): string {
  switch (state.status) {
    case "reconnecting":
      return translate("gui.external_task.reconnecting", "Loading this window's task and its latest result.");
    case "reconnect-error":
      return translate("gui.external_task.reconnect_error", "The task status could not be loaded. Retry to reconnect to the existing task.");
    case "task-unavailable":
      return translate("gui.external_task.task_unavailable", "No task is available for this window. If it never started, launch it again from your file manager; otherwise check the output folder.");
    case "starting":
      return taskWindowActionStatusMessage(
        "gui.external_task.starting_action",
        "Starting {action}",
        state.pendingAction,
        translate,
      );
    case "started":
      return translate("gui.external_task.started", "Task window started");
    case "no-selection":
      return translate("gui.external_task.no_selection", "No file was provided for this action");
    case "preset-error":
      return translate(
        "gui.external_task.preset_error",
        "The file-manager preset could not be used. Open Squallz, review it, then try again.",
      );
    case "requires-desktop-service":
      return translate("gui.external_task.requires_desktop_service", "This action requires the desktop service");
    case "busy":
      return translate(
        "gui.task.one_at_a_time_notice",
        "Finish or cancel the current task before starting another one",
      );
    case "waiting":
      return taskWindowWaitingMessage(state.pendingAction, translate);
  }
}

export function taskWindowSubmitNotice(
  status: TaskWindowShellStatus,
  translate: TaskWindowTranslate,
): string | null {
  switch (status) {
    case "started":
      return translate("gui.external_task.started", "Task window started");
    case "no-selection":
      return translate("gui.external_task.no_selection", "No file was provided for this action");
    case "requires-desktop-service":
      return translate("gui.external_task.requires_desktop_service", "This action requires the desktop service");
    case "preset-error":
      return null;
    case "waiting":
    case "starting":
    case "busy":
    case "reconnecting":
    case "reconnect-error":
    case "task-unavailable":
      return null;
  }
}

export function taskWindowSubmitTransition(
  action: ExternalOpenAction | null,
  status: TaskWindowShellStatus,
  translate: TaskWindowTranslate,
): TaskWindowSubmitTransition {
  return {
    state: taskWindowShellStatusState(action, status),
    notice: taskWindowSubmitNotice(status, translate),
  };
}

export function taskWindowSubmitPlan(
  action: ExternalOpenAction,
  jobSpec: JobSpec | null,
  translate: TaskWindowTranslate,
): TaskWindowSubmitPlan {
  return {
    starting: taskWindowSubmitTransition(action, "starting", translate),
    jobSpec,
    noSelection: taskWindowSubmitTransition(action, "no-selection", translate),
  };
}

export function taskWindowSubmitFailureStatus(blockedByActiveTask: boolean): TaskWindowShellStatus {
  return blockedByActiveTask ? "busy" : "requires-desktop-service";
}
