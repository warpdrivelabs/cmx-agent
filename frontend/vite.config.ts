import { defineConfig } from "vite";

export default defineConfig({
  base: "./",

  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false
  },

  server: {
    host: "127.0.0.1",
    port: 5174,
    strictPort: true,
    proxy: {
      "/api": {
        target: process.env.CMX_AGENT_WEB_ORIGIN ?? "http://127.0.0.1:18080",
        changeOrigin: true
      }
    }
  }
});
