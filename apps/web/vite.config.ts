import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The development server exists for iterating on the layout; the shipping path is the built
// bundle served by the Rust listener itself (ADR 0028), so there is no production proxy here.
//
// /api, /img, and /media are proxied to the running listener. changeOrigin makes the Host the
// listener's own, and the Origin rewrite does the same for the browser's POST header: without both,
// the listener's rebinding and origin checks refuse the request, which is the checks working.
const target = process.env["PEREGRINE_API"] ?? "http://127.0.0.1:8787";

export default defineConfig({
  plugins: [react()],
  server: {
    host: true,
    port: 5174,
    proxy: Object.fromEntries(
      ["/api", "/img", "/media"].map((path) => [
        path,
        {
          target,
          changeOrigin: true,
          configure: (proxy) => {
            proxy.on("proxyReq", (request) => {
              request.setHeader("origin", target);
            });
          },
        },
      ]),
    ),
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
