import { basename } from "./format";
import type { JobSpec } from "./ipc";
import { tFallback as translate } from "./i18n.svelte";

export function jobTitleFor(spec: JobSpec): string {
  switch (spec.kind) {
    case "compress":
      return spec.sfx_target
        ? translate("gui.task.job.create_sfx", { name: basename(spec.dest) })
        : translate("gui.task.job.compress", { name: basename(spec.dest) });
    case "publish_macos_sfx":
      return translate("gui.task.job.publish_macos_sfx", { name: basename(spec.output) });
    case "extract":
      return translate("gui.task.job.extract", { name: basename(spec.path) });
    case "batch_extract":
      return translate("gui.task.job.batch_extract", {});
    case "extract_nested":
      return translate("gui.task.job.extract", { name: basename(spec.entry_path) });
    case "test":
      return translate("gui.task.job.test", { name: basename(spec.path) });
    case "convert":
      return translate("gui.task.job.convert", "Convert {from} -> {to}", {
        from: basename(spec.src),
        to: basename(spec.dest),
      });
    case "export_sqz":
      return translate("gui.task.job.export_sqz", "Export SQZ {from} -> {to}", {
        from: basename(spec.src),
        to: basename(spec.dest),
      });
    case "repair_sqz":
      return translate("gui.task.job.repair_sqz", "Repair SQZ {from} -> {to}", {
        from: basename(spec.src),
        to: basename(spec.dest),
      });
    case "repair_zip":
      return translate("gui.task.job.repair_zip", "Rebuild ZIP index {from} -> {to}", {
        from: basename(spec.src),
        to: basename(spec.dest),
      });
    case "protect":
      return translate("gui.task.job.protect", { name: basename(spec.path) });
    case "verify_recovery":
      return translate("gui.task.job.verify_recovery", { name: basename(spec.path) });
    case "repair_recovery":
      return translate("gui.task.job.repair_recovery", { name: basename(spec.path) });
    case "update":
      return translate("gui.task.job.update", { name: basename(spec.path) });
    case "checksum":
      return translate("gui.task.job.checksum", { name: basename(spec.inputs[0] ?? "files") });
    case "checksum_check":
      return translate("gui.task.job.checksum_check", { name: basename(spec.manifest) });
    case "duplicate_scan":
      return translate("gui.task.job.duplicate_scan", { name: basename(spec.inputs[0] ?? "scan") });
  }
}
