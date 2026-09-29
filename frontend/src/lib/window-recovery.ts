import { taskWindowModeFromParams, taskWindowQuery } from "./task-window";

export interface WindowRecoveryContext {
  taskId?: number | null;
  taskCenter?: boolean;
  taskWindow?: boolean;
}

const taskIdParam = "recoveryTask";
const taskCenterParam = "recoveryTaskCenter";

export function readWindowRecovery(params: URLSearchParams): WindowRecoveryContext {
  const id = Number(params.get(taskIdParam));
  return {
    taskId: Number.isSafeInteger(id) && id > 0 ? id : null,
    taskCenter: params.get(taskCenterParam) === "1",
  };
}

export function windowRecoveryUrl(currentUrl: string, context: WindowRecoveryContext): string {
  const url = new URL(currentUrl);
  const params = url.searchParams;
  if (context.taskWindow || taskWindowModeFromParams(params)) params.set(taskWindowQuery.mode, taskWindowQuery.modeValue);
  // External task parameters are a launch request, never a route to replay.
  params.delete(taskWindowQuery.action);
  params.delete(taskWindowQuery.path);
  params.delete(taskWindowQuery.output);
  params.delete(taskIdParam);
  params.delete(taskCenterParam);
  if (context.taskId && Number.isSafeInteger(context.taskId) && context.taskId > 0) {
    params.set(taskIdParam, String(context.taskId));
  }
  if (context.taskCenter) params.set(taskCenterParam, "1");
  return url.href;
}

export function rememberWindowRecovery(context: WindowRecoveryContext): void {
  window.history.replaceState(window.history.state, "", windowRecoveryUrl(window.location.href, context));
}

export function reloadWindow(context: WindowRecoveryContext): void {
  rememberWindowRecovery(context);
  window.location.reload();
}
