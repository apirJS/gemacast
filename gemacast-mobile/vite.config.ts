import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;
const port = process.env.PORT;

export default defineConfig(async () => {
  return {
    plugins: [react(), tailwindcss()],
    clearScreen: false,
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
