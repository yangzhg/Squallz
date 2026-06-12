import { basename } from "./format";
import type { JobSpec } from "./ipc";
import { t } from "./i18n.svelte";

function fill(template: string, params: Record<string, string | number>): string {
  let out = template;
  for (const [name, value] of Object.entries(params)) {
    out = out.split(`{${name}}`).join(String(value));
  }
  return out;
}

function translate(key: string, fallback: string, params: Record<string, string | number>): string {
  const value = t(key, params);
  return value === key ? fill(fallback, params) : value;
}

export function jobTitleFor(spec: JobSpec): string {
  switch (spec.kind) {
    case "compress":
      return translate("gui.task.job.compress", "Compress {name}", { name: basename(spec.dest) });
    case "extract":
      return translate("gui.task.job.extract", "Extract {name}", { name: basename(spec.path) });
    case "batch_extract":
      return translate("gui.task.job.batch_extract", "Extract {count} archives", { count: spec.items.length });
    case "extract_nested":
      return translate("gui.task.job.extract", "Extract {name}", { name: basename(spec.entry_path) });
    case "test":
      return translate("gui.task.job.test", "Test {name}", { name: basename(spec.path) });
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
      return translate("gui.task.job.protect", "Protect {name}", { name: basename(spec.path) });
    case "verify_recovery":
      return translate("gui.task.job.verify_recovery", "Verify recovery for {name}", { name: basename(spec.path) });
    case "repair_recovery":
      return translate("gui.task.job.repair_recovery", "Repair {name}", { name: basename(spec.path) });
    case "update":
      return translate("gui.task.job.update", "Update {name}", { name: basename(spec.path) });
    case "checksum":
      return translate("gui.task.job.checksum", "Checksum {name}", { name: basename(spec.inputs[0] ?? "files") });
    case "checksum_check":
      return translate("gui.task.job.checksum_check", "Verify checksums {name}", { name: basename(spec.manifest) });
    case "duplicate_scan":
      return translate("gui.task.job.duplicate_scan", "Find duplicates in {name}", { name: basename(spec.inputs[0] ?? "scan") });
  }
}
