import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      awaitWriteFinish: { stabilityThreshold: 200, pollInterval: 50 },
      ignored: ["**/test-results/**", "**/playwright-report/**"],
    },
    proxy: {
      "/api": { target: "http://127.0.0.1:8080", ws: true },
      "/mcp": "http://127.0.0.1:8080",
    },
  },
  build: {
    chunkSizeWarningLimit: 1200,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom", "react-router-dom"],
          antd: ["antd", "@ant-design/icons"],
          charts: ["echarts"],
          terminal: ["@xterm/xterm", "@xterm/addon-fit"],
        },
      },
    },
  },
});
