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
        // Defaults to a local backend; set VITE_API_TARGET to proxy a remote
        // one (e.g. the VM) for HMR against real data.
        target: process.env.VITE_API_TARGET || "http://127.0.0.1:8080",
        changeOrigin: true,
        secure: true,
        // SSE endpoints must not be buffered.
        ws: false,
      },
    },
  },
});
