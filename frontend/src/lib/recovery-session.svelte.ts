import { recoveryReport } from "./recovery-result";
import { ipc, type JobSpec } from "./ipc";
import type { Task } from "./jobs.svelte";
import type { TaskDialogModel } from "./task-model";
import { taskOutcomeStateLabel, taskStateLabel } from "./task-model";
import { tError, tFallback as tr } from "./i18n.svelte";
import { recordOperation } from "./history.svelte";
import { basename as pathBaseName, dirname as pathDir } from "./format";
import { joinDesktopPath, sameDesktopPath } from "./desktop-path";
import { DestinationInspectionError, destinationInspectionCancelled, authorizeArchiveOutput, type ArchiveOutputAuthorization } from "./archive-output-authorization";
import {
  latestMatchingRecoveryTask, recoveryRouteForOpen, recoveryRepairGate,
  recoveryResultBoolean as recoveryResultMetricBoolean, recoveryResultConfirmsRepairCapacity,
  recoveryResultHasNoDamage, recoveryResultMetrics, recoveryResultNumber as recoveryResultMetricNumber,
  recoveryResultOk, recoveryResultOperation, recoveryResultTone as recoveryResultToneFor,
  type RecoveryRouteState,
} from "./recovery-result";
import type { RecoveryWorkspaceActions, RecoveryWorkspaceView } from "./recovery-workspace";
import type { RecoveryOutputKind, RecoverySessionPorts } from "./recovery-session";

type OutputPreparation = {
  id: symbol;
  context: string;
  phase: "opening" | "choosing" | "checking" | "submitting";
};

export class RecoverySession {
  private sourceMode = $state<RecoveryRouteState["sourceMode"]>("none");
  private sourceOverride = $state<string | null>(null);
  private sidecarOverride = $state<string | null>(null);
  private redundancyDraft = $state("10");
  private pickerStatus = $state<"idle" | "archive" | "par2">("idle");
  private pickerRequest = 0;
  private outputPreparation = $state<OutputPreparation | null>(null);
  private inspection: { id: string } | null = null;
  private submission = $state.raw<symbol | null>(null);
  private contextTaskIds = new Set<number>();
  private disposed = false;

  constructor(private readonly ports: RecoverySessionPorts, current = false) {
    this.sourceMode = current ? "current" : "none";
  }

  get preparation(): Readonly<OutputPreparation> | null {
    return this.outputPreparation;
  }

  get picker(): "idle" | "archive" | "par2" {
    return this.pickerStatus;
  }

  private get submissionPending(): boolean {
    return this.submission !== null;
  }

  leave(): void {
    this.dismissOutput();
    this.submission = null;
    this.pickerRequest += 1;
    this.pickerStatus = "idle";
  }

  dispose(): void {
    this.disposed = true;
    this.leave();
  }

  syncContext(): void {
    const request = this.outputPreparation;
    if (request && !this.isCurrentOutputPreparation(request)) this.dismissOutput();
  }

  private dismissOutput(): void {
    this.outputPreparation = null;
    const inspection = this.inspection;
    this.inspection = null;
    if (inspection) void ipc.cancelCreateDestinationInspection(inspection.id).catch(() => {});
  }

  open(source: "preserve" | "current" = "preserve"): void {
    const route = recoveryRouteForOpen(source, Boolean(this.ports.read().archive), {
      sourceMode: this.sourceMode, sourceOverride: this.sourceOverride, par2Override: this.sidecarOverride,
    });
    if (route.sourceMode !== this.sourceMode || route.sourceOverride !== this.sourceOverride
      || route.par2Override !== this.sidecarOverride) this.leave();
    this.sourceMode = route.sourceMode;
    this.sourceOverride = route.sourceOverride;
    this.sidecarOverride = route.par2Override;
    this.ports.notice(this.sourcePath()
      ? tr("gui.recovery.route_for_archive")
          .replace("{name}", this.sourceName() ?? tr("gui.archive.generic"))
      : tr("gui.recovery.choose_archive_to_begin"));
  }

  selectSource(path: string | null, sidecar: string | null = null): void {
    this.leave();
    this.sourceMode = path ? "selected" : "none";
    this.sourceOverride = path;
    this.sidecarOverride = sidecar;
  }

  selectSet(sidecar: string, archivePath: string | null, sidecarCount = 1, sidecarSetCount = 1): void {
    this.selectSource(archivePath, sidecar);
    let notice: string;
    if (sidecarSetCount > 1) {
      notice = tr("gui.recovery.multiple_sidecar_sets_choose")
        .replace("{name}", pathBaseName(sidecar)).replace("{count}", sidecarSetCount.toLocaleString());
    } else if (sidecarCount > 1) {
      notice = tr(archivePath ? "gui.recovery.sidecar_set_loaded" : "gui.recovery.sidecar_set_loaded_choose_archive",
        archivePath ? "{name} selected from {count} PAR2 files. Other recovery volumes will be found beside it."
          : "{name} selected from {count} PAR2 files. Choose the matching archive.")
        .replace("{name}", pathBaseName(sidecar)).replace("{count}", sidecarCount.toLocaleString());
    } else if (archivePath) {
      notice = tr("gui.recovery.set_loaded").replace("{name}", pathBaseName(archivePath));
    } else {
      notice = tr("gui.recovery.par2_loaded_choose_archive")
        .replace("{name}", pathBaseName(sidecar));
    }
    this.ports.notice(notice);
  }

