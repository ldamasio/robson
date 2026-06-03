<script lang="ts">
  import { goto } from '$app/navigation';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import {
    robsonApi,
    type FundingQuote,
    type FundingSagaSummary,
    type ApiError,
  } from '$api/robson';
  import { _ } from 'svelte-i18n';

  // Monetary fields arrive as Decimal strings from the backend; format for
  // display only (no money math in the browser).
  const fmtUsdt = (v: string | number): string => Number(v).toFixed(2);

  type WizardPhase = 'quote' | 'preview' | 'confirm' | 'executing';

  let phase = $state<WizardPhase>('quote');
  let quote = $state<FundingQuote | null>(null);
  let recent = $state<FundingSagaSummary[]>([]);
  let error = $state<string | null>(null);
  let confirmInput = $state('');
  let submitting = $state(false);
  let tick = $state(Date.now());
  let tickTimer: ReturnType<typeof setInterval> | null = null;

  let keyword = $derived($_('funding.confirmKeyword') ?? 'CONVERTER E MOVER');
  let canConfirm = $derived(confirmInput === keyword && !submitting);

  let expiresInMs = $derived.by(() => {
    if (!quote) return 0;
    return Math.max(0, new Date(quote.expires_at).getTime() - tick);
  });

  let isExpired = $derived(expiresInMs === 0);

  function formatDuration(ms: number): string {
    const totalSec = Math.floor(ms / 1000);
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    return `${m}m ${String(s).padStart(2, '0')}s`;
  }

  async function loadRecent() {
    try {
      recent = await robsonApi.listFunding();
    } catch {
      recent = [];
    }
  }

  async function getQuote() {
    error = null;
    quote = null;
    confirmInput = '';
    phase = 'quote';
    submitting = true;
    try {
      quote = await robsonApi.getFundingQuote();
      phase = 'preview';
      startTick();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to get quote';
      phase = 'quote';
    } finally {
      submitting = false;
    }
  }

  function toConfirm() {
    if (!quote || isExpired) return;
    confirmInput = '';
    phase = 'confirm';
  }

  async function execute() {
    if (!quote || !canConfirm) return;
    error = null;
    phase = 'executing';
    submitting = true;
    try {
      const idempotencyKey = crypto.randomUUID();
      const result = await robsonApi.executeFunding(
        quote.quote_id,
        idempotencyKey,
      );
      stopTick();
      void goto(`/funding/${result.saga_id}`);
    } catch (e) {
      const apiErr = e as ApiError;
      if (apiErr?.status === 503) {
        error = $_('funding.fundingDisabled') ?? 'Funding indisponível';
      } else {
        error = apiErr?.message ?? 'Execution failed';
      }
      phase = 'confirm';
    } finally {
      submitting = false;
    }
  }

  function startTick() {
    stopTick();
    tickTimer = setInterval(() => {
      tick = Date.now();
    }, 1000);
  }

  function stopTick() {
    if (tickTimer) {
      clearInterval(tickTimer);
      tickTimer = null;
    }
  }

  function formatTimestamp(iso: string | null): string {
    if (!iso) return '—';
    const d = new Date(iso);
    const yyyy = d.getUTCFullYear();
    const mm = String(d.getUTCMonth() + 1).padStart(2, '0');
    const dd = String(d.getUTCDate()).padStart(2, '0');
    const hh = String(d.getUTCHours()).padStart(2, '0');
    const mi = String(d.getUTCMinutes()).padStart(2, '0');
    const ss = String(d.getUTCSeconds()).padStart(2, '0');
    return `${yyyy}-${mm}-${dd} ${hh}:${mi}:${ss} UTC`;
  }

  function stateLabel(state: string): string {
    return $_(`funding.state.${state}`) ?? state;
  }

  $effect(() => {
    void loadRecent();
    return () => {
      stopTick();
    };
  });
</script>

<svelte:head>
  <title>{$_('funding.pageTitle')}</title>
</svelte:head>

