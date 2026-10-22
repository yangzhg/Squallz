import { ipc, type CreatePlanDto, type DiskSpaceDto } from "./ipc";

export type ArchiveDiskPreflightOutcome =
  | { status: "ready" }
  | { status: "stale" }
  | {
      status: "error";
      stage: "temp" | "destination";
      code:
        | "workspace_service"
        | "workspace_space"
        | "system_temp_space"
        | "destination_service"
        | "destination_space";
      availableBytes?: number;
    };

export type ArchiveDiskPreflightOptions = Readonly<{
  destinationDirectory: string;
  isCurrent: () => boolean;
  onPhase: (phase: "checkingTemp" | "checkingDest") => void;
  onTempDisk: (disk: DiskSpaceDto) => void;
  onSystemTempDisk: (disk: DiskSpaceDto) => void;
  onDestinationDisk: (disk: DiskSpaceDto) => void;
}>;

export async function runArchiveDiskPreflight(
  plan: CreatePlanDto,
  options: ArchiveDiskPreflightOptions,
): Promise<ArchiveDiskPreflightOutcome> {
  const checks = [
    {
      kind: "workspace",
      budget: plan.workspace_budget_bytes,
      code: "workspace_space",
      publish: options.onTempDisk,
    },
    {
      kind: "systemTemp",
      budget: plan.system_temp_budget_bytes,
      code: "system_temp_space",
      publish: options.onSystemTempDisk,
    },
    {
      kind: "destination",
      budget: plan.final_output_budget_bytes,
      code: "destination_space",
      publish: options.onDestinationDisk,
    },
  ] as const;
  for (const check of checks) {
    if (!options.isCurrent()) return { status: "stale" };
    if (check.kind === "systemTemp" && check.budget <= 0) continue;
    if (check.kind !== "systemTemp") {
      options.onPhase(check.kind === "destination" ? "checkingDest" : "checkingTemp");
    }
    const stage = check.kind === "destination" ? "destination" : "temp";
    let disk: DiskSpaceDto;
    try {
      const directory = check.kind === "systemTemp"
        ? await ipc.tempDir()
        : options.destinationDirectory;
      if (!options.isCurrent()) return { status: "stale" };
      disk = await ipc.checkDiskSpace(directory, check.budget);
    } catch {
      if (!options.isCurrent()) return { status: "stale" };
      return {
        status: "error",
        stage,
        code: check.kind === "destination" ? "destination_service" : "workspace_service",
      };
    }
    if (!options.isCurrent()) return { status: "stale" };
    check.publish(disk);
    if (!disk.ok) {
      return {
        status: "error",
        stage,
        code: check.code,
        availableBytes: disk.available_bytes,
      };
    }
  }
  return { status: "ready" };
}
