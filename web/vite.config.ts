/// <reference types="vitest/config" />
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [react()],
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
