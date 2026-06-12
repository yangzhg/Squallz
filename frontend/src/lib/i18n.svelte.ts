// i18n store: fetches the merged locale table from the backend once per
// language switch and renders `{name}` placeholders. UI copy should go
// through this store rather than being embedded in components.

import { ipc } from "./ipc";
import type { ErrorDto } from "./ipc";

const store = $state({
  lang: "en-US",
  table: {} as Record<string, string>,
  ready: false,
});

const fallbackLang = "en-US";
const fallbackTables: Record<string, Record<string, string> | undefined> = {};
const fallbackLoaders: Record<string, () => Promise<{ default: Record<string, string> }>> = Object.fromEntries(
  Object.entries(import.meta.glob("../../../locales/*.json")).flatMap(([path, loader]) => {
    const tag = path.match(/\/([^/]+)\.json$/)?.[1];
    return tag ? [[tag, loader as () => Promise<{ default: Record<string, string> }>]] : [];
  }),
);
const fallbackLanguageTags = Object.keys(fallbackLoaders).sort();

function normalizedLang(lang?: string | null): string {
  const fallback = fallbackLoaders[fallbackLang] ? fallbackLang : fallbackLanguageTags[0] ?? fallbackLang;
  if (!lang) return fallback;

  const requested = lang.trim();
  if (!requested) return fallback;
  const exact = fallbackLanguageTags.find((tag) => tag.toLowerCase() === requested.toLowerCase());
  if (exact) return exact;

  const primary = requested.split("-")[0]?.toLowerCase();
  if (primary) {
    const byPrimary = fallbackLanguageTags.find((tag) => tag.split("-")[0]?.toLowerCase() === primary);
    if (byPrimary) return byPrimary;
  }
  return fallback;
}

function applyDocumentLanguage(lang: string): void {
  if (typeof document === "undefined") return;
  document.documentElement.lang = lang;
}

async function fallbackTable(lang: string): Promise<Record<string, string>> {
  if (fallbackTables[lang]) return fallbackTables[lang];
  const loader = fallbackLoaders[lang] ?? fallbackLoaders[normalizedLang(null)];
  if (!loader) return {};
  try {
    const loaded = (await loader()).default;
    fallbackTables[lang] = loaded;
    return loaded;
  } catch {
    if (lang !== fallbackLang) return fallbackTable(fallbackLang);
    return {};
  }
}

/** Built-in locale files available to the no-backend dev preview. */
export async function listBundledLanguages(): Promise<Array<{ tag: string; name: string }>> {
  const languages = await Promise.all(
    fallbackLanguageTags.map(async (tag) => {
      const table = await fallbackTable(tag);
      return { tag, name: table["meta.name"] ?? tag };
    }),
  );
  return languages.sort((a, b) => a.tag.localeCompare(b.tag));
}

/** Loads the locale table (explicit tag, or backend-resolved default). */
export async function loadLocale(lang?: string | null): Promise<void> {
  try {
    const res = await ipc.getLocaleTable(lang ?? null);
    store.lang = res.lang;
    store.table = res.table;
    applyDocumentLanguage(res.lang);
    store.ready = true;
  } catch {
    const fallbackLang = normalizedLang(lang);
    store.lang = fallbackLang;
    store.table = await fallbackTable(fallbackLang);
    applyDocumentLanguage(fallbackLang);
    store.ready = true;
  }
}

/** Current language tag. */
export function currentLang(): string {
  return store.lang;
}

/** Whether the table has been loaded. */
export function i18nReady(): boolean {
  return store.ready;
}

/** Translates a key, substituting `{name}` placeholders. */
export function t(key: string, params?: Record<string, string | number>): string {
  let out = store.table[key] ?? key;
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      out = out.split(`{${name}}`).join(String(value));
    }
  }
  return out;
}

/** Renders a structured backend error through the locale table. */
export function tError(e: ErrorDto): string {
  return t(e.key, e.params);
}
