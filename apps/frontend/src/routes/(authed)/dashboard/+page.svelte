<script lang="ts">
  import { untrack } from 'svelte';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import Grid from '$design/components/Grid.svelte';
  import TickRuler from '$design/components/TickRuler.svelte';
  import { robsonApi, connectEventStream, type Position, type SseEvent } from '$api/robson';
  import { activePositions } from '$stores/operations';
  import { haltStatus } from '$stores/slots';
  import { recentEvents, pushEvent } from '$stores/events';
  import { deriveSlots, INITIAL_MONTHLY_SLOT_BUDGET } from '$lib/config/slots';
  import { formatTimeUtc, isTodayUtc } from '$lib/utils/time';
  import {
    positionLabel,
    positionStateLabel,
    haltStateLabel,
    eventTypeLabel,
    isPositionActive
  } from '$lib/presentation/labels';
  import { _ } from 'svelte-i18n';

  const POLL_INTERVAL_MS = 10_000;

  let error = $state<string | null>(null);
  let connected = $state(false);
  let closeSse: (() => void) | null = null;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  let positions = $derived($activePositions);
  let slots = $derived(deriveSlots(positions));
  let occupied = $derived(slots.filter((s) => s.occupied).length);
  let displayedSlots = $derived(slots.length);
  let free = $derived(Math.max(0, INITIAL_MONTHLY_SLOT_BUDGET - occupied));
  let activeOps = $derived(positions.filter((p) => isPositionActive(p.state)));
  let todayEvents = $derived($recentEvents.filter((e) => isTodayUtc(e.occurred_at)));
  let haltState = $derived($haltStatus?.state ?? 'active');

  function pnlFor(p: Position): number | null {
    if (typeof p.state === 'object' && 'Closed' in p.state) {
      return Number(p.state.Closed.realized_pnl);
    }
    return p.pnl ?? p.realized_pnl ?? null;
  }

  function monthLabel(): string {
    const now = new Date();
    const month = now.toLocaleDateString('en-US', { month: 'long', timeZone: 'UTC' });
    return `${month.toUpperCase()} ${now.getUTCFullYear()}`;
  }

  async function load() {
    error = null;
    try {
      const [status, halt] = await Promise.all([
        robsonApi.getStatus(),
        robsonApi.getHaltStatus()
      ]);
      activePositions.set(status.positions);
      haltStatus.set(halt);
      connected = true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to connect to Robson backend';
      connected = false;
    }
  }

  function startSse() {
    stopSse();
    closeSse = connectEventStream(
      (event: SseEvent) => {
        pushEvent(event);
        const payload = event.payload as Record<string, unknown>;
        const posId = payload.position_id as string | undefined;
        if (posId && event.event_type === 'position.changed') {
          void robsonApi.getStatus().then((s) => activePositions.set(s.positions)).catch(() => {});
        }
      },
      () => {
        connected = false;
      }
    );
  }

  function stopSse() {
    if (closeSse) {
      closeSse();
      closeSse = null;
    }
  }

  function startPolling() {
    stopPolling();
    pollTimer = setInterval(() => {
      void (async () => {
        try {
          const [status, halt] = await Promise.all([
            robsonApi.getStatus(),
            robsonApi.getHaltStatus()
          ]);
          activePositions.set(status.positions);
          haltStatus.set(halt);
          connected = true;
          error = null;
        } catch {
          // SSE + polling: silent retry, error shown only if initial load fails
        }
      })();
    }, POLL_INTERVAL_MS);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  function retry() {
    void load();
    startSse();
    startPolling();
  }

  $effect(() => {
    untrack(() => {
      void load();
      startSse();
      startPolling();
    });
    return () => {
      stopSse();
      stopPolling();
    };
  });
</script>

<svelte:head>
  <title>{$_('dashboard.pageTitle')}</title>
</svelte:head>

<div class="dashboard">
  <header class="header">
    <Row justify="between" align="center">
      <Row gap={3} align="center">
        <img src="/brand/rbx-mark.svg" alt="RBX" width="32" height="32" />
        <img src="/brand/wordmark-robson.svg" alt="RBX Robson" height="22" />
      </Row>
      <div class="status-strip">
        {#if error}
          <span class="dot err"></span> {$_('dashboard.offline')} · {error}
        {:else if !connected}
          <span class="dot warn"></span> {$_('dashboard.connecting')}
        {:else}
          <span class="dot live"></span>
          {haltStateLabel(haltState)} · SLOT {occupied}/{displayedSlots}
        {/if}
      </div>
    </Row>
  </header>

  {#if error}
    <Card padding={5}>
      <Stack gap={3}>
        <div class="eyebrow">{$_('dashboard.connectionError')}</div>
        <p class="err-text">{error}</p>
        <button class="btn-retry" onclick={retry}>{$_('dashboard.retry')}</button>
      </Stack>
    </Card>
  {:else}
    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('dashboard.slots')} · {monthLabel()}</div>
        <div class="slots-grid">
          {#each slots as slot}
            <a
              href={slot.occupied ? `/operation/${slot.positionId}` : ''}
              class="slot"
              class:occupied={slot.occupied}
            >
              {slot.occupied ? '●' : '○'}
            </a>
          {/each}
        </div>
        <div class="eyebrow dim">{$_('dashboard.occupied', { values: { count: occupied } })} · {$_('dashboard.freeCount', { values: { count: free } })}</div>
      </Stack>
    </section>

    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('dashboard.activeOps')}</div>
        {#if activeOps.length === 0}
          <Card>
            <p class="empty">{$_('dashboard.noActive')}</p>
          </Card>
        {:else}
          <Grid cols={2} gap={4}>
            {#each activeOps as op}
              <a href="/operation/{op.id}" class="op-card-link">
                <Card>
                  <Stack gap={2}>
                    <div class="eyebrow">{positionLabel(op)}</div>
                    <Row justify="between">
                      <span class="meta">{positionStateLabel(op.state)}</span>
                      {#if pnlFor(op) !== null}
                        <span
                          class="mono"
                          class:ok={(pnlFor(op) ?? 0) > 0}
                          class:err={(pnlFor(op) ?? 0) < 0}
                        >
                          {(pnlFor(op) ?? 0) > 0 ? '+' : ''}{pnlFor(op)?.toFixed(2)}%
                        </span>
                      {/if}
                    </Row>
                  </Stack>
                </Card>
              </a>
            {/each}
          </Grid>
        {/if}
      </Stack>
    </section>

    <section>
      <Stack gap={4}>
        <div class="eyebrow">{$_('dashboard.todayEventsLabel')}</div>
        <Card>
          {#if todayEvents.length === 0}
            <p class="empty">{$_('dashboard.noEventsToday')}</p>
          {:else}
            <div class="event-stream">
              {#each todayEvents as e (e.event_id)}
                <div class="event-line">
                  <span class="tick">·</span>
                  <span class="ts">{formatTimeUtc(e.occurred_at)}</span>
                  <span class="type">{eventTypeLabel(e)}</span>
                </div>
              {/each}
            </div>
          {/if}
          <TickRuler ticks={12} />
        </Card>
      </Stack>
    </section>
  {/if}
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
    display: inline-block;
  }
  .dot.live {
    background: var(--ok);
  }
  .dot.err {
    background: var(--err);
  }
  .dot.warn {
    background: var(--warn, #f59e0b);
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
    grid-template-columns: repeat(4, 64px);
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
    text-decoration: none;
    border-bottom: 1px solid var(--border);
  }
  .slot.occupied {
    color: var(--cyan-brand);
    border-color: var(--cyan-dim);
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
    display: flex;
    flex-direction: column;
    gap: var(--s-1);
    margin-bottom: var(--s-3);
  }
  .event-line {
    display: flex;
    gap: var(--s-3);
    align-items: baseline;
  }
  .tick {
    color: var(--cyan-brand);
  }
  .ts {
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
    min-width: 12ch;
  }
  .type {
    color: var(--cyan-brand);
    letter-spacing: var(--track-wide);
    text-transform: uppercase;
  }
  .empty {
    color: var(--fg-3);
    font-size: var(--text-sm);
    font-family: var(--font-mono);
  }
  .op-card-link {
    text-decoration: none;
    border-bottom: none;
  }
  .op-card-link:hover {
    border-bottom: none;
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
