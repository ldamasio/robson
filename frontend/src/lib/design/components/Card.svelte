<script lang="ts">
  import type { Snippet } from 'svelte';
  let { padding = 5, children }: { padding?: number; children: Snippet } = $props();
  const padVar = `var(--s-${padding})`;
</script>

<div class="card" style:--card-pad={padVar}>
  {@render children()}
</div>

<style>
  .card {
    position: relative;
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: var(--card-pad);
    transition: border-color var(--dur) var(--ease);
  }
  .card:hover {
    border-color: var(--border-strong);
  }
  /* L-corner signature */
  .card::before,
  .card::after {
    content: '';
    position: absolute;
    width: 10px;
    height: 10px;
    border: 1.5px solid var(--cyan-brand);
    opacity: 0.4;
    pointer-events: none;
    transition: opacity var(--dur) var(--ease);
  }
  .card::before {
    top: -1px;
    left: -1px;
    border-right: 0;
    border-bottom: 0;
  }
  .card::after {
    top: -1px;
    right: -1px;
    border-left: 0;
    border-bottom: 0;
  }
  .card:hover::before,
  .card:hover::after {
    opacity: 0.8;
  }
</style>
