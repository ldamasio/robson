<script lang="ts">
  import type { Snippet } from 'svelte';
  let { gap = 4, align = 'stretch', children }: { gap?: number; align?: 'start' | 'center' | 'end' | 'stretch'; children: Snippet } = $props();
  const gapVar = `var(--s-${gap})`;
</script>

<div class="stack" style:--stack-gap={gapVar} style:--stack-align={align}>
  {@render children()}
</div>

<style>
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--stack-gap);
    align-items: var(--stack-align);
  }
</style>
