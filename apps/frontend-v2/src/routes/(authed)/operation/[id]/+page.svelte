<script lang="ts">
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import { page } from '$app/stores';

  let operationId = $derived($page.params.id);

  // FE-P1 stub — real fetch via API client in EP-005.
  const events = [
    { seq: 1, ts: '14:22:18.441', type: 'PLAN_SUBMITTED', summary: 'entry 64,230.00 · stop 62,100.00 · size 0.00482' },
    { seq: 2, ts: '14:22:18.891', type: 'PLAN_VALIDATED', summary: 'liquidation distance 12.40%' },
    { seq: 3, ts: '14:22:19.013', type: 'EXECUTE_REQUESTED', summary: '' },
    { seq: 4, ts: '14:22:21.664', type: 'ORDER_FILLED', summary: '64,235.50 · fee 0.06 USDT' },
    { seq: 5, ts: '14:22:21.664', type: 'POSITION_OPEN', summary: '+0.00482 BTC' },
    { seq: 6, ts: '18:47:33.201', type: 'STOP_UPDATED', summary: '63,100.00 (technical)' },
    { seq: 7, ts: '19:02:14.089', type: 'STOP_HIT', summary: '63,098.20' },
    { seq: 8, ts: '19:02:14.089', type: 'POSITION_CLOSED', summary: 'reason STOP_HIT · pnl -1.77%' }
  ];
</script>

<svelte:head>
  <title>Operation {operationId} — RBX Robson</title>
</svelte:head>

<div class="op-page">
  <header class="header">
    <div class="eyebrow">RBX ROBSON · OPERATION {operationId}</div>
    <h1>BTCUSDT · LONG</h1>
    <div class="meta">Opened 2026-04-12 14:22:18 UTC</div>
  </header>

  <Card>
    <Stack gap={3}>
      <div class="eyebrow">SUMMARY</div>
      <pre class="summary">PLAN      entry 64,230.00 · stop 62,100.00 · size 0.00482
VALIDATE  liquidation distance 12.40%
EXECUTE   requested
FILLED    64,235.50 · fee 0.06 USDT
OPEN      position +0.00482 BTC
STOP_UPD  63,100.00 (technical)
STOP_HIT  63,098.20
CLOSE     reason STOP_HIT · pnl -1.77%</pre>
    </Stack>
  </Card>

  <section class="event-stream-section">
    <div class="eyebrow">EVENT STREAM</div>
    <div class="events">
      {#each events as e (e.seq)}
        <div class="event" id="event-{e.seq}">
          <span class="tick">·</span>
          <span class="ts">{e.ts}</span>
          <span class="type">{e.type}</span>
          <span class="summary-text">{e.summary}</span>
        </div>
      {/each}
    </div>
  </section>
</div>

<style>
  .op-page {
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
  .meta,
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .summary {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--fg-0);
    white-space: pre;
    margin: 0;
    line-height: var(--lead-snug);
  }
  .event-stream-section {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }
  .events {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    border-left: 1px solid var(--cyan-dim);
    padding-left: var(--s-4);
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }
  .event {
    display: grid;
    grid-template-columns: 16px 140px 200px 1fr;
    gap: var(--s-3);
    align-items: baseline;
    padding: var(--s-1) 0;
  }
  .event:target {
    background: var(--cyan-subtle);
  }
  .tick {
    color: var(--cyan-brand);
  }
  .ts {
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
  }
  .type {
    color: var(--cyan-brand);
    letter-spacing: var(--track-wide);
    text-transform: uppercase;
    font-weight: 500;
  }
  .summary-text {
    color: var(--fg-1);
  }
</style>
