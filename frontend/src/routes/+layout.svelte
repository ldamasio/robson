<script lang="ts">
  import "$design/tokens.css";
  import { expireAuthIfNeeded, initAuth } from "$stores/auth";
  import {
    connectivity,
    refreshConnectivity,
    startConnectivity,
    stopConnectivity,
  } from "$stores/connectivity";
  import "$lib/i18n";
  import {
    isAndroidNativeApp,
    leaveAndroidApp,
  } from "$lib/mobile/appLifecycle";
  import { browser } from "$app/environment";
  import { onMount } from "svelte";
  import { _ } from "svelte-i18n";

  import type { Snippet } from "svelte";
  let { children }: { children: Snippet } = $props();
  let androidNativeApp = $state(false);
  let leavingApp = $state(false);

  if (browser) initAuth();

  onMount(() => {
    let mounted = true;
    const expireWhenVisible = () => {
      if (document.visibilityState === "visible") {
        expireAuthIfNeeded();
        refreshConnectivity();
      }
    };
    void isAndroidNativeApp().then((isAndroid) => {
      if (mounted) androidNativeApp = isAndroid;
    });
    startConnectivity();
    expireAuthIfNeeded();
    document.addEventListener("visibilitychange", expireWhenVisible);
    window.addEventListener("pageshow", expireWhenVisible);
    return () => {
      mounted = false;
      stopConnectivity();
      document.removeEventListener("visibilitychange", expireWhenVisible);
      window.removeEventListener("pageshow", expireWhenVisible);
    };
  });

  async function leaveApp() {
    if (leavingApp) return;
    leavingApp = true;
    try {
      await leaveAndroidApp();
    } finally {
      leavingApp = false;
    }
  }
</script>

{#if $connectivity.initialized && !$connectivity.online}
  <div
    class="network-banner"
    class:native-app={androidNativeApp}
    role="status"
    aria-live="polite"
  >
    {$_("network.offline")}
  </div>
{/if}

{#if androidNativeApp}
  <button
    class="leave-app"
    type="button"
    aria-label={$_("auth.leaveApp")}
    title={$_("auth.leaveApp")}
    disabled={leavingApp}
    onclick={leaveApp}
  >
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="M6 6l12 12M18 6L6 18" />
    </svg>
  </button>
{/if}

<main
  class="rbx-root"
  class:network-offline={$connectivity.initialized && !$connectivity.online}
  class:native-app={androidNativeApp}
>
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
  .network-banner {
    position: sticky;
    top: 0;
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
    height: 2.25rem;
    padding: 0 var(--s-4);
    border-bottom: 1px solid var(--warn);
    background: var(--bg-1);
    color: var(--warn);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-weight: 700;
    letter-spacing: var(--track-label);
    text-align: center;
    white-space: nowrap;
  }
  .network-banner.native-app {
    justify-content: flex-start;
    padding-right: 4.5rem;
  }
  .leave-app {
    position: fixed;
    top: calc(env(safe-area-inset-top) + var(--s-2));
    right: calc(env(safe-area-inset-right) + var(--s-3));
    z-index: 300;
    display: grid;
    place-items: center;
    width: 3.2rem;
    height: 3.2rem;
    padding: 0;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--bg-2);
    color: var(--fg-1);
    cursor: pointer;
  }
  .leave-app:hover {
    border-color: var(--cyan-muted);
    color: var(--cyan-brand);
  }
  .leave-app:disabled {
    cursor: default;
    opacity: 0.5;
  }
  .leave-app svg {
    width: 1.25rem;
    height: 1.25rem;
    fill: none;
    stroke: currentColor;
    stroke-linecap: square;
    stroke-width: 1.5;
  }
  .rbx-root.native-app :global(.navbar) {
    padding-right: 4.75rem;
  }
  .rbx-root.network-offline :global(.navbar) {
    top: 2.25rem;
  }
</style>
