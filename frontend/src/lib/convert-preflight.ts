import {
  ipc,
  isErrorDto,
  type CreatePlanDto,
  type DiskSpaceDto,
  type JobSpec,
} from "./ipc";

type ConvertJobSpec = Extract<JobSpec, { kind: "convert" }>;
type ConvertPreflightPhase = "measuring" | "checkingTemp" | "checkingDest";
type ConvertPreflightStage = "source" | "temp" | "destination";

export type ConvertPreflightOutcome =
  | {
      status: "ready";
      plan: CreatePlanDto;
    }
  | {
      status: "cancelled" | "stale";
    }
  | {
      status: "error";
      stage: ConvertPreflightStage;
      code:
        | "plan"
        | "workspace_service"
        | "workspace_space"
        | "system_temp_space"
        | "destination_service"
        | "destination_space";
      availableBytes?: number;
      error?: unknown;
    };

type ConvertPreflightOptions = Readonly<{
  spec: ConvertJobSpec;
  requestId: string;
  destinationDirectory: string;
  isCurrent: () => boolean;
  cancelRequested: () => boolean;
  onPlanRequestComplete: () => void;
  onPhase: (phase: ConvertPreflightPhase) => void;
  onPlan: (plan: CreatePlanDto) => void;
  onTempDisk: (disk: DiskSpaceDto) => void;
  onSystemTempDisk: (disk: DiskSpaceDto) => void;
  onDestinationDisk: (disk: DiskSpaceDto) => void;
}>;

export async function runConvertPreflight(
  options: ConvertPreflightOptions,
): Promise<ConvertPreflightOutcome> {
  options.onPhase("measuring");
  let plan: CreatePlanDto;
  let cancelled = false;
  try {
    plan = await ipc.planConvert(options.spec, options.requestId);
    cancelled = options.cancelRequested();
  } catch (error) {
    if (!options.isCurrent()) return { status: "stale" };
    if (isErrorDto(error) && error.key === "error.cancelled") {
      return { status: "cancelled" };
    }
    return { status: "error", stage: "source", code: "plan", error };
  } finally {
    options.onPlanRequestComplete();
  }
  if (!options.isCurrent()) return { status: "stale" };
  if (cancelled) return { status: "cancelled" };
  options.onPlan(plan);

  options.onPhase("checkingTemp");
  let tempDisk: DiskSpaceDto;
  try {
    tempDisk = await ipc.checkDiskSpace(
      options.destinationDirectory,
      plan.workspace_budget_bytes,
    );
  } catch {
    return options.isCurrent()
      ? { status: "error", stage: "temp", code: "workspace_service" }
      : { status: "stale" };
  }
  if (!options.isCurrent()) return { status: "stale" };
  options.onTempDisk(tempDisk);
  if (!tempDisk.ok) {
    return {
      status: "error",
      stage: "temp",
      code: "workspace_space",
      availableBytes: tempDisk.available_bytes,
    };
  }

  if (plan.system_temp_budget_bytes > 0) {
    let systemTempDisk: DiskSpaceDto;
    try {
      const systemTempDir = await ipc.tempDir();
      if (!options.isCurrent()) return { status: "stale" };
      systemTempDisk = await ipc.checkDiskSpace(
        systemTempDir,
        plan.system_temp_budget_bytes,
      );
    } catch {
      return options.isCurrent()
        ? { status: "error", stage: "temp", code: "workspace_service" }
        : { status: "stale" };
    }
    if (!options.isCurrent()) return { status: "stale" };
    options.onSystemTempDisk(systemTempDisk);
    if (!systemTempDisk.ok) {
      return {
        status: "error",
        stage: "temp",
        code: "system_temp_space",
        availableBytes: systemTempDisk.available_bytes,
      };
    }
  }

  options.onPhase("checkingDest");
  let destinationDisk: DiskSpaceDto;
  try {
    destinationDisk = await ipc.checkDiskSpace(
      options.destinationDirectory,
      plan.final_output_budget_bytes,
    );
  } catch {
    return options.isCurrent()
      ? { status: "error", stage: "destination", code: "destination_service" }
      : { status: "stale" };
  }
  if (!options.isCurrent()) return { status: "stale" };
  options.onDestinationDisk(destinationDisk);
  if (!destinationDisk.ok) {
    return {
      status: "error",
      stage: "destination",
      code: "destination_space",
      availableBytes: destinationDisk.available_bytes,
    };
  }
  return { status: "ready", plan };
}
