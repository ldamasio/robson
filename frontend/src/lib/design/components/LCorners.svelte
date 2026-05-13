<script lang="ts">
  import type { Snippet } from 'svelte';
  let { size = 10, color = 'var(--cyan-brand)', opacity = 0.4, children }: { size?: number; color?: string; opacity?: number; children: Snippet } = $props();
</script>

<span
  class="lcorners"
  style:--lc-size="{size}px"
  style:--lc-color={color}
  style:--lc-opacity={opacity}
>
  {@render children()}
</span>

<style>
  .lcorners {
    position: relative;
    display: block;
  }
  .lcorners::before,
  .lcorners::after,
  .lcorners > :global(*)::before,
  .lcorners > :global(*)::after {
    content: '';
    position: absolute;
    width: var(--lc-size);
    height: var(--lc-size);
    border: 1.5px solid var(--lc-color);
    opacity: var(--lc-opacity);
    pointer-events: none;
  }
  .lcorners::before {
    top: -1px;
    left: -1px;
    border-right: 0;
    border-bottom: 0;
  }
  .lcorners::after {
    top: -1px;
    right: -1px;
    border-left: 0;
    border-bottom: 0;
  }
</style>
