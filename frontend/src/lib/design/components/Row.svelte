<script lang="ts">
  import type { Snippet } from 'svelte';
  let { gap = 4, align = 'center', justify = 'start', children }: { gap?: number; align?: 'start' | 'center' | 'end' | 'baseline'; justify?: 'start' | 'center' | 'end' | 'between' | 'around'; children: Snippet } = $props();
  const gapVar = `var(--s-${gap})`;
  const justifyMap = {
    start: 'flex-start',
    center: 'center',
    end: 'flex-end',
    between: 'space-between',
    around: 'space-around'
  };
</script>

<div
  class="row"
  style:--row-gap={gapVar}
  style:--row-align={align}
  style:--row-justify={justifyMap[justify]}
>
  {@render children()}
</div>

<style>
  .row {
    display: flex;
    flex-direction: row;
    gap: var(--row-gap);
    align-items: var(--row-align);
    justify-content: var(--row-justify);
  }
</style>
