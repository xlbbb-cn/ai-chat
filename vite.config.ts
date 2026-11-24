import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Three entry points: the app, the desktop notification card window
  // (`notify.html` → `src/notify.tsx`, see `src-tauri/src/notify.rs`) and the
  // plugin manager window (`plugin-manager.html` → `src/plugin-manager.tsx`,
  // see `src-tauri/src/plugins/manager.rs`). Without the explicit input list
  // rollup would only see `index.html`.
  build: {
    rollupOptions: {
      input: {
        main: "index.html",
        notify: "notify.html",
        "plugin-manager": "plugin-manager.html",
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
