import {
  externalOpenAction,
  externalOpenActionCopy,
  isExternalOpenAction,
  type ExternalOpenAction,
} from "./external-tasks";
import type { JobSpec } from "./ipc";

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
  | "requires-desktop-service"
  | "busy";

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
    return translate("gui.external_task.waiting_body", "The selected action will open here.");
  }
  return translate("gui.external_task.waiting_action", "Waiting for {action} to start")
    .replace("{action}", taskWindowActionLabel(action, translate));
}

export function taskWindowShellTitle(
  state: TaskWindowLaunchState,
  translate: TaskWindowTranslate,
): string {
  switch (state.status) {
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
      return translate("gui.external_task.title_waiting", "Task window ready");
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
    case "waiting":
    case "starting":
    case "busy":
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
