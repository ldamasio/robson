<script lang="ts">
  import { goto } from '$app/navigation';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import { setToken } from '$stores/auth';
  import { robsonApi } from '$api/robson';

  let tokenInput = $state('');
  let error = $state('');
  let loading = $state(false);

  async function handleLogin() {
    error = '';
    loading = true;
    try {
      setToken(tokenInput.trim());
      await robsonApi.health();
      const params = new URLSearchParams(window.location.search);
      const redirect = params.get('redirect') ?? '/dashboard';
      void goto(redirect);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Connection failed';
    } finally {
      loading = false;
    }
  }
</script>

<svelte:head>
  <title>Login — RBX Robson</title>
</svelte:head>

<div class="login-page">
  <Card padding={7}>
    <Stack gap={5}>
      <img src="/brand/rbx-mark.svg" alt="RBX" width="48" height="48" />
      <h1>Robson</h1>
      <p>Enter your API token to access the operations console.</p>
      <form onsubmit={(e) => { e.preventDefault(); handleLogin(); }}>
        <Stack gap={3}>
          <input
            type="password"
            bind:value={tokenInput}
            placeholder="Bearer token"
            autocomplete="off"
            disabled={loading}
          />
          {#if error}
            <p class="error">{error}</p>
          {/if}
          <button class="btn-primary" type="submit" disabled={!tokenInput.trim() || loading}>
            {loading ? 'Connecting...' : 'Connect'}
          </button>
        </Stack>
      </form>
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
  input {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    width: 100%;
    padding: var(--s-3);
    background: var(--bg-1);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--fg-0);
  }
  input:focus {
    border-color: var(--cyan-brand);
    outline: none;
  }
  .error {
    color: var(--fg-error, #ff4444);
    font-size: var(--text-sm);
  }
  .btn-primary {
    font-family: var(--font-sans);
    font-size: var(--text-base);
    font-weight: 500;
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--cyan-signal);
    background: var(--cyan-signal);
    color: var(--bg-0);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background var(--dur) var(--ease), border-color var(--dur) var(--ease);
  }
  .btn-primary:hover:not(:disabled) {
    background: var(--cyan-brand);
    border-color: var(--cyan-brand);
  }
  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
