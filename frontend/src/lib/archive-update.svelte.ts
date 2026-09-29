import type { CreateContentPolicy, JobSpec } from "./ipc";
import { archiveEditPathIssue } from "./archive-editing";
import { parseDelimitedRules } from "./format";

type UpdateJob = Extract<JobSpec, { kind: "update" }>;
export type UpdateOperation = {
  id: number;
  kind: "add" | "delete" | "rename" | "mkdir";
  source: string;
  value: string;
  enabled: boolean;
};
type UpdateDraft = {
  path: string;
  displayPath: string;
  encoding: string;
  level: string;
  contentPolicy: CreateContentPolicy;
  excludes: string[];
  excludesText: string;
  operations: UpdateOperation[];
};
export type UpdateIssue = { field: string; kind: "empty" | "path" | "unchanged" | "level" | "selection" };

/** Editable intent only. Passwords and execution authorization never enter the review. */
export class ArchiveUpdateReview {
  draft = $state<UpdateDraft | null>(null);
  pending = $state(false);
  issue = $state<UpdateIssue | null>(null);

  restore(spec: UpdateJob, displayPath: string): boolean {
    if (this.pending) return false;
    const operations: UpdateOperation[] = [];
    const add = (kind: UpdateOperation["kind"], source: string, value: string) => {
      operations.push({ id: operations.length, kind, source, value, enabled: true });
    };
    spec.add.forEach((path) => add("add", "", path));
    spec.mkdir.forEach((path) => add("mkdir", "", path));
    spec.delete.forEach((path) => add("delete", path, ""));
    spec.rename.forEach(({ from, to }) => add("rename", from, to));
    this.draft = { path: spec.path, displayPath, encoding: spec.encoding ?? "", level: String(spec.level),
      contentPolicy: spec.content_policy, excludes: [...spec.excludes], excludesText: spec.excludes.join("\n"), operations };
    this.issue = null;
    return true;
  }

  editOperation(id: number, change: { value?: string; enabled?: boolean }): void {
    if (!this.draft || this.pending) return;
    this.draft.operations = this.draft.operations.map((row) => row.id === id ? { ...row, ...change } : row);
    this.issue = null;
  }

  editSettings(change: Partial<Pick<UpdateDraft, "encoding" | "level" | "contentPolicy">>): void {
    if (!this.draft || this.pending) return;
    Object.assign(this.draft, change);
    this.issue = null;
  }

  editExcludes(value: string): void {
    if (!this.draft || this.pending) return;
    this.draft.excludesText = value;
    this.draft.excludes = parseDelimitedRules(value);
    this.issue = null;
  }

  selectedCount(): number {
    return this.draft?.operations.filter((row) => row.enabled).length ?? 0;
  }

  async submit(submitJob: (spec: JobSpec) => Promise<unknown>): Promise<boolean> {
    const draft = this.draft;
    if (!draft || this.pending) return false;
    const rows = draft.operations.filter((row) => row.enabled);
    this.issue = null;
    if (!rows.length) this.issue = { field: "update-review-heading", kind: "selection" };
    const level = Number(draft.level);
    if (!draft.level.trim() || !Number.isInteger(level) || level < 0 || level > 9) {
      this.issue = { field: "update-level", kind: "level" };
    }
    for (const row of rows) {
      if (row.kind === "delete") continue;
      const field = `update-operation-${row.id}`;
      if (!row.value.trim()) { this.issue = { field, kind: "empty" }; break; }
      if (row.kind !== "add" && archiveEditPathIssue(row.value)) { this.issue = { field, kind: "path" }; break; }
      if (row.kind === "rename" && row.source === row.value) { this.issue = { field, kind: "unchanged" }; break; }
    }
    if (this.issue) return false;
    const spec: UpdateJob = {
      kind: "update", path: draft.path, encoding: draft.encoding.trim() || null,
      level, content_policy: draft.contentPolicy, excludes: [...draft.excludes], password: null,
      add: rows.filter((row) => row.kind === "add").map((row) => row.value),
      mkdir: rows.filter((row) => row.kind === "mkdir").map((row) => row.value),
      delete: rows.filter((row) => row.kind === "delete").map((row) => row.source),
      rename: rows.filter((row) => row.kind === "rename").map((row) => ({ from: row.source, to: row.value })),
    };
    this.pending = true;
    try {
      await submitJob(spec);
      return true;
    } finally {
      this.pending = false;
    }
  }
}
