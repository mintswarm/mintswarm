import { defineConfig } from "vitest/config";

// Paths are relative to the Vite root, web/apps/ui: build output and caches go under the
// repository root's build/ tree, never into the source tree.
export default defineConfig({
  cacheDir: "../../../build/cache/vite",
  esbuild: { jsx: "automatic" },
  build: { outDir: "../../../build/web/ui", emptyOutDir: true },
  test: { environment: "jsdom" },
});
