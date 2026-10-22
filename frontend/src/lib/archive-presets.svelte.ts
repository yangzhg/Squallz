import {
  ipc,
  isErrorDto,
  type ArchivePresetDocument,
  type CreateArchivePresetOptions,
  type NamedArchivePreset,
} from "./ipc";
import { tFallback as tr, tError } from "./i18n.svelte";
import { recordOperation } from "./history.svelte";
import { createArchivePresetSummary, extractArchivePresetSummary } from "./archive-option-labels";

export type PresetKind = NamedArchivePreset["kind"];
type Preset<K extends PresetKind> = Extract<NamedArchivePreset, { kind: K }>;
type PresetDraft<K extends PresetKind> = {
  options: () => Preset<K>["options"];
  apply: (preset: Preset<K> | null, announce: boolean) => boolean;
  ready: () => boolean;
  touched: () => boolean;
  disabledReason: () => string;
  saveIssue?: () => string;
};
export interface ArchivePresetPort {
  create: PresetDraft<"create">;
  extract: PresetDraft<"extract">;
  notice: (message: string) => void;
}
type Selection = { id: string | null; name: string; mutation: "idle" | "saving" | "error" };
type PresetStatus = Selection["mutation"] | "applied" | "modified";

export const defaultPresetIds = { create: "builtin.create.balanced-7z", extract: "builtin.extract.smart" } as const;
export const maxArchivePresets = 65;
export const maxArchivePresetExcludeRules = 64;
export const maxArchivePresetExcludeRuleBytes = 256;
const normalizeName = (name: string): string => Array.from(name.replace(/\s+/gu, " ").trim()).slice(0, 40).join("");
const bindingKey = (kind: PresetKind, target: "app" | "file_manager") =>
  `${target === "app" ? "app_default" : target}_${kind}` as keyof ArchivePresetDocument["bindings"];

export function archivePresetDisplayName(preset: NamedArchivePreset): string {
  if (preset.id === "builtin.create.cross-platform-7z") return tr("gui.presets.builtin_cross_platform_7z");
  if (preset.id === defaultPresetIds.create) return tr("gui.presets.builtin_balanced_7z");
  if (preset.id === defaultPresetIds.extract) return tr("gui.presets.builtin_smart_extract");
  return preset.label;
}

function finderCompatible(options: CreateArchivePresetOptions): boolean {
  return options.format === "7z" && options.credential.kind === "none" && options.output.kind === "archive"
    && options.completion !== "open_in_squallz" && options.post_success === "keep_source";
}

function sameOptions(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (left === null || right === null || typeof left !== "object" || typeof right !== "object") return false;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length
      && left.every((value, index) => sameOptions(value, right[index]));
  }
  const a = left as Record<string, unknown>;
  const b = right as Record<string, unknown>;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => Object.hasOwn(b, key) && sameOptions(a[key], b[key]));
}

