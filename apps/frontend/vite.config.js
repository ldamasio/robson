import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'url';
import { dirname, resolve as pathResolve } from 'path';

const rootDir = dirname(fileURLToPath(import.meta.url));

export default defineConfig(() => {
  return {
    root: rootDir,
    resolve: {
      alias: {
        'react-dom/test-utils': pathResolve(
          rootDir,
          'tests',
          'react-dom-test-utils.js',
        ),
      },
    },
    server: {
      deps: {
        inline: ['@testing-library/react'],
      },
    },
    build: {
      outDir: 'build',
    },
    test: {
      environment: 'jsdom',
    },
    plugins: [react()],
  };
});
