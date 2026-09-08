import type { StateEvent } from "./ipc";

export type SnapshotState = StateEvent["state"];

export function isTerminalSnapshotState(state: SnapshotState): boolean {
  return state === "done" || state === "failed" || state === "cancelled";
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
