/// <reference types="vitest/config" />
import path from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  // One .env at the repository root serves both the services and the web app.
  const env = loadEnv(mode, path.resolve(__dirname, "../.."), ["VITE_"]);
  const apiTarget = env.VITE_API_PROXY_TARGET ?? "http://127.0.0.1:8080";
  return {
    envDir: path.resolve(__dirname, "../.."),
    plugins: [react()],
    resolve: { alias: { "@": path.resolve(__dirname, "src") } },
    server: {
      port: 5173,
      strictPort: true,
      // Same-origin in development: the browser talks to Vite, Vite talks to the gateway.
      proxy: {
        "/api": { target: apiTarget, changeOrigin: false },
        "/rt": { target: apiTarget, changeOrigin: false, ws: true },
      },
    },
    build: {
      sourcemap: true,
      rollupOptions: {
        output: {
          manualChunks: {
            react: ["react", "react-dom", "react-router-dom"],
            query: ["@tanstack/react-query", "zustand"],
          },
        },
      },
    },
    test: {
      globals: true,
      environment: "jsdom",
      setupFiles: ["./tests/setup.ts"],
      include: ["src/**/*.test.{ts,tsx}", "tests/unit/**/*.test.{ts,tsx}"],
      css: false,
    },
  };
});
