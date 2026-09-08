export type DeferredComponentModule<T> = Readonly<{ default: T }>;

export interface DeferredComponentLoader<T> {
  load: () => Promise<T>;
  retry: () => Promise<T>;
}

export function createDeferredComponentLoader<T>(
  loadModule: () => Promise<DeferredComponentModule<T>>,
): DeferredComponentLoader<T> {
  let cached: Promise<T> | null = null;

  function load(): Promise<T> {
    cached ??= loadModule().then((module) => module.default);
    return cached;
  }

  function retry(): Promise<T> {
    cached = null;
    return load();
  }

  return { load, retry };
}
