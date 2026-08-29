<script lang="ts">
  import { goto } from "$app/navigation";
  import { browser } from "$app/environment";
  import type { Snippet } from "svelte";
  import { authToken } from "$stores/auth";
  import NavBar from "$design/components/NavBar.svelte";
  import RiskDisclaimer from "$design/components/RiskDisclaimer.svelte";

  let { children }: { children: Snippet } = $props();
  let checked = $state(false);
  let hasToken = $derived(Boolean($authToken));

  if (browser) {
    checked = true;
  }

  $effect(() => {
    if (checked && !hasToken) {
      void goto(
        `/login?redirect=${encodeURIComponent(window.location.pathname)}`,
      );
    }
  });
</script>

{#if checked && hasToken}
  <NavBar />
  {@render children()}
  <RiskDisclaimer />
{/if}
