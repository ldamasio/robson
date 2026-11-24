<script lang="ts">
  import { goto } from "$app/navigation";
  import type { Snippet } from "svelte";
  import NavBar from "$design/components/NavBar.svelte";
  import RiskDisclaimer from "$design/components/RiskDisclaimer.svelte";
  import { session } from "$stores/auth";
  import { mutationPending } from "$stores/mutation";

  let { children }: { children: Snippet } = $props();

  $effect(() => {
    if ($session.initialized && !$session.authenticated) {
      void goto(
        `/login?redirect=${encodeURIComponent(window.location.pathname)}`,
      );
    }
  });
</script>

{#if $session.initialized && $session.authenticated}
  <!-- Guardrail 4: while any mutation is pending the whole surface, navigation
       included, is inert and covered, so nothing can be submitted twice. -->
  <div class="surface" inert={$mutationPending} aria-busy={$mutationPending}>
    <NavBar />
    {@render children()}
    <RiskDisclaimer />
  </div>
  {#if $mutationPending}
    <div class="mutation-freeze" role="status" aria-live="polite">
      PROCESSING…
    </div>
  {/if}
{/if}

<style>
  /* Wrapper only for inert/aria-busy: no box, so the layout is unchanged. */
  .surface {
    display: contents;
  }
  .mutation-freeze {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.35);
    cursor: wait;
    font-family: var(--font-mono);
    letter-spacing: var(--track-wide);
  }
</style>
