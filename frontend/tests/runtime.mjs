import { fileURLToPath } from "node:url";
import { createServer } from "vite";

export function createTestServer() {
  return createServer({
    appType: "custom",
    logLevel: "silent",
    root: fileURLToPath(new URL("../", import.meta.url)),
    server: { hmr: false, middlewareMode: true },
  });
}
