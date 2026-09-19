<script lang="ts">
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  // ADR-0054: dynamic, not static, public env — see robson.ts for why.
  import { env } from '$env/dynamic/public';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import { setToken } from '$stores/auth';
  import { robsonApi } from '$api/robson';
  import { _ } from 'svelte-i18n';
  import { watchGisTheme } from '$lib/utils/gis-theme';

  let error = $state('');
  let loading = $state(false);
  let buttonContainer: HTMLDivElement | undefined = $state();

  // ADR-0054: Google Identity Services (GIS) replaces the pasted-token
  // form. GIS runs entirely client-side — it hands the ID token straight
  // to this callback — so there is no OAuth callback server involved,
  // which is exactly the constraint ADR-0025 amends around (see
  // docs/adr/ADR-0025-frontend-auth-bearer-token.md and the new ADR).
  async function handleCredentialResponse(response: { credential?: string }) {
    error = '';
    if (!response.credential) {
      error = $_('login.connectionFailed');
      return;
    }
    loading = true;
    try {
      setToken(response.credential);
      await robsonApi.health();
      const params = new URLSearchParams(window.location.search);
      const redirect = params.get('redirect') ?? '/dashboard';
      void goto(redirect);
    } catch (e) {
      error = e instanceof Error && e.message ? e.message : $_('login.connectionFailed');
    } finally {
      loading = false;
    }
  }

  // NOTE: `$effect`, not `onMount`. In this component (and reproduced in
  // an isolated, from-scratch test route too), Rollup's tree-shaking
  // silently removed the equivalent `onMount(...)` callback from the
  // shipped production bundle — no build error, the button just never
  // rendered. Disabling Rollup's treeshake entirely made it survive,
  // confirming tree-shaking as the mechanism, but none of Rollup's
  // documented, targeted `treeshake` sub-options fixed it, and why some
  // other `onMount` usages in this app are/aren't affected is not fully
  // understood. `$effect` is confirmed present in the built output and
  // working end-to-end (real browser, real GIS button) — don't change
  // this back to `onMount` without re-verifying the compiled bundle.
  $effect(() => {
    if (!browser) return;

    // The script can finish loading after this effect is torn down (a fast
    // navigation away from /login), and removing an already-fetching script
    // does not reliably cancel its load event. Without this flag we would
    // initialize GIS and start observers on a detached container, with no
    // cleanup left to stop them.
    let disposed = false;
    let unwatch: (() => void) | undefined;

    // Loaded client-side only ($effect bodies never run during
    // SSR/prerender), so this never touches the static build.
    const script = document.createElement('script');
    script.src = 'https://accounts.google.com/gsi/client';
    script.async = true;
    script.defer = true;
    script.onload = () => {
      const container = buttonContainer;
      if (disposed || !window.google?.accounts?.id || !container) return;
      window.google.accounts.id.initialize({
        client_id: env.PUBLIC_GOOGLE_WEB_CLIENT_ID ?? '',
        callback: handleCredentialResponse,
      });
      // Keeps the requested filled_black theme on whichever button variant
      // GIS renders, now and on its later re-renders. See gis-theme.ts.
      unwatch = watchGisTheme(container);
      window.google.accounts.id.renderButton(container, {
        type: 'standard',
        theme: 'filled_black',
        size: 'large',
        text: 'signin_with',
        shape: 'rectangular',
      });
    };
    script.onerror = () => {
      error = $_('login.connectionFailed');
    };
    document.head.appendChild(script);

    return () => {
      disposed = true;
      script.onload = null;
      script.onerror = null;
      unwatch?.();
      script.remove();
    };
  });
</script>

<svelte:head>
  <title>{$_('login.pageTitle')}</title>
</svelte:head>

<div class="login-page">
  <Card padding={7}>
    <Stack gap={5}>
      <img src="/brand/rbx-mark.svg" alt="RBX" width="48" height="48" />
      <h1>{$_('login.title')}</h1>
      <p>{$_('login.description')}</p>
      <div class="google-button" bind:this={buttonContainer} aria-live="polite"></div>
      {#if loading}
        <p class="status">{$_('login.signingIn')}</p>
      {/if}
      {#if error}
        <p class="error">{error}</p>
      {/if}
    </Stack>
  </Card>
</div>

<style>
  .login-page {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: var(--s-5);
  }
  h1 {
    font-size: var(--text-3xl);
    font-weight: 300;
  }
  p {
    color: var(--fg-1);
  }
  .google-button {
    min-height: 44px;
    display: flex;
    justify-content: flex-start;
  }
  .status {
    font-size: var(--text-sm);
  }
  .error {
    color: var(--fg-error, #ff4444);
    font-size: var(--text-sm);
  }
</style>
