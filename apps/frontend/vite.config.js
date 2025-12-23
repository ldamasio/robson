import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const rootDir = dirname(fileURLToPath(import.meta.url));

export default defineConfig(() => {
  return {
    root: rootDir,
    build: {
      outDir: 'build',
    },
    test: {
      environment: 'jsdom',
      setupFiles: ['tests/vitest.setup.js'],
    },
    plugins: [react()],
  };
});
