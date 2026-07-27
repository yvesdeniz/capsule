import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Tauri drives this dev server; the fixed port and strictPort matter because
// tauri.conf.json points at it explicitly.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Never watch the Rust build tree: cargo rewrites the exe while Vite has
      // it open, which throws EBUSY and kills the dev server on Windows.
      ignored: ['**/src-tauri/**', '**/dist/**'],
    },
  },
  build: {
    // WebView2 is evergreen Chromium, so there is no reason to down-level.
    target: 'chrome120',
    sourcemap: true,
  },
})
