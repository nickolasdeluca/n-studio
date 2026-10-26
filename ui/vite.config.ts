import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: "./",
  // Tauri expects a fixed port, and shows its own errors
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  // A desktop app loads from disk: one bundle is fine, whatever its size
  build: { target: "safari16", outDir: "dist", chunkSizeWarningLimit: 2048 },
});
