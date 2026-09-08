export type CssVariableMap = Record<string, string | number | null | undefined>;

/** Supplies runtime values to custom properties consumed by design.css. */
export function cssVariables(node: HTMLElement, initial: CssVariableMap) {
  let activeNames = new Set<string>();

  function apply(next: CssVariableMap) {
    const nextNames = new Set<string>();
    for (const [name, value] of Object.entries(next)) {
      if (!name.startsWith("--") || value === null || value === undefined || value === "") continue;
      node.style.setProperty(name, String(value));
      nextNames.add(name);
    }
    for (const name of activeNames) {
      if (!nextNames.has(name)) node.style.removeProperty(name);
    }
    activeNames = nextNames;
  }

  apply(initial);
  return {
    update: apply,
    destroy() {
      for (const name of activeNames) node.style.removeProperty(name);
    },
  };
}
