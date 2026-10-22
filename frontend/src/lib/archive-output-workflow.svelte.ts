import { runArchiveDiskPreflight, type ArchiveDiskPreflightOutcome } from "./archive-disk-preflight";
import { DestinationInspectionError, destinationInspectionCancelled, refreshArchiveOutputAuthorization } from "./archive-output-authorization";
import { ipc, isErrorDto, type CreatePlanDto, type DiskSpaceDto, type ErrorDto, type JobSpec } from "./ipc";

export type ArchiveOutputSpec = Extract<JobSpec, { kind: "compress" | "convert" }>;
export type ArchiveOutputStage = "source" | "temp" | "destination" | "submit";
export type ArchiveOutputPhase = "idle" | "selecting" | "choosingDest" | "measuring" | "checkingTemp"
  | "checkingDest" | "reviewing" | "submitting" | "ready" | "cancelled" | "blocked";
export type ArchiveOutputPending = { spec: ArchiveOutputSpec };
export type ArchiveOutputState<P> = {
  phase: ArchiveOutputPhase;
  plan: CreatePlanDto | null;
  destination: string | null;
  destinationDisk: DiskSpaceDto | null;
  workspaceDisk: DiskSpaceDto | null;
  systemTempDisk: DiskSpaceDto | null;
  pending: P | null;
  issue: string;
  issueStage: ArchiveOutputStage | null;
  requestId: string | null;
  requestKind: "source" | "plan" | "destination" | null;
  cancelPending: boolean;
  current: string;
  scanned: number;
  processedBytes: number;
};
export type ArchiveOutputPreflightOutcome =
  | { status: "ready"; plan: CreatePlanDto }
  | { status: "cancelled" }
  | { status: "stale" }
  | { status: "error"; stage: "source"; code: "plan" | "empty"; error?: unknown }
  | Extract<ArchiveDiskPreflightOutcome, { status: "error" }>;

type WorkflowServices = {
  kind: "create" | "convert";
  ensureListener: () => Promise<void>;
  nextRequestId: () => string;
  getDialogModule: () => Promise<typeof import("@tauri-apps/plugin-dialog")>;
  tr: (key: string) => string;
  notice: (message: string) => void;
  tError: (error: ErrorDto) => string;
};

function emptyState<P>(): ArchiveOutputState<P> {
  return {
    phase: "idle", plan: null, destination: null, destinationDisk: null,
    workspaceDisk: null, systemTempDisk: null, pending: null, issue: "", issueStage: null,
    requestId: null, requestKind: null, cancelPending: false, current: "", scanned: 0, processedBytes: 0,
  };
}

/** One instance owns every asynchronous preparation and submission for one output workflow. */
export class ArchiveOutputWorkflow<P extends ArchiveOutputPending> {
  state = $state<ArchiveOutputState<P>>(emptyState());
  generation = 0;
  private closed = false;
  private confirmation = 0;

  constructor(private readonly services: WorkflowServices) {}

  current(generation = this.generation): () => boolean {
    return () => !this.closed && generation === this.generation;
  }

  busy(): boolean {
    return ["selecting", "choosingDest", "measuring", "checkingTemp", "checkingDest", "submitting"].includes(this.state.phase);
  }

  reset(): P | null {
    const pending = this.state.pending;
    void this.cancel(false, true);
    this.generation += 1;
    this.state = emptyState();
    return pending;
  }

  begin(): () => boolean {
    if (this.closed) return () => false;
    this.reset();
    this.state.phase = "choosingDest";
    return this.current();
  }

  dispose(): void {
    this.reset();
    this.closed = true;
  }

  clearRequest(): void {
    Object.assign(this.state, { requestId: null, requestKind: null, cancelPending: false, current: "" });
  }

  issue(stage: ArchiveOutputStage, message: string, phase: "blocked" | "cancelled" | "reviewing" = "blocked", announce = true): void {
    this.clearRequest();
    Object.assign(this.state, { issueStage: stage, issue: message, phase, processedBytes: 0 });
    if (announce) this.services.notice(message);
  }

  review(pending: P): void {
    Object.assign(this.state, { pending, issue: "", issueStage: null, phase: "reviewing" });
  }

  private message(suffix: string): string {
    return this.services.tr(`gui.${this.services.kind}.${suffix}`);
  }

  async cancel(announce = true, keepIntentOnFailure = false): Promise<void> {
    const { requestId, requestKind, cancelPending } = this.state;
    if (!requestId || !requestKind || cancelPending) return;
    this.state.cancelPending = true;
    const prefix = this.services.kind === "create" ? "destination_check" : "preflight";
    try {
      if (requestKind === "destination") await ipc.cancelCreateDestinationInspection(requestId);
      else if (requestKind === "source") await ipc.cancelCreatePlan(requestId);
      else await ipc.cancelConvertPlan(requestId);
      if (announce && this.state.requestId === requestId && this.state.cancelPending) {
        this.services.notice(this.message(`${prefix}_cancel_requested`));
      }
    } catch {
      if (this.state.requestId !== requestId) return;
      if (!keepIntentOnFailure) this.state.cancelPending = false;
      if (announce) this.services.notice(this.message(`${prefix}_cancel_failed`));
    }
  }

