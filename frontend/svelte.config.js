import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
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
    }
  }
};

export default config;
