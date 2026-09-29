import { isErrorDto, type NestedArchivePasswords } from "./ipc";

type PasswordScope = "outer" | "inner";
type PreviewPasswordPrompt = {
  id: number;
  name: string;
  scope: PasswordScope;
  wrong: boolean;
  busy: boolean;
};

export function createPreviewPasswordFlow() {
  let prompt = $state<PreviewPasswordPrompt | null>(null);
  let generation = 0;
  let nextPromptId = 0;
  let resolveAnswer: ((value: string | null) => void) | null = null;

  function cancel() {
    generation += 1;
    const resolve = resolveAnswer;
    resolveAnswer = null;
    prompt = null;
    resolve?.(null);
  }

  return {
    get prompt() { return prompt; },
    cancel,
    answer(value: string): boolean {
      if (!prompt || prompt.busy || !resolveAnswer || value.length === 0) return false;
      const resolve = resolveAnswer;
      resolveAnswer = null;
      prompt = { ...prompt, busy: true };
      resolve(value);
      return true;
    },
    async run<T>(
      context: { outerName: string; innerName: string; isCurrent: () => boolean },
      operation: (passwords: NestedArchivePasswords) => Promise<T>,
    ): Promise<T | null> {
      cancel();
      const requestGeneration = generation;
      const passwords: NestedArchivePasswords = { outer: null, inner: null };
      const current = () => requestGeneration === generation && context.isCurrent();
      try {
        while (current()) {
          try {
            // The caller still receives a late successful handle so it can release it.
            return await operation(passwords);
          } catch (error) {
            if (!current()) return null;
            if (!isErrorDto(error) || !["error.password_required", "error.wrong_password"].includes(error.key)) throw error;
            const scope: PasswordScope = error.params.password_scope === "inner" ? "inner" : "outer";
            prompt = {
              id: ++nextPromptId,
              name: scope === "inner" ? context.innerName : context.outerName,
              scope,
              wrong: error.key === "error.wrong_password",
              busy: false,
            };
            const answer = await new Promise<string | null>((resolve) => { resolveAnswer = resolve; });
            if (answer === null || !current()) return null;
            passwords[scope] = answer;
          }
        }
        return null;
      } finally {
        passwords.outer = null;
        passwords.inner = null;
        if (requestGeneration === generation) {
          prompt = null;
          resolveAnswer = null;
        }
      }
    },
  };
}
