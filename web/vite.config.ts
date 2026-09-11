/// <reference types="vitest/config" />
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [react()],
  // The wasm-bindgen glues fetch their `<name>_bg.wasm` sidecar relative to
  // `import.meta.url`; under dep pre-bundling that URL lands in .vite/deps,
  // where the sidecar does not exist and the SPA fallback returns HTML.
  // Serving both packages as raw ESM keeps the URL pointing at the real file.
  optimizeDeps: {
    exclude: ['kaspa-wasm', 'vprog-tictactoe-encoder-wasm'],
  },
  server: {
    proxy: {
      // DA HTTP server (node crate, TT_DA_BIND default).
      '/api': 'http://127.0.0.1:9880',
    },
  },
  test: {
    environment: 'node',
  },
});
