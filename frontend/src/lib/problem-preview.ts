import type { JobResult, ExtractReport } from "./job-result";
type Result = JobResult | ExtractReport | null | undefined;

export function resultProblemMessages(result: Result): string[] {
  return result && "problems" in result ? result.problems : [];
}

export function resultProblemTotal(result: Result): number {
  return result && "problems_total" in result ? result.problems_total : 0;
}
