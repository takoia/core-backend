import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The frontend is served by the Rust binary in production (from dist/).
// In dev, Vite serves with HMR and proxies API + SSE calls to the backend so
// everything is same-origin from the browser's point of view.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
        // SSE endpoints must not be buffered.
        ws: false,
      },
    },
  },
});
