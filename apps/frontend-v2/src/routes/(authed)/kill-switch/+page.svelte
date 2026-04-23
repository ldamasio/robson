<script lang="ts">
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';

  // FE-P1 stub — real wiring in EP-006.
  let currentState: 'ACTIVE' | 'DISABLED' = $state('ACTIVE');
  let input = $state('');
  const KEYWORD_DISABLE = 'DESLIGAR';
  const KEYWORD_ENABLE = 'RELIGAR';
  let targetKeyword = $derived(currentState === 'ACTIVE' ? KEYWORD_DISABLE : KEYWORD_ENABLE);
  let canConfirm = $derived(input === targetKeyword);

  function confirm() {
    if (!canConfirm) return;
    // EP-006: call robsonApi.toggleKillSwitch(...)
    currentState = currentState === 'ACTIVE' ? 'DISABLED' : 'ACTIVE';
    input = '';
  }
</script>

<svelte:head>
  <title>Kill Switch — RBX Robson</title>
</svelte:head>

<div class="ks-page">
  <Card padding={7}>
    <Stack gap={5}>
      <div class="eyebrow">KILL SWITCH</div>
      <h1>{currentState === 'ACTIVE' ? 'Desligar Robson' : 'Religar Robson'}</h1>

      <div class="current-state">
        <div class="eyebrow">ESTADO ATUAL</div>
        <div class="state-line">
          <span class="dot" class:active={currentState === 'ACTIVE'} class:disabled={currentState === 'DISABLED'}></span>
          ROBSON {currentState} · SLOT 4/6 · 2 POSIÇÕES ABERTAS
        </div>
      </div>

      {#if currentState === 'ACTIVE'}
        <p>Esta ação impede Robson de abrir novas posições. As posições existentes continuam sendo gerenciadas até fecharem naturalmente.</p>
        <p>A reativação será bloqueada por 5 minutos após a confirmação (cooldown imposto pelo backend).</p>
      {:else}
        <p>Reativar permite Robson abrir novas posições quando sinais de oportunidade chegarem.</p>
      {/if}

      <Stack gap={2}>
        <label class="eyebrow" for="confirm-input">
          Para confirmar, digite "{targetKeyword}" abaixo:
        </label>
        <input
          id="confirm-input"
          type="text"
          bind:value={input}
          autocomplete="off"
          spellcheck="false"
          class="confirm-input"
        />
      </Stack>

      <Row gap={3} justify="end">
        <a href="/dashboard" class="btn-cancel">Cancelar</a>
        <button
          class="btn-confirm"
          class:ready={canConfirm}
          disabled={!canConfirm}
          onclick={confirm}
        >
          {currentState === 'ACTIVE' ? 'Desligar' : 'Religar'}
        </button>
      </Row>
    </Stack>
  </Card>
</div>

<style>
  .ks-page {
    max-width: 640px;
    margin: 0 auto;
    padding: var(--s-7) var(--s-5);
  }
  h1 {
    font-size: var(--text-3xl);
    font-weight: 300;
    letter-spacing: var(--track-tight);
  }
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .current-state {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    padding: var(--s-4);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .state-line {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--fg-0);
    letter-spacing: var(--track-wide);
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .dot.active {
    background: var(--ok);
  }
  .dot.disabled {
    background: var(--err);
  }
  p {
    color: var(--fg-1);
    font-size: var(--text-base);
  }
  .confirm-input {
    font-family: var(--font-mono);
    font-size: var(--text-md);
    text-transform: uppercase;
    letter-spacing: var(--track-wide);
    padding: var(--s-3);
    background: var(--bg-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--fg-0);
    outline: none;
  }
  .confirm-input:focus {
    border-color: var(--cyan-brand);
  }
  .btn-cancel {
    font-family: var(--font-sans);
    font-size: var(--text-base);
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--border-strong);
    background: var(--bg-2);
    color: var(--fg-0);
    border-radius: var(--radius-sm);
    cursor: pointer;
    border-bottom: 1px solid var(--border-strong);
  }
  .btn-confirm {
    font-family: var(--font-sans);
    font-size: var(--text-base);
    font-weight: 500;
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-3);
    border-radius: var(--radius-sm);
    cursor: not-allowed;
    transition: all var(--dur) var(--ease);
  }
  .btn-confirm.ready {
    border-color: var(--err);
    background: var(--err);
    color: var(--bg-0);
    cursor: pointer;
  }
  .btn-confirm.ready:hover {
    background: var(--cyan-brand);
    border-color: var(--cyan-brand);
  }
</style>
