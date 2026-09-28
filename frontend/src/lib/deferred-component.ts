export type DeferredComponentModule<T> = Readonly<{ default: T }>;

export interface DeferredComponentLoader<T> {
  load: () => Promise<T>;
  retry: () => Promise<T>;
}

export class DeferredComponentLoadError extends Error {
  constructor(cause: unknown, readonly retryFailed: boolean) {
    super("View could not be loaded", { cause });
  }
}

export function createDeferredComponentLoader<T>(
  loadModule: () => Promise<DeferredComponentModule<T>>,
): DeferredComponentLoader<T> {
  let cached: Promise<T> | null = null;
  let failed = false;
  let failures = 0;

  function load(): Promise<T> {
    if (cached) return cached;
    failed = false;
    cached = Promise.resolve().then(loadModule).then(
      (module) => {
        failures = 0;
        return module.default;
      },
      (cause: unknown) => {
        failed = true;
        failures += 1;
        throw new DeferredComponentLoadError(cause, failures > 1);
      },
    );
    return cached;
  }

  function retry(): Promise<T> {
    // Keep an in-flight request and a successfully loaded component shared.
    if (failed) cached = null;
    return load();
  }

  return { load, retry };
}
