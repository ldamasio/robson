<script lang="ts">
  import type { Snippet } from 'svelte';
  let { cols = 12, gap = 5, children }: { cols?: number; gap?: number; children: Snippet } = $props();
  const gapVar = `var(--s-${gap})`;
</script>

<div
  class="grid"
  style:--grid-cols="repeat({cols}, minmax(0, 1fr))"
  style:--grid-gap={gapVar}
>
  {@render children()}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: var(--grid-cols);
    gap: var(--grid-gap);
  }
</style>
