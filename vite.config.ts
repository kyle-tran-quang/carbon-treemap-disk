import path from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
// @ts-expect-error type error without @types/node package
import process from 'node:process';

const host = process.env.TAURI_DEV_HOST;

// Resolve Carbon's ~@ibm/plex tilde paths to the actual node_modules location
const plexPath = path.resolve(
  __dirname,
  'node_modules/.pnpm/@ibm+plex@6.4.1/node_modules/@ibm/plex',
);

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  css: {
    preprocessorOptions: {
      scss: {
        // Silence Carbon's internal Sass deprecation warnings (upstream issue)
        silenceDeprecations: ['if-function'],
      },
    },
  },

  resolve: {
    alias: {
      // Allow Carbon SCSS to resolve ~@ibm/plex/** font references
      '~@ibm/plex': plexPath,
    },
  },

  // Vite options tailored for Tauri development
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. Tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
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
      // 3. Tell Vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**'],
    },
  },
}));
