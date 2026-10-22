import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { readFileSync } from "node:fs";

const englishMessages = JSON.parse(readFileSync(new URL("../../locales/en-US.json", import.meta.url), "utf8"));
export const englishTranslate = (key, fallback) => fallback ?? englishMessages[key] ?? key;

export function createTestServer() {
  return createServer({
    appType: "custom",
    logLevel: "silent",
    root: fileURLToPath(new URL("../", import.meta.url)),
    server: { hmr: false, middlewareMode: true },
  });
}
