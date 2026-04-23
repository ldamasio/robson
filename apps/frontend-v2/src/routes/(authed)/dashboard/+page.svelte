<script lang="ts">
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import Grid from '$design/components/Grid.svelte';
  import TickRuler from '$design/components/TickRuler.svelte';

  // FE-P1 stub — real data loading via API client in EP-004.
  const SLOT_COUNT = 6;
  const slots: Array<{ state: 'free' | 'open'; symbol?: string; pnl?: number }> = [
    { state: 'open', symbol: 'BTCUSDT', pnl: 1.24 },
    { state: 'open', symbol: 'ETHUSDT', pnl: -0.42 },
    { state: 'open', symbol: 'SOLUSDT', pnl: 0.18 },
    { state: 'open', symbol: 'AVAXUSDT', pnl: 2.01 },
    { state: 'free' },
    { state: 'free' }
  ];
</script>

<svelte:head>
  <title>Dashboard — RBX Robson</title>
</svelte:head>

<div class="dashboard">
  <header class="header">
    <Row justify="between" align="center">
      <Row gap={3} align="center">
        <img src="/brand/rbx-mark.svg" alt="RBX" width="32" height="32" />
        <img src="/brand/wordmark-robson.svg" alt="RBX Robson" height="22" />
      </Row>
      <div class="status-strip">
        <span class="dot live"></span> LIVE · SLOT 4/6 · COOLDOWN 00:00
      </div>
    </Row>
  </header>

  <section>
    <Stack gap={4}>
      <div class="eyebrow">SLOTS · APRIL 2026</div>
      <div class="slots-grid">
        {#each slots as slot, i}
          <div
            class="slot"
            class:occupied={slot.state === 'open'}
            class:positive={slot.pnl !== undefined && slot.pnl > 0}
            class:negative={slot.pnl !== undefined && slot.pnl < 0}
            title={slot.symbol ?? 'Free'}
          >
            {slot.state === 'open' ? '●' : '○'}
          </div>
        {/each}
      </div>
      <div class="eyebrow dim">{SLOT_COUNT - slots.filter((s) => s.state === 'free').length} OCCUPIED · {slots.filter((s) => s.state === 'free').length} FREE</div>
    </Stack>
  </section>

  <section>
    <Stack gap={4}>
      <div class="eyebrow">ACTIVE OPERATIONS</div>
      <Grid cols={2} gap={4}>
        {#each slots.filter((s) => s.state === 'open') as op}
          <Card>
            <Stack gap={2}>
              <div class="eyebrow">{op.symbol}</div>
              <Row justify="between">
                <span class="meta">PnL</span>
                <span class="mono" class:ok={(op.pnl ?? 0) > 0} class:err={(op.pnl ?? 0) < 0}>
                  {(op.pnl ?? 0) > 0 ? '+' : ''}{op.pnl?.toFixed(2)}%
                </span>
              </Row>
            </Stack>
          </Card>
        {/each}
      </Grid>
    </Stack>
  </section>

  <section>
    <Stack gap={4}>
      <div class="eyebrow">TODAY'S EVENTS</div>
      <Card>
        <pre class="event-stream">· 14:22:18.441  PLAN_SUBMITTED      BTCUSDT
· 14:22:18.891  PLAN_VALIDATED      liquidation distance 12.40%
· 14:22:19.013  EXECUTE_REQUESTED
· 14:22:21.664  ORDER_FILLED        64,235.50 · fee 0.06 USDT
· 14:22:21.664  POSITION_OPEN       +0.00482 BTC</pre>
        <TickRuler ticks={12} />
      </Card>
    </Stack>
  </section>
</div>

<style>
  .dashboard {
    max-width: var(--content-w);
    margin: 0 auto;
    padding: var(--s-6) var(--s-5);
    display: flex;
    flex-direction: column;
    gap: var(--s-7);
  }
  .header {
    padding-bottom: var(--s-5);
    border-bottom: 1px solid var(--border);
  }
  .status-strip {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    letter-spacing: var(--track-wide);
    text-transform: uppercase;
    color: var(--fg-1);
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .dot.live {
    background: var(--ok);
  }
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .eyebrow.dim {
    color: var(--fg-3);
  }
  .slots-grid {
    display: grid;
    grid-template-columns: repeat(6, 64px);
    gap: var(--s-2);
  }
  .slot {
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    font-size: 28px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-3);
    background: var(--bg-1);
  }
  .slot.occupied {
    color: var(--cyan-brand);
    border-color: var(--cyan-dim);
  }
  .slot.occupied.positive {
    color: var(--ok);
    border-color: var(--ok);
  }
  .slot.occupied.negative {
    color: var(--err);
    border-color: var(--err);
  }
  .meta {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .mono.ok {
    color: var(--ok);
  }
  .mono.err {
    color: var(--err);
  }
  .event-stream {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--fg-1);
    white-space: pre;
    margin: 0;
    line-height: var(--lead-snug);
  }
</style>
