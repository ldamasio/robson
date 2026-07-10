<script lang="ts">
  import { _ } from 'svelte-i18n';
</script>

<footer class="risk-disclaimer" role="note" aria-label={$_('disclaimer.label')}>
  <span class="label">{$_('disclaimer.label')}</span>
  <p class="body">{$_('disclaimer.body')}</p>
</footer>

<style>
  .risk-disclaimer {
    margin: var(--s-6) var(--s-5) var(--s-4);
    padding: var(--s-3) var(--s-4);
    background: var(--warn-subtle);
    border: 1px solid var(--border);
    border-left: 2px solid var(--warn);
    border-radius: 4px;
    display: flex;
    gap: var(--s-3);
    align-items: flex-start;
    box-sizing: border-box;
  }

  .label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--warn);
    white-space: nowrap;
    padding-top: 2px;
  }

  .body {
    margin: 0;
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--fg-1);
    max-width: 80ch;
  }
</style>