/** One revisioned preset document for both workspaces. Drafts retain ownership of their options. */
export class ArchivePresetSession {
  #document = $state<ArchivePresetDocument | null>(null);
  #loadState = $state<"loading" | "ready" | "error">("loading");
  #selections = $state<Record<PresetKind, Selection>>({
    create: { id: null, name: "", mutation: "idle" },
    extract: { id: null, name: "", mutation: "idle" },
  });
  #disposed = false;

  constructor(private readonly port: ArchivePresetPort) {}

  get document(): ArchivePresetDocument | null { return this.#document; }
  get loadState() { return this.#loadState; }
  selection(kind: PresetKind): Readonly<Selection> { return this.#selections[kind]; }
  selected<K extends PresetKind>(kind: K): Preset<K> | null {
    return this.#document?.presets.find((preset) => preset.kind === kind && preset.id === this.#selections[kind].id) as Preset<K> | undefined ?? null;
  }
  options<K extends PresetKind>(kind: K): Preset<K>[] {
    return (this.#document?.presets.filter((preset) => preset.kind === kind) ?? []) as Preset<K>[];
  }
  summary(kind: PresetKind): string {
    return kind === "create" ? createArchivePresetSummary(this.port.create.options())
      : extractArchivePresetSummary(this.port.extract.options());
  }
  editName(kind: PresetKind, name: string): void { this.#selections[kind].name = name; }
  clearSelection(kind: PresetKind): void { this.#selections[kind] = { id: null, name: "", mutation: "idle" }; }
  isBound(kind: PresetKind, target: "app" | "file_manager"): boolean {
    return this.#document?.bindings[bindingKey(kind, target)] === this.#selections[kind].id;
  }

  async load(): Promise<void> {
    try {
      const document = await ipc.getArchivePresets();
      if (this.#disposed) return;
      this.#document = document;
      this.#loadState = "ready";
      this.applyDefault("create");
      this.applyDefault("extract");
    } catch {
      if (this.#disposed) return;
      this.#loadState = "error";
      this.#selections.create.mutation = this.#selections.extract.mutation = "error";
    }
  }

  dispose(): void { this.#disposed = true; }

  applyDefault(kind: PresetKind): void {
    const draft = this.port[kind];
    const id = this.#document?.bindings[bindingKey(kind, "app")];
    if (id && draft.ready() && !draft.touched()) this.apply(kind, id, false);
  }

  apply(kind: PresetKind, id: string | null, announce = true): void {
    const preset = this.#document?.presets.find((item) => item.id === id && item.kind === kind) ?? null;
    if (id && !preset) return;
    const applied = kind === "create"
      ? this.port.create.apply(preset as Preset<"create"> | null, announce)
      : this.port.extract.apply(preset as Preset<"extract"> | null, announce);
    if (applied) this.#selections[kind] = { id: preset?.id ?? null, name: preset?.label ?? "", mutation: "idle" };
  }

  status(kind: PresetKind): PresetStatus {
    const selection = this.#selections[kind];
    if (this.#loadState === "error" || selection.mutation === "error") return "error";
    if (this.#loadState === "loading" || selection.mutation === "saving") return "saving";
    const preset = this.selected(kind);
    if (!preset) return "idle";
    return !sameOptions(this.port[kind].options(), preset.options)
      || normalizeName(selection.name) !== preset.label ? "modified" : "applied";
  }

  statusLabel(kind: PresetKind): string {
    if (this.#loadState === "loading") return tr("gui.presets.loading");
    if (this.#loadState === "error") return tr("gui.presets.unavailable");
    switch (this.status(kind)) {
      case "saving": return tr("gui.presets.saving");
      case "error": return tr("gui.presets.failed");
      case "modified": return tr("gui.presets.modified");
      case "applied": return tr("gui.presets.applied");
      default: return tr("gui.presets.current");
    }
  }

  disabledReason(kind: PresetKind): string {
    if (this.#loadState === "loading") return tr("gui.presets.loading");
    if (this.#loadState === "error") return tr("gui.presets.load_failed");
    return this.port[kind].disabledReason();
  }
  readOnlyReason(kind: PresetKind): string {
    return this.selected(kind)?.built_in ? tr("gui.presets.built_in_read_only") : "";
  }
  finderDisabledReason(): string {
    const selected = this.selected("create");
    return selected && !finderCompatible(selected.options)
      ? tr("gui.presets.finder_create_incompatible") : "";
  }
  updateDisabledReason(kind: PresetKind): string {
    if (!this.selected(kind)) return "";
    const reason = this.readOnlyReason(kind) || this.port[kind].disabledReason();
    if (reason) return reason;
    return kind === "create" && this.isBound(kind, "file_manager") && !finderCompatible(this.port.create.options())
      ? tr("gui.presets.unbind_finder_before_update") : "";
  }

  async save(kind: PresetKind, operation: "save_as" | "update"): Promise<void> {
    const selected = operation === "update" ? this.selected(kind) : null;
    const next = this.#snapshot();
    if (!next) {
      if (operation === "save_as") this.port.notice(tr("gui.presets.load_failed"));
      return;
    }
    if (operation === "update" && (!selected || selected.built_in)) return;
    const issue = (operation === "update" ? this.updateDisabledReason(kind) : "") || this.port[kind].saveIssue?.();
    if (issue) { this.port.notice(issue); return; }
    const requested = normalizeName(this.#selections[kind].name);
    if (!requested) { this.port.notice(tr("gui.presets.name_required")); return; }
    let name = requested;
    let index = -1;
    if (selected) {
      if (this.#nameExists(kind, name, selected.id)) { this.port.notice(tr("gui.presets.name_in_use")); return; }
      index = next.presets.findIndex((preset) => preset.id === selected.id);
      if (index < 0) return;
    } else {
      if (next.presets.length >= maxArchivePresets) { this.port.notice(tr("gui.presets.limit")); return; }
      name = this.#uniqueName(kind, requested);
    }
    const random = globalThis.crypto?.randomUUID?.().replaceAll("-", "")
      ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
    const id = selected?.id ?? `user.${kind}.${random}`.slice(0, 64);
    const identity = { id, label: name, built_in: false };
    const preset: NamedArchivePreset = kind === "create"
      ? { ...identity, kind, options: this.port.create.options() }
      : { ...identity, kind, options: this.port.extract.options() };
    if (selected) next.presets[index] = preset;
    else next.presets.push(preset);
    if (await this.#persist(kind, next, operation === "update" ? "gui.presets.operation_updated" : "gui.presets.operation_saved",
      operation === "update" ? "Preset updated" : "Preset saved", name)) {
      if (operation === "save_as") this.#selections[kind].id = id;
      this.#selections[kind].name = name;
    }
  }

  async remove(kind: PresetKind): Promise<void> {
    const selected = this.selected(kind);
    const next = this.#snapshot();
    if (!selected || !next || selected.built_in) return;
    next.presets = next.presets.filter((preset) => preset.id !== selected.id);
    for (const target of ["app", "file_manager"] as const) {
      const key = bindingKey(kind, target);
      if (next.bindings[key] === selected.id) next.bindings[key] = defaultPresetIds[kind];
    }
    if (await this.#persist(kind, next, "gui.presets.operation_deleted", "Preset deleted", selected.label)) {
      if (this.port[kind].disabledReason()) this.clearSelection(kind);
      else this.apply(kind, defaultPresetIds[kind], false);
    }
  }

  async bind(kind: PresetKind, target: "app" | "file_manager", enabled: boolean): Promise<void> {
    const selected = this.selected(kind);
    const next = this.#snapshot();
    if (!selected || !next) return;
    if (kind === "create" && target === "file_manager" && enabled && this.finderDisabledReason()) {
      this.port.notice(this.finderDisabledReason()); return;
    }
    next.bindings[bindingKey(kind, target)] = enabled ? selected.id : null;
    await this.#persist(kind, next, "gui.presets.operation_binding", "Preset default changed", archivePresetDisplayName(selected));
  }

  #snapshot(): ArchivePresetDocument | null { return this.#document ? $state.snapshot(this.#document) : null; }
  #nameExists(kind: PresetKind, name: string, except: string | null): boolean {
    return this.options(kind).some((preset) => preset.id !== except && preset.label.toLocaleLowerCase() === name.toLocaleLowerCase());
  }
  #uniqueName(kind: PresetKind, requested: string): string {
    if (!this.#nameExists(kind, requested, null)) return requested;
    for (let index = 2; index <= 99; index += 1) {
      const suffix = ` ${index}`;
      const candidate = `${Array.from(requested).slice(0, 40 - suffix.length).join("")}${suffix}`;
      if (!this.#nameExists(kind, candidate, null)) return candidate;
    }
    return `${Array.from(requested).slice(0, 32).join("")} ${Date.now().toString(36)}`;
  }
  async #persist(kind: PresetKind, next: ArchivePresetDocument, key: string, fallback: string, detail: string): Promise<boolean> {
    if (this.#disposed || this.#selections[kind].mutation === "saving") return false;
    this.#selections[kind].mutation = "saving";
    try {
      const saved = await ipc.saveArchivePresets(next.revision, next);
      if (this.#disposed) return false;
      this.#document = saved;
      this.#loadState = "ready";
      this.#selections[kind].mutation = "idle";
      this.port.notice(tr(key, fallback));
      recordOperation({ status: "done", title: tr(key, fallback), detail });
      return true;
    } catch (error) {
      if (this.#disposed) return false;
      this.#selections[kind].mutation = "error";
      if (isErrorDto(error) && error.key === "error.presets_conflict") {
        void ipc.getArchivePresets().then((latest) => {
          if (this.#disposed) return;
          this.#document = latest;
          this.#loadState = "ready";
          for (const kind of ["create", "extract"] as const) {
            if (!this.selected(kind)) this.#selections[kind].id = null;
          }
        }).catch(() => undefined);
      }
      const fallback = tr("gui.presets.failed");
      const message = isErrorDto(error) ? tError(error) : fallback;
      this.port.notice(isErrorDto(error) && message === error.key ? fallback : message);
      return false;
    }
  }
}
