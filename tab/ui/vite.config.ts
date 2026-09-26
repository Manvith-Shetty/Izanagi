import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Dev only: the Tab Rust server (TAB_BIND, default 127.0.0.1:3000) answers /api and /mcp.
// In production Tab serves this build itself, so every request is same-origin.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const tab = env.TAB_DEV_PROXY || "http://127.0.0.1:3000";
  return {
    plugins: [react(), tailwindcss()],
    server: {
      port: 5173,
      strictPort: true,
      proxy: { "/api": { target: tab, changeOrigin: true }, "/mcp": { target: tab, changeOrigin: true } },
    },
  };
});
