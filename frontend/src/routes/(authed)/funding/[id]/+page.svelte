<script lang="ts">
  import { untrack } from 'svelte';
  import { page } from '$app/stores';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import { robsonApi, type FundingSaga, type FundingEvent } from '$api/robson';
  import { _ } from 'svelte-i18n';

  // Decimal strings from the backend; format for display only.
  const fmtUsdt = (v: string | number): string => Number(v).toFixed(2);

  const POLL_INTERVAL_MS = 2_000;
  const TERMINAL_STATES = new Set(['REFRESHED', 'FAILED']);

  let sagaId = $derived($page.params.id ?? '');
  let saga = $state<FundingSaga | null>(null);
  let error = $state<string | null>(null);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  let isTerminal = $derived(saga ? TERMINAL_STATES.has(saga.state) : false);

  async function loadSaga() {
    if (!sagaId) return;
    try {
      saga = await robsonApi.getFundingSaga(sagaId);
      error = null;
      if (isTerminal) {
        stopPolling();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load saga';
    }
  }

  function startPolling() {
    stopPolling();
    pollTimer = setInterval(() => {
      void loadSaga();
    }, POLL_INTERVAL_MS);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
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

  function eventTypeLabel(ev: FundingEvent): string {
    return ev.type;
  }

  $effect(() => {
    untrack(() => {
      void loadSaga();
      startPolling();
    });
    return () => {
      stopPolling();
    };
  });
</script>

<svelte:head>
  <title>{$_('funding.sagaTitle')} — {sagaId.slice(0, 8)}</title>
</svelte:head>

<div class="saga-page">
  {#if error}
    <Card padding={5}>
      <Stack gap={3}>
        <div class="eyebrow">{$_('funding.error')}</div>
        <p class="err-text">{error}</p>
        <button
          class="btn-retry"
          onclick={() => {
            void loadSaga();
            startPolling();
          }}
        >
          {$_('funding.retry')}
        </button>
      </Stack>
    </Card>
  {:else if saga}
    <header class="header">
      <div class="eyebrow">{$_('funding.sagaTitle')}</div>
      <h1>{saga.saga_id.slice(0, 8)}</h1>
      <div class="meta">
        <span class="state-pill">{stateLabel(saga.state)}</span>
        <span class="mono dim">· {formatTimestamp(saga.updated_at)}</span>
      </div>
    </header>

    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('funding.items')}</div>
        <Card padding={4}>
          <Stack gap={2}>
            {#each saga.items as item}
              <Row gap={4} justify="between" align="baseline">
                <span class="mono">{item.asset}</span>
                <span class="mono">{item.qty}</span>
                <span class="mono dim">{fmtUsdt(item.est_usdt)} USDT</span>
              </Row>
            {/each}
          </Stack>
        </Card>
      </Stack>
    </section>

    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('funding.timeline')}</div>
        <Card padding={4}>
          <Stack gap={3}>
            {#each saga.events as ev}
              <div class="event">
                <span class="tick">·</span>
                <span class="ts">{formatTimestamp(ev.at)}</span>
                <span class="type">{eventTypeLabel(ev)}</span>
              </div>
            {:else}
              <p class="empty">—</p>
            {/each}
          </Stack>
        </Card>
      </Stack>
    </section>
  {:else}
    <div class="loading">
      <div class="eyebrow">{$_('funding.loading')}</div>
    </div>
  {/if}
</div>

<style>
  .saga-page {
    max-width: var(--content-w);
    margin: 0 auto;
    padding: var(--s-6) var(--s-5);
    display: flex;
    flex-direction: column;
    gap: var(--s-6);
  }
  .header h1 {
    font-size: var(--text-3xl);
    font-weight: 300;
    letter-spacing: var(--track-tight);
    margin: var(--s-2) 0;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .mono.dim {
    color: var(--fg-2);
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
  .event {
    display: grid;
    grid-template-columns: 16px 130px 1fr;
    gap: var(--s-3);
    align-items: baseline;
    padding: var(--s-1) 0;
  }
  .tick {
    color: var(--cyan-brand);
  }
  .ts {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
  }
  .type {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--cyan-brand);
    letter-spacing: var(--track-wide);
    text-transform: uppercase;
    font-weight: 500;
  }
  .empty {
    color: var(--fg-3);
    font-size: var(--text-sm);
    font-family: var(--font-mono);
  }
  .loading {
    padding: var(--s-7) 0;
    display: grid;
    place-items: center;
  }
  .err-text {
    color: var(--err, #ff4444);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    word-break: break-word;
  }
  .btn-retry {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    padding: var(--s-2) var(--s-4);
    border: 1px solid var(--cyan-dim);
    background: transparent;
    color: var(--cyan-brand);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .btn-retry:hover {
    background: var(--cyan-subtle);
  }
</style>
