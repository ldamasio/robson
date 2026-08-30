<script lang="ts">
  import "$design/tokens.css";
  import { expireAuthIfNeeded, initAuth } from "$stores/auth";
  import "$lib/i18n";
  import { browser } from "$app/environment";
  import { onMount } from "svelte";

  import type { Snippet } from "svelte";
  let { children }: { children: Snippet } = $props();

  if (browser) initAuth();

  onMount(() => {
    const expireWhenVisible = () => {
      if (document.visibilityState === "visible") expireAuthIfNeeded();
    };
    expireAuthIfNeeded();
    document.addEventListener("visibilitychange", expireWhenVisible);
    window.addEventListener("pageshow", expireWhenVisible);
    return () => {
      document.removeEventListener("visibilitychange", expireWhenVisible);
      window.removeEventListener("pageshow", expireWhenVisible);
    };
  });
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
  :global(body) {
    padding-top: env(safe-area-inset-top);
    padding-right: env(safe-area-inset-right);
    padding-bottom: env(safe-area-inset-bottom);
    padding-left: env(safe-area-inset-left);
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
    transition:
      border-color var(--dur) var(--ease),
      color var(--dur) var(--ease);
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
    min-height: 100dvh;
    background: var(--bg-0);
  }
</style>
