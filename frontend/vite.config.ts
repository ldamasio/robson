import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [
    sveltekit({
      preprocess: vitePreprocess(),
      adapter: adapter({
        pages: 'build',
        assets: 'build',
        fallback: 'index.html',
        precompress: false,
        strict: false
      }),
      alias: {
        '$design': 'src/lib/design',
        '$api': 'src/lib/api',
        '$stores': 'src/lib/stores',
        '$components': 'src/lib/components',
        '$icons': 'src/lib/icons',
        '$i18n': 'src/lib/i18n'
      },
      version: {
        pollInterval: 0
      }
    })
  ],
  server: {
    port: 5173,
    strictPort: false
  },
  test: {
    include: ['tests/unit/**/*.{test,spec}.{js,ts}'],
    environment: 'node'
  }
});
