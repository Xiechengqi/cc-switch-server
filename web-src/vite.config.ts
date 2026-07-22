import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: process.env.WEB_DIST_DIR || "../web-dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (
            id.includes("node_modules/codemirror") ||
            id.includes("node_modules/@codemirror")
          ) {
            return "codemirror";
          }
          if (
            id.includes("node_modules/recharts") ||
            id.includes("node_modules/d3-")
          ) {
            return "recharts";
          }
          if (id.includes("node_modules/framer-motion")) {
            return "framer-motion";
          }
          if (
            id.includes("/i18n/locales/") ||
            id.includes("/i18n/server-locales/")
          ) {
            return "locales";
          }
        },
      },
    },
  },
  server: {
    port: 15722,
    strictPort: false,
    proxy: {
      "/api": "http://127.0.0.1:15721",
      "/web-api": "http://127.0.0.1:15721",
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      "@tauri-apps/api/core": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/api/core.ts",
      ),
      "@tauri-apps/api/event": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/api/event.ts",
      ),
      "@tauri-apps/api/app": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/api/app.ts",
      ),
      "@tauri-apps/api/path": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/api/path.ts",
      ),
      "@tauri-apps/plugin-process": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/plugin-process.ts",
      ),
      "@tauri-apps/plugin-updater": path.resolve(
        __dirname,
        "./src/shims/tauri-apps/plugin-updater.ts",
      ),
    },
  },
  clearScreen: false,
});
