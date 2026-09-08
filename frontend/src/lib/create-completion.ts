export type CreateOpenResult = "opened" | "revealed" | "unavailable";

export async function openCreatedOutputWithFallback(
  open: () => boolean | void | Promise<boolean | void>,
  reveal: () => boolean | Promise<boolean>,
): Promise<CreateOpenResult> {
  try {
    if ((await open()) !== false) return "opened";
  } catch {
    // The reveal fallback below provides the actionable result.
  }

  try {
    return (await reveal()) ? "revealed" : "unavailable";
  } catch {
    return "unavailable";
  }
}