  adoptArchive(): void {
    this.leave();
    this.sourceMode = "current";
    this.sourceOverride = null;
    this.sidecarOverride = null;
  }

  private sameFilePath(left: string, right: string): boolean {
    return sameDesktopPath(left, right, this.ports.read().platform);
  }

  private joinFolderPath(folder: string, name: string): string {
    return joinDesktopPath(folder, name, this.ports.read().platform);
  }

  private openArchiveFirstLabel(): string {
    return tr("gui.empty.open_archive_first");
  }

  readonly actions: RecoveryWorkspaceActions = {
    chooseArchive: () => void this.chooseArchive(),
    choosePar2: () => void this.choosePar2(),
    useCurrentArchive: () => this.useCurrentArchive(),
    useDefaultPar2: () => this.useDefaultPar2(),
    testArchive: () => void this.testArchive(),
    setRedundancy: (value) => this.setRedundancy(value),
    protect: () => void this.protect(),
    verify: () => void this.verify(),
    repair: () => void this.submitOutput("repair_recovery"),
    repairZip: () => void this.submitOutput("repair_zip"),
    repairSqz: () => void this.submitOutput("repair_sqz"),
    exportSqz: () => void this.submitOutput("export_sqz"),
    extractReadable: () => void this.extractReadable(),
  };

  private defaultSqzExportDest(): string {
    const source = this.sourcePath();
    if (!source) return this.openArchiveFirstLabel();
    return `${pathDir(source)}/${this.ports.stem(pathBaseName(source))}.zip`;
  }

  private defaultSqzRepairDest(): string {
    const source = this.sourcePath();
    if (!source) return this.openArchiveFirstLabel();
    return `${pathDir(source)}/${this.ports.stem(pathBaseName(source))}.repaired.sqz`;
  }

  private defaultZipRepairDest(): string {
    const source = this.sourcePath();
    if (!source) return this.openArchiveFirstLabel();
    return `${pathDir(source)}/${this.ports.stem(pathBaseName(source))}.rebuilt.zip`;
  }

  sourcePath(): string | null {
    if (this.sourceMode === "current") return this.ports.read().archive?.path ?? null;
    if (this.sourceMode === "selected") return this.sourceOverride;
    return null;
  }

  jobSource(): string | null {
    const source = this.sourcePath();
    return this.usesCurrentArchive() ? this.ports.read().archive?.source ?? null : source;
  }

  sourceName(): string | null {
    const source = this.sourcePath();
    return source ? pathBaseName(source) : null;
  }

  private defaultSidecar(): string | null {
    const source = this.sourcePath();
    return source ? `${source}.par2` : null;
  }

  sidecarPath(): string | null {
    return this.sidecarOverride ?? this.defaultSidecar();
  }

  usesCurrentArchive(): boolean {
    const source = this.sourcePath();
    const archive = this.ports.read().archive;
    return Boolean(
      this.sourceMode === "current" &&
      source &&
      archive &&
      this.sameFilePath(source, archive.path),
    );
  }

  private sourceIsSplit(): boolean {
    const source = this.sourcePath();
    return Boolean(source && /\.\d{3,}$/i.test(pathBaseName(source)));
  }

  private repairUsesDirectory(): boolean {
    const reportedCount = this.report()?.source_file_count;
    if (reportedCount != null) return reportedCount > 1;
    return this.sourceIsSplit()
      || (this.usesCurrentArchive() && (this.ports.read().archive?.volumes?.length ?? 0) > 1);
  }

  private sourceFormat(): string | null {
    const source = this.sourcePath();
    if (!source) return null;
    if (this.usesCurrentArchive()) return this.ports.read().archive?.format.toLowerCase() ?? null;
    return this.ports.extension(pathBaseName(source));
  }

  private isZipFamily(): boolean {
    const format = this.sourceFormat();
    return Boolean(format && ["zip", "jar", "apk", "cbz", "ipa"].includes(format));
  }

  private isSqz(): boolean {
    return this.sourceFormat() === "sqz";
  }

  private defaultPar2RepairDest(): string {
    const source = this.sourcePath();
    if (!source) return this.openArchiveFirstLabel();
    const sourceName = pathBaseName(source);
    const extension = this.ports.extension(sourceName);
    const suffix = extension ? `.${extension}` : "";
    return `${pathDir(source)}/${this.ports.stem(sourceName)}.repaired${suffix}`;
  }

  private defaultPar2RepairDirectoryName(): string {
    const source = this.sourcePath();
    if (!source) return tr("gui.recovery.repaired_set_folder_name");
    return `${this.ports.stem(pathBaseName(source))}.repaired`;
  }

  private zipDisabledReason(): string {
    if (!this.sourcePath()) return tr("gui.recovery.choose_archive_before_zip_rebuild");
    if (this.sourceIsSplit()) return tr("gui.recovery.zip_rebuild_no_split");
    return this.isZipFamily() ? "" : tr("gui.recovery.zip_rebuild_zip_family_only");
  }

