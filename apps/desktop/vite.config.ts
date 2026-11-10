import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // SharedArrayBuffer needs cross-origin isolation. tauri.conf.json sets the same
    // headers for the built app, but the dev server has to send them itself.
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
  },
  build: {
    target: 'es2023',
    // The app's interface, React and React Aria, is one chunk of about 600 kB, loaded only by the
    // app; the reader and the benchmark pages never load it.
    chunkSizeWarningLimit: 800,
    rollupOptions: {
      input: { main: 'index.html', reader: 'reader.html' },
    },
  },
  worker: { format: 'es' },
});
