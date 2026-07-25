import path from "node:path";
import { defineConfig } from "vitest/config";

// The suite reaches into `@/` the same way the app does, so the alias has to be
// the one from `vite.config.ts`. Duplicated rather than imported because that
// config is an async factory wrapped around Tauri's dev-server settings, and
// none of that belongs in a test run.
export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // Module-level singletons (the preview session, the thumbnail queue) make
    // cross-file leakage possible, so each file gets its own environment.
    isolate: true,
    restoreMocks: true,
  },
});
