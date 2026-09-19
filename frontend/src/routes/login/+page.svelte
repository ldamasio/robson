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

  // Google Identity Services honours `theme` on its standard button, but
  // not on the "personalized" variant it swaps in when a Google session
  // already exists ("Entrar como <nome>"): that one comes back carrying
  // the default light classes instead of the filled_black ones we asked
  // for, so it rendered as a white pill on our dark card. Re-apply
  // Google's own dark classes instead of hand-rolling colours, so the
  // hover/active states and the dimmer second line (the e-mail) all come
  // from the theme we requested. If Google ever renames these, the button
  // simply falls back to its light styling - nothing breaks.
  const GIS_DARK_CLASSES = ['MFS4be-JaPV2b-Ia7Qfc', 'MFS4be-Ia7Qfc'];
  const GIS_LIGHT_CLASSES = ['i5vt6e-Ia7Qfc', 'i5vt6e-to915-Ia7Qfc'];

  function enforceDarkTheme(container: HTMLElement) {
    const button = container.querySelector<HTMLElement>(
      '[role="button"][aria-labelledby="button-label"]',
    );
    if (!button || button.classList.contains(GIS_DARK_CLASSES[0])) return;
    button.classList.remove(...GIS_LIGHT_CLASSES);
    button.classList.add(...GIS_DARK_CLASSES);
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

    let observer: MutationObserver | undefined;

    // Loaded client-side only ($effect bodies never run during
    // SSR/prerender), so this never touches the static build.
    const script = document.createElement('script');
    script.src = 'https://accounts.google.com/gsi/client';
    script.async = true;
    script.defer = true;
    script.onload = () => {
      const container = buttonContainer;
      if (!window.google?.accounts?.id || !container) return;
      window.google.accounts.id.initialize({
        client_id: env.PUBLIC_GOOGLE_WEB_CLIENT_ID ?? '',
        callback: handleCredentialResponse,
      });
      // GIS re-renders the button on its own (e.g. once it resolves the
      // signed-in account), so watch the container rather than patching
      // a single render.
      observer = new MutationObserver(() => enforceDarkTheme(container));
      observer.observe(container, { childList: true, subtree: true });
      window.google.accounts.id.renderButton(container, {
        type: 'standard',
        theme: 'filled_black',
        size: 'large',
        text: 'signin_with',
        shape: 'rectangular',
      });
      enforceDarkTheme(container);
    };
    script.onerror = () => {
      error = $_('login.connectionFailed');
    };
    document.head.appendChild(script);

    return () => {
      observer?.disconnect();
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
