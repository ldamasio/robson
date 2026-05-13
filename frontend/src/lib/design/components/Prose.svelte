<script lang="ts">
  import type { Snippet } from 'svelte';
  let { maxWidth = 'var(--prose-w)', children }: { maxWidth?: string; children: Snippet } = $props();
</script>

<div class="prose" style:--prose-max-width={maxWidth}>
  {@render children()}
</div>

<style>
  .prose {
    max-width: var(--prose-max-width);
    margin-inline: auto;
    line-height: var(--lead-body);
  }
</style>
