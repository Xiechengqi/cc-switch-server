import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: process.env.WEB_DIST_DIR || "../web-dist",
    emptyOutDir: true,
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
    },
  },
  clearScreen: false,
});
