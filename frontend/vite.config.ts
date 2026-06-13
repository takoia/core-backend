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
    host: process.env.VITE_DEV_HOST || "127.0.0.1",
    // When served behind Caddy on the VM, allow the public host and point the
    // HMR websocket at it (wss on 443). Plain local dev leaves these defaults.
    allowedHosts: process.env.VITE_ALLOWED_HOST ? [process.env.VITE_ALLOWED_HOST] : true,
    hmr: process.env.VITE_HMR_HOST
      ? { host: process.env.VITE_HMR_HOST, protocol: "wss", clientPort: 443 }
      : true,
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
