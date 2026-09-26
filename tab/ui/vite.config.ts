import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In development the Tab server runs separately; everything that is not the app goes to it.
const tab = process.env.TAB_DEV_SERVER ?? "http://127.0.0.1:3100";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { "/api": { target: tab, changeOrigin: false }, "/mcp": { target: tab, changeOrigin: false } },
  },
  build: { outDir: "dist", sourcemap: false },
});
