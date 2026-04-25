<script lang="ts">
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import TickRuler from '$design/components/TickRuler.svelte';
</script>

<svelte:head>
  <title>RBX Robson</title>
</svelte:head>

<div class="page">
  <header class="header">
    <Row justify="between" align="center">
      <Row gap={3} align="center">
        <img src="/brand/rbx-mark.svg" alt="RBX" width="40" height="40" />
        <img src="/brand/wordmark-robson.svg" alt="RBX Robson" height="28" />
      </Row>
      <div class="eyebrow">FE-P1 · SCAFFOLD · v0.0.1</div>
    </Row>
  </header>

  <section class="hero">
    <Stack gap={5}>
      <div class="eyebrow">SYSTEMS ENGINEERING</div>
      <h1>Systems engineering for operations that demand control.</h1>
      <p class="lead">
        Robson is a deterministic, auditable risk execution engine.
        This scaffold confirms RBX Voltage tokens + signature elements render correctly.
      </p>
    </Stack>
  </section>

  <section class="sample">
    <Card>
      <Stack gap={4}>
        <div class="eyebrow">BTCUSDT · PERPETUAL</div>
        <Row gap={6}>
          <Stack gap={1}>
            <span class="meta">Position</span>
            <span class="mono">+0.00482 BTC</span>
          </Stack>
          <Stack gap={1}>
            <span class="meta">PnL</span>
            <span class="mono ok">+1.24%</span>
          </Stack>
          <Stack gap={1}>
            <span class="meta">Liquidation distance</span>
            <span class="mono">12.40%</span>
          </Stack>
          <Stack gap={1}>
            <span class="meta">Status</span>
            <span class="status"><span class="dot"></span> LIVE</span>
          </Stack>
        </Row>
        <TickRuler ticks={12} />
      </Stack>
    </Card>
  </section>

  <footer class="footer">
    <Row justify="between">
      <span class="eyebrow">RBX · VOLTAGE · FE-P1</span>
      <span class="eyebrow">2026-04-23</span>
    </Row>
  </footer>
</div>

<style>
  .page {
    max-width: var(--content-w);
    margin: 0 auto;
    padding: var(--s-7) var(--s-5);
  }
  .header {
    padding-bottom: var(--s-6);
    border-bottom: 1px solid var(--border);
  }
  .eyebrow {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .hero {
    padding: var(--s-9) 0;
    max-width: var(--prose-w);
  }
  .hero h1 {
    font-size: var(--text-5xl);
    font-weight: 300;
    line-height: var(--lead-tight);
    letter-spacing: var(--track-tight);
  }
  .lead {
    font-size: var(--text-lg);
    color: var(--fg-0);
    font-weight: 300;
  }
  .sample {
    padding: var(--s-6) 0;
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
    font-size: var(--text-xl);
    color: var(--fg-0);
  }
  .mono.ok {
    color: var(--ok);
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-wide);
    padding: var(--s-1) var(--s-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ok);
  }
  .footer {
    padding-top: var(--s-6);
    margin-top: var(--s-9);
    border-top: 1px solid var(--border);
  }
</style>
