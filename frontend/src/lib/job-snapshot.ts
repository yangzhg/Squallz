import type { JobQuestion, JobSnapshot, JobSpec, StateEvent } from "./ipc";

export type SnapshotState = StateEvent["state"];

/** Capture output protection before credentials are removed from a task. */
export function outputPasswordRequired(spec: JobSpec): boolean {
  if (spec.kind === "compress") return spec.password !== null || spec.encrypt_names;
  if (spec.kind === "convert") return spec.dest_password !== null || spec.encrypt_names;
  if (spec.kind === "export_sqz") return spec.dest_password !== null;
  return false;
}

export function isTerminalSnapshotState(state: SnapshotState): boolean {
  return state === "done" || state === "failed" || state === "cancelled";
}

export function snapshotQuestion(
  snapshot: Pick<JobSnapshot, "owned_by_requester" | "state" | "question">,
  answeredVersion: number,
  current: JobQuestion | null = null,
): JobQuestion | null {
  const question = snapshot.question;
  if (!snapshot.owned_by_requester || isTerminalSnapshotState(snapshot.state)
    || !question || question.prompt.version <= answeredVersion) return null;
  // Progress and queue updates must not reset the active form or its focus.
  return current?.prompt.version === question.prompt.version ? current : question;
}

export function shouldApplySnapshotState(
  currentVersion: number,
  currentState: SnapshotState,
  incomingVersion: number,
  incomingState: SnapshotState,
): boolean {
  if (incomingVersion <= currentVersion) return false;
  return !isTerminalSnapshotState(currentState) || isTerminalSnapshotState(incomingState);
}

export function shouldApplySnapshotProgress(
  currentVersion: number,
  currentState: SnapshotState,
  incomingVersion: number,
): boolean {
  return incomingVersion > currentVersion && !isTerminalSnapshotState(currentState);
}

export function shouldApplyFullSnapshot(
  currentVersion: number,
  currentState: SnapshotState,
  snapshotVersion: number,
  snapshotState: SnapshotState,
): boolean {
  if (snapshotVersion < currentVersion) return false;
  return !isTerminalSnapshotState(currentState) || isTerminalSnapshotState(snapshotState);
}

export function localSubmissionPromotion(
  state: SnapshotState,
  alreadyLocal: boolean,
): { resetHistory: boolean; replayTerminal: boolean } {
  return {
    resetHistory: !alreadyLocal,
    replayTerminal: !alreadyLocal && isTerminalSnapshotState(state),
  };
}
