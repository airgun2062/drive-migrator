import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// https://vite.dev/config/
// Tauri-specific settings: fixed port so tauri.conf.json's devUrl matches,
// and ignore the sibling Rust crate so its rebuilds don't trigger Vite HMR.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/crates/gui/**'],
    },
  },
})
