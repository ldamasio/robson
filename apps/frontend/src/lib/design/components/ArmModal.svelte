<script lang="ts">
  import { robsonApi } from '$api/robson';

  let { onclose }: { onclose: () => void } = $props();

  let symbol = $state('BTCUSDT');
  let side = $state<'Long' | 'Short'>('Long');
  let submitting = $state(false);
  let error = $state<string | null>(null);

  function enforceUppercase(e: Event) {
    const input = e.target as HTMLInputElement;
    symbol = input.value.toUpperCase();
  }

  async function submit() {
    error = null;
    if (!symbol.trim()) { error = 'SYMBOL REQUIRED'; return; }

    submitting = true;
    try {
      await robsonApi.armPosition({ symbol: symbol.trim(), side });
      onclose();
    } catch (e) {
      error = e instanceof Error ? e.message : 'ARM FAILED';
    } finally {
      submitting = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') onclose(); }} />

<div class="overlay" role="dialog" aria-modal="true">
  <div class="modal">
    <div class="modal-header">
      <span class="eyebrow">ARM POSITION</span>
      <button class="btn-close" onclick={onclose} aria-label="Close">ESC</button>
    </div>

    <div class="fields">
      <label class="field">
        <span class="label">SYMBOL</span>
        <input
          type="text"
          class="input"
          bind:value={symbol}
          oninput={enforceUppercase}
          placeholder="BTCUSDT"
        />
      </label>

      <div class="field">
        <span class="label">SIDE</span>
        <div class="toggle">
          <button
            class="toggle-btn"
            class:active={side === 'Long'}
            onclick={() => (side = 'Long')}
          >LONG</button>
          <button
            class="toggle-btn"
            class:active={side === 'Short'}
            onclick={() => (side = 'Short')}
          >SHORT</button>
        </div>
      </div>
    </div>

    {#if error}
      <p class="err-text">{error}</p>
    {/if}

    <button class="btn-submit" onclick={submit} disabled={submitting}>
      {submitting ? 'ARMING...' : 'ARM'}
    </button>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .modal {
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: var(--s-6);
    min-width: 340px;
    max-width: 420px;
    box-shadow: var(--shadow-overlay);
  }
  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--s-5);
  }
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .btn-close {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-3);
    background: transparent;
    border: none;
    cursor: pointer;
  }
  .btn-close:hover {
    color: var(--fg-1);
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--s-4);
    margin-bottom: var(--s-4);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--s-1);
  }
  .label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .input {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    letter-spacing: var(--track-wide);
    color: var(--fg-0);
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--s-2) var(--s-3);
    outline: none;
    text-transform: uppercase;
  }
  .input:focus {
    border-color: var(--border-accent);
  }
  .input::placeholder {
    color: var(--fg-3);
  }
  .toggle {
    display: flex;
    gap: var(--s-1);
  }
  .toggle-btn {
    flex: 1;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    background: var(--bg-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--s-2) var(--s-3);
    cursor: pointer;
    transition: color var(--dur) var(--ease), border-color var(--dur) var(--ease), background var(--dur) var(--ease);
  }
  .toggle-btn:hover {
    border-color: var(--border-strong);
  }
  .toggle-btn.active {
    color: var(--cyan-brand);
    border-color: var(--cyan-dim);
    background: var(--cyan-subtle);
  }
  .err-text {
    color: var(--err);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-wide);
    margin-bottom: var(--s-3);
  }
  .btn-submit {
    width: 100%;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--cyan-brand);
    background: transparent;
    border: 1px solid var(--cyan-dim);
    border-radius: var(--radius-sm);
    padding: var(--s-2) var(--s-4);
    cursor: pointer;
    transition: background var(--dur) var(--ease);
  }
  .btn-submit:hover:not(:disabled) {
    background: var(--cyan-subtle);
  }
  .btn-submit:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
