import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const rootDir = dirname(fileURLToPath(import.meta.url));

export default defineConfig(() => {
  return {
    root: rootDir,
    resolve: {
      alias: {
        'react-dom/test-utils': 'react',
      },
    },
    build: {
      outDir: 'build',
    },
    test: {
      environment: 'jsdom',
      deps: {
        inline: ['@testing-library/react'],
      },
    },
    plugins: [react()],
  };
});