  private sqzExportDisabledReason(): string {
    if (!this.isSqz()) return tr("gui.recovery.choose_sqz_before_export");
    return this.usesCurrentArchive()
      ? ""
      : tr("gui.recovery.open_selected_sqz_before_export");
  }

  private sqzRepairDisabledReason(): string {
    return this.isSqz()
      ? ""
      : tr("gui.recovery.choose_sqz_before_repair");
  }

  private protectSourceDisabledReason(): string {
    if (!this.sourcePath()) return tr("gui.recovery.choose_archive_before_protect");
    if (!this.usesCurrentArchive()) return tr("gui.recovery.open_selected_before_protect");
    if (this.ports.read().archive?.read_only) return this.ports.mutationDisabledReason();
    return "";
  }

  private redundancyValue(): number | null {
    const value = this.redundancyDraft.trim();
    if (!/^\d+$/.test(value)) return null;
    const percent = Number(value);
    return Number.isSafeInteger(percent) && percent >= 1 && percent <= 100
      ? percent
      : null;
  }

  private redundancyError(): string {
    return this.redundancyValue() === null
      ? tr("gui.recovery.redundancy_invalid")
      : "";
  }

  setRedundancy(value: string): void {
    this.redundancyDraft = value;
  }

  private protectDisabledReason(): string {
    return this.protectSourceDisabledReason() || this.redundancyError();
  }

  private verifyDisabledReason(): string {
    if (this.usesCurrentArchive() && this.ports.read().archive?.read_only) return this.ports.mutationDisabledReason();
    return this.sourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_verify");
  }

  private repairPar2DisabledReason(): string {
    if (!this.sourcePath()) return tr("gui.recovery.choose_archive_before_par2_repair");
    if (this.usesCurrentArchive() && this.ports.read().archive?.read_only) return this.ports.mutationDisabledReason();
    const gate = recoveryRepairGate(this.report());
    if (gate === "verify_first") {
      return tr("gui.recovery.verify_before_repair");
    }
    if (gate === "no_damage") {
      return tr("gui.recovery.no_damage_to_repair");
    }
    if (gate === "over_capacity") {
      return tr("gui.recovery.insufficient_par2_data");
    }
    return "";
  }

  private testDisabledReason(): string {
    return this.sourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_test");
  }

  private bestEffortDisabledReason(): string {
    return this.sourcePath()
      ? ""
      : tr("gui.recovery.choose_archive_before_best_effort");
  }

  private pickerBusyReason(): string {
    return this.pickerStatus === "idle"
      ? ""
      : tr("gui.recovery.file_picker_open");
  }

  private encodingForJob(): string | null {
    return this.usesCurrentArchive() ? this.ports.read().archive?.encoding_override ?? null : null;
  }

  private selectionForJob(): string[] | null {
    return this.usesCurrentArchive() ? this.ports.read().selection : null;
  }

  chooseArchive(): Promise<void> {
    return this.choose("archive");
  }

  choosePar2(): Promise<void> {
    return this.choose("par2");
  }

  private async choose(kind: "archive" | "par2"): Promise<void> {
    if (this.disposed || !this.ports.read().active || this.pickerStatus !== "idle") return;
    const request = ++this.pickerRequest;
    const isCurrent = () => !this.disposed && request === this.pickerRequest && this.ports.read().active;
    const archive = kind === "archive";
    this.pickerStatus = kind;
    this.ports.notice(archive
      ? tr("gui.recovery.opening_archive_picker")
      : tr("gui.recovery.opening_par2_picker"));
    try {
      const { open } = await this.ports.dialogs();
      if (!isCurrent()) return;
      const selected = await this.ports.openDialog(`recovery.choose-${kind}`, open, {
        title: archive ? tr("gui.recovery.choose_archive") : tr("gui.recovery.choose_par2", "Choose PAR2 file"),
        multiple: false,
        directory: false,
        ...(archive ? {} : { filters: [{ name: tr("gui.recovery.par2_files"), extensions: ["par2"] }] }),
      });
      if (!isCurrent()) return;
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (typeof path !== "string") {
        this.ports.notice(archive ? tr("gui.recovery.archive_picker_cancelled")
          : tr("gui.recovery.par2_picker_cancelled"));
        return;
      }
      this.dismissOutput();
      if (archive) {
        this.sourceMode = "selected";
        this.sourceOverride = path;
      } else {
        this.sidecarOverride = path;
      }
      this.ports.notice((archive ? tr("gui.recovery.archive_selected")
        : tr("gui.recovery.par2_selected")).replace("{name}", pathBaseName(path)));
    } catch {
      if (isCurrent()) this.ports.notice(archive
        ? tr("gui.recovery.archive_picker_unavailable")
        : tr("gui.recovery.par2_picker_unavailable"));
    } finally {
      if (isCurrent()) this.pickerStatus = "idle";
    }
  }


