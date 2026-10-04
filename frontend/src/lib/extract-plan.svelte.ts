import { ipc, type ExtractPlanPreflightDto } from "./ipc";

export type ExtractPlanInput = Readonly<{
  archiveId: number;
  path: string;
  displayPath: string;
  dest: string;
  selection: string[] | null;
  smart: boolean;
  encoding: string | null;
}>;

type ExtractPlanPhase = "idle" | "loading" | "ready" | "blocked" | "error";
type PlanRequest = {
  input: ExtractPlanInput;
  key: string;
  generation: number;
  requestId: string;
  promise: Promise<void>;
  resolve: () => void;
  cancelRequested: boolean;
};
type PreviewPlan = (input: ExtractPlanInput) => ExtractPlanPreflightDto | null;
const debounceMs = 140;

function inputKey(input: ExtractPlanInput): string {
  return JSON.stringify([
    input.archiveId, input.path, input.displayPath, input.dest,
    input.selection, input.smart, input.encoding,
  ]);
}

/** Keeps one extraction read active until settlement and retains only the latest queued input. */
export class ExtractPlanSession {
  #plan = $state<ExtractPlanPreflightDto | null>(null);
  #phase = $state<ExtractPlanPhase>("idle");
  #errorKey = $state("");
  #key = "";
  #generation = 0;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #queued: PlanRequest | null = null;
  #active: PlanRequest | null = null;
  #disposed = false;

  constructor(
    private readonly nextRequestId: () => string,
    private readonly preview: PreviewPlan = () => null,
  ) {}

  get plan(): Readonly<ExtractPlanPreflightDto> | null { return this.#plan; }
  get phase() { return this.#phase; }
  get errorKey() { return this.#errorKey; }

  matches(input: ExtractPlanInput | null): boolean {
    return input !== null && this.#key === inputKey(input);
  }

  request(input: ExtractPlanInput, { debounce = false, force = false } = {}): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    const snapshot = { ...input, selection: input.selection === null ? null : [...input.selection] };
    const key = inputKey(snapshot);
    if (!force && key === this.#key) {
      if ((this.#phase === "ready" || this.#phase === "blocked") && this.#plan) return Promise.resolve();
      const active = this.#active;
      if (active?.key === key && active.generation === this.#generation) return active.promise;
      const queued = this.#queued;
      if (queued?.key === key && queued.generation === this.#generation) {
        if (!debounce && this.#timer !== null) {
          this.clearDebounce();
          this.startQueued();
        }
        return queued.promise;
      }
    }

    this.cancelActive();
    const generation = ++this.#generation;
    this.#key = key;
    this.#plan = null;
    this.#phase = "loading";
    this.#errorKey = "";
    let resolveRequest: () => void = () => undefined;
    const promise = new Promise<void>((resolve) => { resolveRequest = resolve; });
    const request: PlanRequest = {
      input: snapshot,
      key,
      generation,
      requestId: this.nextRequestId(),
      promise,
      resolve: resolveRequest,
      cancelRequested: false,
    };
    this.discardQueued();
    this.#queued = request;
    if (debounce) {
      this.#timer = setTimeout(() => {
        this.#timer = null;
        this.startQueued();
      }, debounceMs);
    } else {
      this.startQueued();
    }
    return promise;
  }

  reset(): void {
    this.cancelActive();
    this.#generation += 1;
    this.#key = "";
    this.discardQueued();
    this.#plan = null;
    this.#phase = "idle";
    this.#errorKey = "";
  }

  dispose(): void {
    this.#disposed = true;
    this.reset();
  }

  private clearDebounce(): void {
    if (this.#timer === null) return;
    clearTimeout(this.#timer);
    this.#timer = null;
  }

  private discardQueued(): void {
    this.clearDebounce();
    const queued = this.#queued;
    this.#queued = null;
    queued?.resolve();
  }

  private cancelActive(): void {
    const active = this.#active;
    if (!active || active.cancelRequested) return;
    active.cancelRequested = true;
    void ipc.cancelExtractPlan(active.requestId).catch(() => {
      // The read-only plan may already have completed.
    });
  }

  private startQueued(): void {
    if (this.#disposed || this.#active || this.#timer !== null || !this.#queued) return;
    const request = this.#queued;
    this.#queued = null;
    this.#active = request;
    void this.refresh(request).finally(() => {
      request.resolve();
      if (this.#active !== request) return;
      this.#active = null;
      this.startQueued();
    });
  }

  private isCurrent(request: PlanRequest): boolean {
    return request.generation === this.#generation && request.key === this.#key;
  }

  private async refresh(request: PlanRequest): Promise<void> {
    try {
      const { input } = request;
      const plan = this.preview(input) ?? await ipc.planExtract(
        input.path, input.displayPath, input.dest, input.selection, input.smart, input.encoding, request.requestId,
      );
      if (!this.isCurrent(request)) return;
      this.#plan = plan;
      this.#phase = plan.space_ok ? "ready" : "blocked";
    } catch {
      if (!this.isCurrent(request)) return;
      this.#plan = null;
      this.#phase = "error";
      this.#errorKey = "gui.extract.plan_unavailable_body";
    }
  }
}
