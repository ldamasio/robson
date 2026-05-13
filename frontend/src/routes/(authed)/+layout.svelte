<script lang="ts">
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  import type { Snippet } from 'svelte';

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
  {@render children()}
{/if}
