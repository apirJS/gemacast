import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;
const port = process.env.PORT;

export default defineConfig(async () => {
  return {
    plugins: [react(), tailwindcss()],
    clearScreen: false,
    // minSdk 26 + an auto-updating Android System WebView: target modern JS so
    // Vite skips legacy transpilation/polyfills. Drop console/debugger from the
    // production bundle.
    build: {
      target: 'esnext',
    },
    esbuild: {
      drop: ['console', 'debugger'],
    },
    server: {
      port: port ? parseInt(port) : 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: 'ws',
            host,
            port: 1421,
          }
        : undefined,
      watch: {
        ignored: ['**/src-tauri/**'],
      },
    },
  };
});