  async inspect(path: string, split: boolean, sfxTarget: Extract<ArchiveOutputSpec, { kind: "compress" }>["sfx_target"], isCurrent = this.current()) {
    if (!isCurrent()) throw new DestinationInspectionError(undefined, true);
    await this.services.ensureListener();
    if (!isCurrent()) throw new DestinationInspectionError(undefined, true);
    const requestId = this.services.nextRequestId();
    Object.assign(this.state, { requestId, requestKind: "destination", processedBytes: 0, cancelPending: false, current: "" });
    try {
      const inspection = await ipc.inspectCreateDestination(path, split, requestId, sfxTarget);
      if (!isCurrent() || this.state.cancelPending) throw new DestinationInspectionError(undefined, true);
      return inspection;
    } catch (error) {
      const cancelled = this.state.requestId === requestId && this.state.cancelPending;
      if (error instanceof DestinationInspectionError) {
        if (!cancelled || error.cancelled) throw error;
        throw new DestinationInspectionError(error.detail ?? undefined, true);
      }
      throw new DestinationInspectionError(error, cancelled);
    } finally {
      if (this.state.requestId === requestId) this.clearRequest();
    }
  }

  async prepare(spec: P["spec"], destinationDirectory: string, isCurrent = this.current()): Promise<ArchiveOutputPreflightOutcome> {
    if (!isCurrent()) return { status: "stale" };
    this.state.phase = "measuring";
    let requestId: string | null = null;
    let plan: CreatePlanDto;
    try {
      await this.services.ensureListener();
      if (!isCurrent()) return { status: "stale" };
      requestId = this.services.nextRequestId();
      Object.assign(this.state, { requestId, requestKind: spec.kind === "compress" ? "source" : "plan", processedBytes: 0, cancelPending: false });
      plan = await (spec.kind === "compress" ? ipc.planCreate(spec, requestId) : ipc.planConvert(spec, requestId));
      if (!isCurrent()) return { status: "stale" };
      if (this.state.cancelPending) return { status: "cancelled" };
    } catch (error) {
      if (!isCurrent()) return { status: "stale" };
      return isErrorDto(error) && error.key === "error.cancelled"
        ? { status: "cancelled" } : { status: "error", stage: "source", code: "plan", error };
    } finally {
      if (requestId !== null && this.state.requestId === requestId) this.clearRequest();
    }
    this.state.plan = plan;
    if (spec.kind === "compress" && plan.entries === 0) return { status: "error", stage: "source", code: "empty" };
    this.state.scanned = plan.entries + plan.deduplicated_entries;
    const outcome = await runArchiveDiskPreflight(plan, {
      destinationDirectory, isCurrent,
      onPhase: (phase) => (this.state.phase = phase),
      onTempDisk: (disk) => (this.state.workspaceDisk = disk),
      onSystemTempDisk: (disk) => (this.state.systemTempDisk = disk),
      onDestinationDisk: (disk) => (this.state.destinationDisk = disk),
    });
    if (!isCurrent()) return { status: "stale" };
    return outcome.status === "ready" ? { status: "ready", plan } : outcome;
  }

  async confirm(options: {
    recheck: boolean;
    prepare: (isCurrent: () => boolean) => void;
    submit: (spec: P["spec"]) => Promise<number>;
    blockedMessage: (error: unknown) => string | null;
    queued: (pending: P, plan: CreatePlanDto, jobId: number, isCurrent: () => boolean) => void | Promise<void>;
    focusReview: (isCurrent: () => boolean) => void | Promise<void>;
  }): Promise<void> {
    if (this.closed || this.busy()) return;
    const { pending, plan } = this.state;
    if (!pending || !plan) return;
    const current = this.current();
    const confirmation = ++this.confirmation;
    const isCurrent = () => current() && confirmation === this.confirmation;
    Object.assign(this.state, { issue: "", issueStage: null, phase: "submitting" });
    options.prepare(isCurrent);
    let spec = pending.spec;
    try {
      if (options.recheck) {
        const refreshed = await refreshArchiveOutputAuthorization(spec, {
          path: spec.dest,
          inspect: () => this.inspect(spec.dest, spec.split_size !== null, spec.kind === "compress" ? spec.sfx_target : null, isCurrent),
          isCurrent, confirm: async () => (await this.services.getDialogModule()).confirm,
          tr: this.services.tr, message: `gui.${this.services.kind}.replace_changed`,
        });
        if (!isCurrent()) return;
        if (!refreshed) {
          this.state.phase = "reviewing";
          this.services.notice(this.message("replace_changed.kept"));
          return;
        }
        spec = refreshed;
        if (spec !== pending.spec) this.state.pending = { ...pending, spec };
      }
    } catch (error) {
      if (!isCurrent()) return;
      if (destinationInspectionCancelled(error)) {
        this.issue("destination", this.message("destination_recheck_cancelled"), "reviewing");
        await options.focusReview(isCurrent);
      } else {
        this.issue("destination", error instanceof DestinationInspectionError && error.detail
          ? this.services.tError(error.detail) : this.message("destination_recheck_failed"));
      }
      return;
    }
    let jobId: number;
    try {
      if (!isCurrent()) return;
      jobId = await options.submit(spec);
      if (!isCurrent()) return;
    } catch (error) {
      if (!isCurrent()) return;
      const blocked = options.blockedMessage(error);
      this.issue("submit", blocked ?? this.message("submission_requires_desktop_service"), "blocked",
        blocked === null || this.services.kind === "convert");
      return;
    }
    this.state.pending = null;
    this.state.phase = "ready";
    await options.queued(pending, plan, jobId, isCurrent);
  }
}
