<script lang="ts">
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  import type { Snippet } from 'svelte';
  import NavBar from '$design/components/NavBar.svelte';
  import RiskDisclaimer from '$design/components/RiskDisclaimer.svelte';

  let { children }: { children: Snippet } = $props();
  let checked = $state(false);
  let hasToken = $state(false);

  if (browser) {
    hasToken = Boolean(sessionStorage.getItem('robson_api_token'));
    checked = true;
  }

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
