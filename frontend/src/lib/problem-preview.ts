type ResultRecord = Record<string, unknown> | null | undefined;

export function resultProblemMessages(result: ResultRecord): unknown[] {
  if (Array.isArray(result?.problem_messages)) return result.problem_messages;
  if (Array.isArray(result?.problems)) return result.problems;
  return [];
}

export function resultProblemTotal(result: ResultRecord): number {
  const messages = resultProblemMessages(result);
  const reportedTotal = result?.problems_total;
  if (typeof reportedTotal === "number" && Number.isFinite(reportedTotal)) {
    return Math.max(messages.length, Math.max(0, Math.trunc(reportedTotal)));
  }
  const legacyTotal = result?.problems;
  if (typeof legacyTotal === "number" && Number.isFinite(legacyTotal)) {
    return Math.max(messages.length, Math.max(0, Math.trunc(legacyTotal)));
  }
  return messages.length;
}
