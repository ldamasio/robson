<script lang="ts">
  import '$design/tokens.css';
  import { initAuth } from '$stores/auth';
  import { browser } from '$app/environment';

  import type { Snippet } from 'svelte';
  let { children }: { children: Snippet } = $props();

  if (browser) initAuth();
</script>

<main class="rbx-root">
  {@render children()}
</main>

<style>
  :global(html, body) {
    margin: 0;
    padding: 0;
    background: var(--bg-0);
    color: var(--fg-0);
    font-family: var(--font-sans);
    font-size: var(--text-base);
    line-height: var(--lead-body);
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
  }
  :global(*, *::before, *::after) {
    box-sizing: border-box;
  }
  :global(h1, h2, h3, h4, h5, h6) {
    margin: 0;
    color: var(--fg-0);
  }
  :global(p) {
    margin: 0;
    color: var(--fg-1);
  }
  :global(a) {
    color: var(--fg-0);
    text-decoration: none;
    border-bottom: 1px solid var(--border-strong);
    transition: border-color var(--dur) var(--ease), color var(--dur) var(--ease);
  }
  :global(a:hover) {
    color: var(--cyan-brand);
    border-bottom-color: var(--cyan-brand);
  }
  :global(*:focus-visible) {
    outline: 2px solid var(--cyan-brand);
    outline-offset: 2px;
  }
  .rbx-root {
    min-height: 100vh;
    background: var(--bg-0);
  }
</style>
