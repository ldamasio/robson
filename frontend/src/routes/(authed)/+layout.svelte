<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  // ADR-0054: dynamic, not static, public env — see robson.ts for why.
  import { env } from '$env/dynamic/public';
  import type { Snippet } from 'svelte';
  import NavBar from '$design/components/NavBar.svelte';
  import RiskDisclaimer from '$design/components/RiskDisclaimer.svelte';
  import { authToken, isNearExpiry, silentRefreshGoogleToken } from '$stores/auth';

  let { children }: { children: Snippet } = $props();
  let checked = $state(false);
  let hasToken = $state(false);

  // ADR-0054: reactive to the auth store (not a one-shot sessionStorage
  // read under the old `robson_api_token` key) so a token cleared
  // mid-session — e.g. by `apiFetch`'s 401 handling when a silent refresh
  // fails — redirects to /login without waiting for a full navigation to
  // re-evaluate this guard.
  const unsubscribeAuth = authToken.subscribe((token) => {
    hasToken = Boolean(token);
    checked = true;
  });

  // Periodic silent-refresh: every few minutes, check whether the cached
  // Google ID token is close to expiry and, if so, refresh it in the
  // background (no visible click) so an open tab doesn't hit a hard 401
  // mid-action. If the silent refresh fails, the next API call's own
  // 401 handling (see `apiFetch`) takes over and redirects to /login.
  const REFRESH_CHECK_INTERVAL_MS = 3 * 60 * 1000;
  let refreshTimer: ReturnType<typeof setInterval> | undefined;

  /** Read the store's current value synchronously (subscribe + immediately
   *  unsubscribe) without needing a component-level reactive binding. */
  function currentToken(): string | null {
    let value: string | null = null;
    const unsub = authToken.subscribe((t) => (value = t));
    unsub();
    return value;
  }

  onMount(() => {
    if (!browser) return;
    refreshTimer = setInterval(() => {
      const token = currentToken();
      if (token && isNearExpiry(token)) {
        void silentRefreshGoogleToken(env.PUBLIC_GOOGLE_WEB_CLIENT_ID ?? '');
      }
    }, REFRESH_CHECK_INTERVAL_MS);
  });

  onDestroy(() => {
    unsubscribeAuth();
    if (refreshTimer) clearInterval(refreshTimer);
  });

  $effect(() => {
    if (checked && !hasToken) {
      void goto(`/login?redirect=${encodeURIComponent(window.location.pathname)}`);
    }
  });
</script>

{#if checked && hasToken}
  <NavBar />
  {@render children()}
  <RiskDisclaimer />
{/if}
