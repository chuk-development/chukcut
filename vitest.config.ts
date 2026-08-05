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
    // vitest's 5 s default is not enough for this suite, and the failure it
    // produces lies: eight tests across six files failed at 5.1–6.9 s with a
    // different set each run, which reads as flakiness in the components and
    // is not. Mounting the editor, the export dialog or the preview in jsdom
    // costs seconds on its own — whole files take 17–103 s here — so a test
    // that waits for anything after the mount is racing the timeout rather
    // than the code. Raise it, and let a genuine hang be caught by the fact
    // that 20 s is still far more than any of these need.
    testTimeout: 20000,
    hookTimeout: 20000,
  },
});
