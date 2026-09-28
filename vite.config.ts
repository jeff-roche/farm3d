import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [solid()],

  build: {
    // The three.js chunk (~557 kB) is above Vite's 500 kB default. It is
    // loaded lazily, when a 3D viewport first mounts
    // (src/slicing/viewport/renderer-factory.ts). The main chunk is ~583 kB
    // (measured on this branch after P8's Attention/Incidents/Notifications
    // screens were split into their own lazy chunks -- AttentionCenter,
    // AttentionEventDetail, IncidentDetail, NotificationSettingsDialog --
    // up from ~545 kB before P8; getting it back under 500 kB would need
    // trimming pre-existing main-chunk code too, not just P8's additions).
    chunkSizeWarningLimit: 600,
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