  useCurrentArchive() {
    const archive = this.ports.read().archive;
    if (!archive) {
      this.ports.notice(tr("gui.recovery.no_current_archive", "No archive is currently open."));
      return;
    }
    this.leave();
    this.sourceMode = "current";
    this.sourceOverride = null;
    this.ports.notice(
      tr("gui.recovery.using_current_archive").replace("{name}", archive.name),
    );
  }

  useDefaultPar2() {
    const source = this.sourcePath();
    if (!source) {
      this.ports.notice(tr("gui.recovery.choose_archive_before_default_par2", "Choose an archive before using its default PAR2 file."));
      return;
    }
    this.leave();
    this.sidecarOverride = null;
    this.ports.notice(
      tr("gui.recovery.using_default_par2")
        .replace("{name}", pathBaseName(`${source}.par2`)),
    );
  }

  async testArchive() {
    const source = this.sourcePath();
    const jobSource = this.jobSource();
    if (!source || !jobSource) {
      this.ports.notice(tr("gui.recovery.choose_archive_before_test"));
      return;
    }
    await this.queue({ kind: "test", path: jobSource, encoding: this.encodingForJob(), password: null }, {
      notice: tr("gui.recovery.archive_test_started"),
      title: tr("gui.test.queued"),
      detail: pathBaseName(source),
      failure: tr("gui.recovery.archive_test_requires_desktop_service"),
      trackContext: true,
    });
  }

  private outputContext(): string {
    return JSON.stringify([
      this.ports.read().active, this.ports.read().mode, this.sourceMode, this.sourcePath(), this.jobSource(),
      this.sidecarOverride, this.sourceMode === "current" ? this.ports.read().generation : null,
    ]);
  }

  private isCurrentOutputPreparation(request: OutputPreparation): boolean {
    return !this.disposed && this.outputPreparation?.id === request.id
      && request.context === this.outputContext();
  }

  private outputStatus(): string {
    switch (this.outputPreparation?.phase) {
      case "opening":
        return tr("gui.recovery.preparing_output");
      case "choosing":
        return tr("gui.recovery.choosing_output");
      case "checking":
        return tr("gui.output.checking_existing");
      case "submitting":
        return tr("gui.recovery.submitting_task");
      default:
        return this.submissionPending
          ? tr("gui.recovery.submitting_task")
          : "";
    }
  }