<div class="funding-page">
  <Card padding={7}>
    <Stack gap={5}>
      <div class="eyebrow">FUNDING</div>

      {#if phase === 'quote'}
        <h1>{$_('funding.previewTitle')}</h1>
        {#if error}
          <p class="err-text">{error}</p>
        {/if}
        <Row gap={3} justify="start">
          <button class="btn-primary" disabled={submitting} onclick={getQuote}>
            {#if submitting}
              {$_('funding.loading') ?? '...'}
            {:else}
              {$_('funding.quoteButton')}
            {/if}
          </button>
        </Row>
      {:else if phase === 'preview' || phase === 'confirm' || phase === 'executing'}
        {#if quote}
          <h1>
            {phase === 'confirm' || phase === 'executing'
              ? $_('funding.confirmTitle')
              : $_('funding.previewTitle')}
          </h1>

          <div class="quote-block">
            <Stack gap={4}>
              <div class="eyebrow">QUOTE</div>
              <div class="items">
                {#each quote.items as item}
                  <Row gap={4} justify="between" align="baseline">
                    <span class="mono">{item.asset}</span>
                    <span class="mono">{item.qty}</span>
                    <span class="mono dim">{fmtUsdt(item.est_usdt)} USDT</span
                    >
                  </Row>
                {/each}
              </div>
              <div class="summary">
                <Row gap={4} justify="between" align="baseline">
                  <span class="eyebrow">{$_('funding.totalEstimated')}</span>
                  <span class="mono"
                    >{fmtUsdt(quote.estimated_usdt)} USDT</span
                  >
                </Row>
                <Row gap={4} justify="between" align="baseline">
                  <span class="eyebrow">{$_('funding.fees')}</span>
                  <span class="mono">{fmtUsdt(quote.fees)} USDT</span>
                </Row>
                <Row gap={4} justify="between" align="baseline">
                  <span class="eyebrow">{$_('funding.slippageBps')}</span>
                  <span class="mono">{quote.slippage_bps}</span>
                </Row>
                <Row gap={4} justify="between" align="baseline">
                  <span class="eyebrow">{$_('funding.expiresIn')}</span>
                  <span class="mono" class:err={isExpired}>
                    {isExpired ? 'EXPIRED' : formatDuration(expiresInMs)}
                  </span>
                </Row>
              </div>
            </Stack>
          </div>

          {#if phase === 'preview'}
            {#if error}
              <p class="err-text">{error}</p>
            {/if}
            <Row gap={3} justify="end">
              <button
                class="btn-cancel"
                onclick={() => {
                  phase = 'quote';
                  quote = null;
                  stopTick();
                }}
              >
                {$_('funding.cancel')}
              </button>
              <button
                class="btn-confirm"
                class:ready={!isExpired}
                disabled={isExpired}
                onclick={toConfirm}
              >
                {$_('funding.executeButton')}
              </button>
            </Row>
          {:else if phase === 'confirm' || phase === 'executing'}
            <Stack gap={2}>
              <label class="eyebrow" for="confirm-input">
                {$_('funding.typeToConfirm')}
              </label>
              <input
                id="confirm-input"
                type="text"
                bind:value={confirmInput}
                autocomplete="off"
                spellcheck="false"
                class="confirm-input"
                disabled={phase === 'executing'}
              />
            </Stack>

            {#if error}
              <p class="err-text">{error}</p>
            {/if}

            <Row gap={3} justify="end">
              <button
                class="btn-cancel"
                disabled={phase === 'executing'}
                onclick={() => {
                  phase = 'preview';
                  confirmInput = '';
                  error = null;
                }}
              >
                {$_('funding.cancel')}
              </button>
              <button
                class="btn-confirm"
                class:ready={canConfirm}
                disabled={!canConfirm}
                onclick={execute}
              >
                {#if phase === 'executing'}
                  {$_('funding.loading') ?? '...'}
                {:else}
                  {$_('funding.executeButton')}
                {/if}
              </button>
            </Row>
          {/if}
        {/if}
      {/if}
    </Stack>
  </Card>

  {#if recent.length > 0}
    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('funding.recentSagas')}</div>
        {#each recent as saga (saga.saga_id)}
          <a href="/funding/{saga.saga_id}" class="saga-link">
            <Card padding={4}>
              <Row justify="between" align="center">
                <Stack gap={1}>
                  <span class="mono">{saga.saga_id.slice(0, 8)}</span>
                  <span class="meta dim"
                    >{formatTimestamp(saga.created_at)}</span
                  >
                </Stack>
                <span class="state-pill">{stateLabel(saga.state)}</span>
                <span class="mono">{fmtUsdt(saga.estimated_usdt)} USDT</span>
              </Row>
            </Card>
          </a>
        {/each}
      </Stack>
    </section>
  {:else}
    <section>
      <Card padding={4}>
        <p class="empty">{$_('funding.noSagas')}</p>
      </Card>
    </section>
  {/if}
</div>

<style>
  .funding-page {
    max-width: 640px;
    margin: 0 auto;
    padding: var(--s-7) var(--s-5);
    display: flex;
    flex-direction: column;
    gap: var(--s-7);
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
  .quote-block {
    padding: var(--s-4);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .items {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }
  .summary {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    padding-top: var(--s-3);
    border-top: 1px solid var(--border);
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .mono.dim {
    color: var(--fg-2);
  }
  .meta {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
  }
  .meta.dim {
    color: var(--fg-3);
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
    width: 100%;
    box-sizing: border-box;
  }
  .confirm-input:focus {
    border-color: var(--cyan-brand);
  }
  .btn-primary {
    font-family: var(--font-sans);
    font-size: var(--text-base);
    font-weight: 500;
    padding: var(--s-3) var(--s-5);
    border: 1px solid var(--cyan-dim);
    background: transparent;
    color: var(--cyan-brand);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: all var(--dur) var(--ease);
  }
  .btn-primary:hover:not(:disabled) {
    background: var(--cyan-subtle);
  }
  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
    text-decoration: none;
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
  .err-text {
    color: var(--err, #ff4444);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    word-break: break-word;
  }
  .state-pill {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--s-1) var(--s-2);
    white-space: nowrap;
  }
  .saga-link {
    text-decoration: none;
    border-bottom: none;
  }
  .saga-link:hover {
    border-bottom: none;
  }
  .empty {
    color: var(--fg-3);
    font-size: var(--text-sm);
    font-family: var(--font-mono);
  }
</style>
