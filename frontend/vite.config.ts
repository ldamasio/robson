import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 5173,
    strictPort: false
  },
  test: {
    include: ['tests/unit/**/*.{test,spec}.{js,ts}'],
    environment: 'node'
  }
});
