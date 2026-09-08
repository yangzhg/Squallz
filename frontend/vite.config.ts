import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

const targetPlatform =
  process.env.SQUALLZ_TARGET_PLATFORM ??
  (process.platform === "darwin"
    ? "macos"
    : process.platform === "win32"
      ? "windows"
      : "linux");

// Vite config tuned for Tauri: fixed port, no auto-open, target per WebKit.
export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  clearScreen: false,
  define: {
    __SQUALLZ_TARGET_PLATFORM__: JSON.stringify(targetPlatform),
  },
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "safari15",
    outDir: "dist",
  },
});
