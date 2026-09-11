<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  import { PUBLIC_GOOGLE_WEB_CLIENT_ID } from '$env/static/public';
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

  onMount(() => {
    if (!browser) return;

    // Loaded client-side only (onMount never runs during SSR/prerender),
    // so this never touches the static build.
    const script = document.createElement('script');
    script.src = 'https://accounts.google.com/gsi/client';
    script.async = true;
    script.defer = true;
    script.onload = () => {
      if (!window.google?.accounts?.id || !buttonContainer) return;
      window.google.accounts.id.initialize({
        client_id: PUBLIC_GOOGLE_WEB_CLIENT_ID,
        callback: handleCredentialResponse,
      });
      window.google.accounts.id.renderButton(buttonContainer, {
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