  async submitOutput(kind: RecoveryOutputKind) {
    if (this.disposed || !this.ports.read().active || this.outputPreparation || this.submissionPending
      || this.pickerStatus !== "idle") return;
    const source = this.sourcePath();
    const jobSource = this.jobSource();
    const outputDirectory = kind === "repair_recovery" && this.repairUsesDirectory();
    const extension = source ? this.ports.extension(pathBaseName(source)) : null;
    const plans = {
      export_sqz: {
        disabledReason: this.sqzExportDisabledReason(),
        missing: tr("gui.recovery.choose_sqz_before_export"),
        operation: "recovery.export-sqz",
        title: tr("gui.recovery.export_sqz_as"),
        defaultPath: this.defaultSqzExportDest(),
        filters: [
          { name: this.ports.outputFilterName("zip"), extensions: ["zip"] },
          { name: this.ports.outputFilterName("7z"), extensions: ["7z"] },
          { name: this.ports.outputFilterName("tar.zst"), extensions: ["tar.zst", "tzst"] },
          { name: this.ports.outputFilterName("tar"), extensions: ["tar"] },
        ],
        cancelled: tr("gui.recovery.export_sqz_cancelled"),
        queued: tr("gui.recovery.sqz_export_queued"),
      },
      repair_sqz: {
        disabledReason: this.sqzRepairDisabledReason(),
        missing: tr("gui.recovery.choose_sqz_before_repair"),
        operation: "recovery.repair-sqz",
        title: tr("gui.recovery.repair_sqz_as"),
        defaultPath: this.defaultSqzRepairDest(),
        filters: [{ name: this.ports.outputFilterName("sqz"), extensions: ["sqz"] }],
        cancelled: tr("gui.recovery.repair_sqz_cancelled"),
        queued: tr("gui.recovery.sqz_repair_queued"),
      },
      repair_zip: {
        disabledReason: this.zipDisabledReason(),
        missing: tr("gui.recovery.choose_archive_before_zip_rebuild"),
        operation: "recovery.rebuild-zip-index",
        title: tr("gui.recovery.rebuild_zip_index_as"),
        defaultPath: this.defaultZipRepairDest(),
        filters: [{ name: this.ports.outputFilterName("zip"), extensions: ["zip"] }],
        cancelled: tr("gui.recovery.zip_rebuild_cancelled"),
        queued: tr("gui.recovery.zip_rebuild_queued"),
      },
      repair_recovery: {
        disabledReason: this.repairPar2DisabledReason(),
        missing: tr("gui.recovery.choose_archive_before_par2_repair"),
        operation: "recovery.repair-par2",
        title: tr("gui.recovery.repair_par2_as"),
        defaultPath: this.defaultPar2RepairDest(),
        filters: extension
          ? [{
              name: tr("gui.recovery.repaired_archive_filter"),
              extensions: [extension],
            }]
          : [],
        cancelled: tr("gui.recovery.repair_par2_cancelled"),
        queued: outputDirectory
          ? tr("gui.recovery.par2_set_repair_queued")
          : tr("gui.recovery.par2_repair_queued"),
      },
    };
    const plan = plans[kind];
    if (plan.disabledReason || !source || !jobSource) {
      this.ports.notice(plan.disabledReason || plan.missing);
      return;
    }
    if (this.ports.focusBlockingTask()) return;
    const sidecar = this.sidecarOverride;
    const directoryName = this.defaultPar2RepairDirectoryName();
    const level = this.ports.read().level;
    const profile = this.ports.read().profile;
    const request: OutputPreparation = {
      id: Symbol(), context: this.outputContext(), phase: "opening",
    };
    this.outputPreparation = request;
    const isCurrent = () => this.isCurrentOutputPreparation(request);
    const setPhase = (phase: OutputPreparation["phase"]) => {
      if (this.outputPreparation?.id === request.id) this.outputPreparation.phase = phase;
    };
    const inspect = async (candidate: string, splitOutput: boolean) => {
      const inspection = { id: this.ports.inspectionId() };
      this.inspection = inspection;
      try {
        return await ipc.inspectCreateDestination(candidate, splitOutput, inspection.id, null);
      } catch (error) {
        throw new DestinationInspectionError(error);
      } finally {
        if (this.inspection === inspection) this.inspection = null;
      }
    };
    try {
      const dialogs = await this.ports.dialogs();
      if (!isCurrent()) return;
      setPhase("choosing");
      let dest: string | null = null;
      if (outputDirectory) {
        const selected = await this.ports.openDialog("recovery.repair-par2-set-parent", dialogs.open, {
          title: tr("gui.recovery.choose_repaired_set_parent"),
          defaultPath: pathDir(source),
          multiple: false,
          directory: true,
        });
        if (!isCurrent()) return;
        if (typeof selected === "string") {
          setPhase("checking");
          dest = await ipc.uniqueCreateDestination(this.joinFolderPath(selected, directoryName), false);
          if (!isCurrent()) return;
        }
      } else {
        dest = await this.ports.saveDialog(plan.operation, dialogs.save, {
          title: plan.title, defaultPath: plan.defaultPath, filters: plan.filters,
        });
        if (!isCurrent()) return;
      }
      if (!dest) {
        this.ports.notice(plan.cancelled);
        return;
      }
      if (kind !== "export_sqz" && (this.sameFilePath(dest, source) || this.sameFilePath(dest, jobSource))) {
        this.ports.notice(tr("gui.recovery.repair_output_must_differ"));
        return;
      }
      setPhase("checking");
      let authorization: ArchiveOutputAuthorization | null = null;
      if (kind === "export_sqz") {
        authorization = await authorizeArchiveOutput({
          path: dest, inspect: () => inspect(dest, false), isCurrent,
          confirm: async () => dialogs.confirm, tr, message: "gui.output.replace_existing",
        });
        if (!isCurrent()) return;
        if (!authorization) {
          this.ports.notice(plan.cancelled);
          return;
        }
      } else {
        const inspection = await inspect(dest, false);
        if (!isCurrent()) return;
        if (inspection.conflict) {
          this.ports.notice(tr("gui.recovery.repair_output_exists"));
          return;
        }
      }
      let spec: JobSpec;
      if (kind === "repair_recovery") {
        spec = { kind, path: jobSource, output: dest, output_directory: outputDirectory, recovery: sidecar };
      } else if (kind === "export_sqz") {
        if (!authorization) return;
        spec = {
          kind, src: jobSource, dest, level, dest_password: null,
          replace_existing: authorization.replaceExisting,
          replacement_guard: authorization.replacementGuard,
        };
      } else {
        spec = { kind, src: jobSource, dest, level };
      }
      setPhase("submitting");
      await this.ports.submit(spec);
      recordOperation({
        status: "queued",
        title: plan.queued,
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}${kind === "export_sqz" ? ` · ${profile}` : ""}`,
      });
      if (isCurrent()) this.ports.notice(plan.queued);
    } catch (error) {
      if (!isCurrent() || this.ports.blocked(error)) return;
      if (destinationInspectionCancelled(error)) {
        this.ports.notice(plan.cancelled);
      } else if (error instanceof DestinationInspectionError) {
        this.ports.notice(error.detail
          ? tError(error.detail)
          : tr("gui.output.inspect_failed"));
      } else {
        this.ports.notice(tr("gui.recovery.output_preparation_failed"));
      }
    } finally {
      if (this.outputPreparation?.id === request.id) this.outputPreparation = null;
    }
  }

  async protect() {
    const disabledReason = this.protectDisabledReason();
    const source = this.sourcePath();
    const jobSource = this.jobSource();
    const redundancy = this.redundancyValue();
    if (disabledReason || !source || !jobSource || redundancy === null) {
      this.ports.notice(disabledReason || tr("gui.recovery.choose_archive_before_protect"));
      return;
    }
    await this.queue({ kind: "protect", path: jobSource, redundancy, recovery: null }, {
      notice: tr("gui.recovery.par2_protection_queued"),
      detail: pathBaseName(source),
      failure: tr("gui.recovery.protect_requires_desktop_service"),
    });
  }

  async verify() {
    const disabledReason = this.verifyDisabledReason();
    const source = this.sourcePath();
    const jobSource = this.jobSource();
    if (disabledReason || !source || !jobSource) {
      this.ports.notice(disabledReason || tr("gui.recovery.choose_archive_before_verify"));
      return;
    }
    await this.queue({ kind: "verify_recovery", path: jobSource, recovery: this.sidecarOverride }, {
      notice: tr("gui.recovery.par2_verify_queued"),
      detail: `${pathBaseName(source)} · ${pathBaseName(this.sidecarPath() ?? "")}`,
      failure: tr("gui.recovery.verify_requires_desktop_service"),
    });
  }

  async extractReadable() {
    const source = this.sourcePath();
    const jobSource = this.jobSource();
    if (!source || !jobSource) {
      this.ports.notice(tr("gui.recovery.choose_archive_before_best_effort"));
      return;
    }
    const selection = this.selectionForJob();
    const dest = `${this.ports.extractDestination(source)}-readable`;
    await this.queue({
        kind: "extract",
        path: jobSource,
        dest,
        expected_destination: null,
        expected_input_guard: null,
        selection,
        overwrite: "rename",
        symlinks: "preserve",
        smart: true,
        encoding: this.encodingForJob(),
        password: null,
        verify_sfx: false,
        best_effort: true,
      }, {
        notice: selection
        ? tr("gui.recovery.best_effort_selected_queued")
        : tr("gui.recovery.best_effort_queued"),
        detail: `${pathBaseName(source)} -> ${pathBaseName(dest)}`,
        failure: tr("gui.recovery.best_effort_requires_desktop_service"),
        trackContext: true,
      });
  }

  private async queue(spec: JobSpec, feedback: {
    notice: string;
    title?: string;
    detail: string;
    failure: string;
    trackContext?: boolean;
  }): Promise<void> {
    if (this.disposed || !this.ports.read().active || this.submissionPending
      || this.outputPreparation || this.pickerStatus !== "idle" || this.ports.focusBlockingTask()) return;
    const request = Symbol();
    const context = this.outputContext();
    const isCurrent = () => !this.disposed && this.submission === request && context === this.outputContext();
    this.submission = request;
    try {
      const id = await this.ports.submit(spec);
      if (feedback.trackContext) this.contextTaskIds.add(id);
      recordOperation({ status: "queued", title: feedback.title ?? feedback.notice, detail: feedback.detail });
      if (isCurrent()) this.ports.notice(feedback.notice);
    } catch (error) {
      if (isCurrent() && !this.ports.blocked(error)) this.ports.notice(feedback.failure);
    } finally {
      if (this.submission === request) this.submission = null;
    }
  }

  adoptTask(task: TaskDialogModel, displayedSpec: JobSpec = task.spec): boolean {
    let source: string | null = null;
    let sidecar: string | null | undefined;
    switch (task.spec.kind) {
      case "protect":
        source = task.spec.path;
        sidecar = null;
        break;
      case "verify_recovery":
      case "repair_recovery":
        source = task.spec.path;
        sidecar = task.spec.recovery;
        break;
      case "repair_zip":
      case "repair_sqz":
      case "export_sqz":
        source = task.spec.src;
        sidecar = null;
        break;
      case "extract":
        if (!task.spec.best_effort) return false;
        source = task.spec.path;
        sidecar = this.sourcePath() && this.sameFilePath(this.sourcePath() ?? "", source)
          ? undefined
          : null;
        break;
      case "test":
        source = task.spec.path;
        sidecar = this.sourcePath() && this.sameFilePath(this.sourcePath() ?? "", source)
          ? undefined
          : null;
        break;
      default:
        return false;
    }
    if (!source) return false;
    const displayedSource = "path" in displayedSpec ? displayedSpec.path
      : "src" in displayedSpec ? displayedSpec.src : source;
    const archive = this.ports.read().archive;
    const matchesCurrentSource = Boolean(archive && this.sameFilePath(archive.source, source));
    if (source !== displayedSource && !matchesCurrentSource) {
      this.ports.notice(tr("gui.task.review.source_unavailable"));
      return false;
    }
    const preserveSelectedSource = Boolean(
      this.sourceMode === "selected" &&
      this.sourceOverride &&
      this.sameFilePath(this.sourceOverride, source),
    );
    this.leave();
    this.sourceMode = matchesCurrentSource || (!preserveSelectedSource && archive && this.sameFilePath(archive.path, source))
      ? "current"
      : "selected";
    this.sourceOverride = this.sourceMode === "selected" ? source : null;
    if (sidecar !== undefined) this.sidecarOverride = sidecar;
    return true;
  }

  usesTaskContext(task: TaskDialogModel): boolean {
    if (task.spec.kind !== "test") return false;
    if (task.id !== null && this.contextTaskIds.has(task.id)) return true;
    if (
      this.sourceMode === "selected" &&
      this.sourceOverride &&
      this.sameFilePath(this.sourceOverride, task.spec.path)
    ) {
      return true;
    }
    const archive = this.ports.read().archive;
    return !archive || (!this.sameFilePath(archive.source, task.spec.path) && !this.sameFilePath(archive.path, task.spec.path));
  }

  latestReportTask(): Task | null {
    const source = this.sourcePath();
    const sidecar = this.sidecarPath();
    if (!source || !sidecar) return null;
    return latestMatchingRecoveryTask(this.ports.read().tasks, (task) => {
      if (task.spec.kind !== "verify_recovery" && task.spec.kind !== "repair_recovery") return false;
      const taskSidecar = task.spec.recovery ?? `${task.spec.path}.par2`;
      return this.sameFilePath(task.spec.path, source) && this.sameFilePath(taskSidecar, sidecar);
    });
  }

  private report() {
    return recoveryReport(this.latestReportTask()?.result ?? null);
  }

  private metrics() {
    return recoveryResultMetrics(this.report());
  }

  private metricNumber(key: Parameters<typeof recoveryResultMetricNumber>[1]): number | null {
    return recoveryResultMetricNumber(this.report(), key);
  }

  private metricBoolean(key: Parameters<typeof recoveryResultMetricBoolean>[1]): boolean | null {
    return recoveryResultMetricBoolean(this.report(), key);
  }

  resultAvailable(): boolean {
    return this.report() !== null;
  }

  metricsAvailable(): boolean {
    return this.metrics() !== null;
  }

  private beyondCapacity(): boolean {
    return this.metricsAvailable() && this.metricBoolean("repair_possible") === false;
  }

  private noDamage(): boolean {
    return recoveryResultHasNoDamage(this.report());
  }

  resultTone(): RecoveryWorkspaceView["resultTone"] {
    return recoveryResultToneFor(this.report());
  }

  private verifyRecommended(): boolean {
    return !this.resultAvailable() && !this.verifyDisabledReason();
  }

  private repairRecommended(): boolean {
    return recoveryResultOperation(this.report()) === "verify"
      && recoveryResultConfirmsRepairCapacity(this.report())
      && !this.repairPar2DisabledReason();
  }

  private remainingMargin(): number | null {
    const needed = this.metricNumber("blocks_needed");
    const available = this.metricNumber("recovery_blocks_available");
    if (needed === null || available === null) return null;
    return Math.max(0, available - needed);
  }

  private overCapacityDetail(): string {
    const needed = this.metricNumber("blocks_needed") ?? 0;
    const available = this.metricNumber("recovery_blocks_available") ?? 0;
    return tr("gui.recovery.damage_over_capacity_values")
      .replace("{needed}", needed.toLocaleString())
      .replace("{available}", available.toLocaleString());
  }

  resultTitle(): string {
    if (!this.sourcePath()) return tr("gui.recovery.no_archive_selected");
    if (!this.resultAvailable()) return tr("gui.recovery.not_verified");
    const operation = recoveryResultOperation(this.report());
    const ok = recoveryResultOk(this.report());
    if (operation === "repair") {
      if (ok === true) return tr("gui.recovery.repair_completed");
      return tr("gui.recovery.repair_not_completed");
    }
    if (this.noDamage()) {
      return tr("gui.recovery.no_damage");
    }
    const repairPossible = this.metricBoolean("repair_possible");
    if (repairPossible === true) return tr("gui.recovery.repairable");
    if (repairPossible === false) return tr("gui.recovery.not_repairable");
    return ok === true
      ? tr("gui.recovery.verification_passed")
      : tr("gui.recovery.damage_detected");
  }

  resultDetail(): string {
    if (!this.sourcePath()) return tr("gui.recovery.choose_archive_before_verify");
    if (!this.resultAvailable()) return tr("gui.recovery.run_verify_capacity");
    const needed = this.metricNumber("blocks_needed");
    const available = this.metricNumber("recovery_blocks_available");
    if (needed !== null && available !== null) {
      return tr("gui.recovery.capacity_summary")
        .replace("{needed}", needed.toLocaleString())
        .replace("{available}", available.toLocaleString());
    }
    return recoveryResultOk(this.report()) === true
      ? tr("gui.recovery.tool_no_block_counts_ok")
      : tr("gui.recovery.tool_no_block_counts_failed");
  }

  resultExplanation(): string {
    if (!this.resultAvailable() || !this.metricsAvailable()) return this.resultDetail();
    if (recoveryResultOperation(this.report()) === "repair" && recoveryResultOk(this.report()) === false) {
      if (this.beyondCapacity()) return this.overCapacityDetail();
      return tr("gui.recovery.repair_incomplete_despite_capacity");
    }
    if (this.noDamage()) {
      return tr("gui.recovery.no_damage_body");
    }
    if (this.metricBoolean("repair_possible") === true) {
      return tr("gui.recovery.damage_within_rs_capacity");
    }
    return this.overCapacityDetail();
  }

  resultFooter(): string {
    if (!this.sourcePath()) return tr("gui.recovery.choose_archive_before_verify");
    if (!this.resultAvailable()) return tr("gui.recovery.no_verification_result");
    const repaired = this.metricNumber("blocks_repaired") ?? 0;
    if (repaired > 0) {
      return tr("gui.recovery.blocks_repaired").replace("{count}", repaired.toLocaleString());
    }
    if (recoveryResultOperation(this.report()) === "repair" && recoveryResultOk(this.report()) === true) {
      const output = this.report()?.output;
      return output
        ? tr("gui.recovery.repaired_copy_ready").replace("{name}", pathBaseName(output))
        : tr("gui.recovery.repair_completed");
    }
    if (recoveryResultOk(this.report()) === false) {
      return this.beyondCapacity()
        ? tr("gui.recovery.damage_exceeds_capacity")
        : tr("gui.recovery.verification_did_not_pass");
    }
    const task = this.latestReportTask();
    return task ? taskOutcomeStateLabel(task) : taskStateLabel(null);
  }

  private redundancyLabel(): string {
    const redundancy = this.redundancyValue();
    if (redundancy === null) return this.redundancyError();
    return tr("gui.recovery.redundancy_percent", "{percent}% requested redundancy")
      .replace("{percent}", redundancy.toLocaleString());
  }

  view(): RecoveryWorkspaceView {
    const source = this.sourcePath();
    const preparationMessage = this.outputStatus();
    const actionBusyReason = preparationMessage || this.pickerBusyReason();
    const protectSourceDisabledReason = this.protectSourceDisabledReason();
    const protectDisabledReason = this.protectDisabledReason();
    const resultAvailable = this.resultAvailable();
    const metricsAvailable = this.metricsAvailable();
    const sourceIsSqz = this.isSqz();
    const notAvailable = tr("gui.recovery.not_available_for_target");
    const protectedSourceCount = this.report()?.source_file_count
      ?? (this.usesCurrentArchive()
        ? Math.max(1, this.ports.read().archive?.volumes?.length ?? 1)
        : 1);
    const metrics = metricsAvailable
      ? {
          blocksNeeded: this.metricNumber("blocks_needed")?.toLocaleString() ?? "-",
          recoveryBlocksAvailable:
            this.metricNumber("recovery_blocks_available")?.toLocaleString() ?? "-",
          remainingMargin: this.remainingMargin()?.toLocaleString() ?? "-",
        }
      : null;
    const formatWorkflowTitle = sourceIsSqz
      ? tr("gui.recovery.sqz_capable_not_checked", "SQZ recovery capable · not checked")
      : source
        ? tr("gui.recovery.par2_sidecar_workflow")
        : tr("gui.recovery.no_archive_selected");
    const formatWorkflowBody = sourceIsSqz
      ? tr(
          "gui.recovery.sqz_capability_body",
          "SQZ supports embedded recovery, but this file has not been checked until a repair or test task reports a result.",
        )
      : source
        ? tr(
            "gui.recovery.standard_archive_capability_body",
          )
        : tr(
            "gui.recovery.choose_archive_for_capabilities",
            "Choose an archive to see the recovery tools available for its format.",
          );

    return {
      archiveName: source,
      par2Name: this.sidecarPath(),
      currentArchiveAvailable: this.ports.read().archive !== null,
      usesCurrentArchive: this.usesCurrentArchive(),
      usesDefaultPar2: source !== null && this.sidecarOverride === null,
      pickerBusy: this.pickerStatus !== "idle",
      pickerBusyReason: this.pickerBusyReason(),
      preparationMessage,
      testDisabledReason: actionBusyReason || this.testDisabledReason(),
      sourceName: this.sourceName() ?? tr("gui.recovery.no_archive_selected"),
      requestedRedundancy: protectSourceDisabledReason
        ? notAvailable
        : this.redundancyLabel(),
      redundancyDraft: this.redundancyDraft,
      redundancyError: this.redundancyError(),
      protectedSourceCount,
      repairCapacity: resultAvailable
        ? this.resultDetail()
        : tr("gui.recovery.shown_after_verify"),
      repairOutputMode: this.repairUsesDirectory()
        ? tr(
            "gui.recovery.repair_output_new_folder",
          ).replace("{count}", protectedSourceCount.toLocaleString())
        : tr(
            "gui.recovery.repair_output_new_file",
          ),
      plannedIndex: protectSourceDisabledReason
        ? notAvailable
        : pathBaseName(this.defaultSidecar() ?? ""),
      resultTone: this.resultTone(),
      resultTitle: this.resultTitle(),
      resultDetail: this.resultDetail(),
      resultExplanation: this.resultExplanation(),
      resultFooter: this.resultFooter(),
      resultAvailable,
      metrics,
      beyondCapacity: this.beyondCapacity(),
      formatWorkflowTitle,
      formatWorkflowBody,
      protectDisabledReason: actionBusyReason || protectDisabledReason,
      verifyDisabledReason: actionBusyReason || this.verifyDisabledReason(),
      repairDisabledReason: actionBusyReason || this.repairPar2DisabledReason(),
      zipDisabledReason: actionBusyReason || this.zipDisabledReason(),
      sqzRepairDisabledReason: actionBusyReason || this.sqzRepairDisabledReason(),
      sqzExportDisabledReason: actionBusyReason || this.sqzExportDisabledReason(),
      bestEffortDisabledReason: actionBusyReason || this.bestEffortDisabledReason(),
      verifyRecommended: this.verifyRecommended(),
      repairRecommended: this.repairRecommended(),
    };
  }

}
