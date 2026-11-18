<script lang="ts">
  import { untrack } from 'svelte';
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import LCorners from '$design/components/LCorners.svelte';
  import { page } from '$app/state';
  import { robsonApi, connectEventStream, type Position, type SseEvent } from '$api/robson';
  import { formatTimeUtc } from '#lib/utils/time.ts';
  import { OPERATION_REFRESH_MS, operationStreamHandlers } from '#lib/config/polling.ts';
  import { serialRefresh } from '#lib/utils/refresh.ts';
  import { sseFreshness, markSseEvent, markSseConnected, markSseStale } from '$stores/sseFreshness';
  import {
    positionLabel,
    positionSummaryLines,
    positionMetaLine,
    eventTypeLabel,
    eventSummaryText
  } from '#lib/presentation/labels.ts';
  import { _ } from 'svelte-i18n';

  let operationId = $derived(page.params.id ?? '');
  let position = $state<Position | null>(null);
  let error = $state<string | null>(null);
  type TaggedEvent = SseEvent & { _seq: number };
  let events = $state<TaggedEvent[]>([]);
  let seq = $state(0);
  let closeSse: (() => void) | null = null;
  let refreshTimer: ReturnType<typeof setTimeout> | null = null;
  const MAX_OP_EVENTS = 500;

  let summaryLines = $derived(position ? positionSummaryLines(position) : []);
  let metaLine = $derived(position ? positionMetaLine(position) : '');

  // Snapshot reads run one at a time; triggers during a read coalesce into
  // one follow-up. Results publish in order and every read publishes, so a
  // slow older snapshot never overwrites a newer one and a steady trigger
  // stream cannot starve the page.
  const loadPosition = serialRefresh(async () => {
    if (!operationId) return;
    error = null;
    try {
      position = await robsonApi.getPosition(operationId);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load position';
    }
  });

  // The stream is a delta feed: re-read the position snapshot when an event
  // concerns it or the server asks for a resync, at most once per window so a
  // burst cannot become a request storm; every stream establishment re-reads
  // it at once (operationStreamHandlers).
  function scheduleRefresh() {
    if (refreshTimer) return;
    refreshTimer = setTimeout(() => {
      refreshTimer = null;
      void loadPosition();
    }, OPERATION_REFRESH_MS);
  }

  function startSse() {
    stopSse();
    const handlers = operationStreamHandlers<SseEvent>(operationId, {
      refreshNow: () => void loadPosition(),
      scheduleRefresh,
      appendEvent: (event) => {
        seq += 1;
        const tagged: TaggedEvent = { ...event, _seq: seq };
        events = events.length >= MAX_OP_EVENTS
          ? [...events.slice(-(MAX_OP_EVENTS - 1)), tagged]
          : [...events, tagged];
      },
      markEvent: markSseEvent,
      markConnected: markSseConnected,
      markStale: markSseStale
    });
    closeSse = connectEventStream(
      handlers.onEvent,
      handlers.onError,
      handlers.onReconnect,
      handlers.onStale,
      handlers.onActivity,
      handlers.onOpen
    );
  }

  function stopSse() {
    if (closeSse) {
      closeSse();
      closeSse = null;
    }
    if (refreshTimer) {
      clearTimeout(refreshTimer);
      refreshTimer = null;
    }
  }

  $effect(() => {
    untrack(() => {
      void loadPosition();
      startSse();
      sseFreshness.start();
    });
    return () => {
      stopSse();
      sseFreshness.stop();
    };
  });
</script>

<svelte:head>
  <title>{$_('operation.title', { values: { label: position ? positionLabel(position) : operationId } })}</title>
</svelte:head>

<div class="op-page">
  {#if error}
    <Card padding={5}>
      <Stack gap={3}>
        <div class="eyebrow">{$_('operation.loadError')}</div>
        <p class="err-text">{error}</p>
        <button class="btn-retry" onclick={() => { void loadPosition(); startSse(); }}>{$_('operation.retry')}</button>
      </Stack>
    </Card>
  {:else if position}
    <header class="header">
      <div class="eyebrow">{$_('operation.header', { values: { id: operationId.slice(0, 8) } })}</div>
      <h1>{positionLabel(position)}</h1>
      <div class="meta">{metaLine}</div>
    </header>

    <LCorners size={14}>
      <Card>
        <Stack gap={3}>
          <div class="eyebrow">{$_('operation.summary')}</div>
          <pre class="summary">{summaryLines.join('\n')}</pre>
        </Stack>
      </Card>
    </LCorners>

    <section class="event-stream-section">
      <div class="eyebrow">
        {$_('operation.eventStreamLabel')}
        <span
          class="sse-badge"
          class:live={$sseFreshness.fresh}
          class:stale={!$sseFreshness.fresh}
          title={$sseFreshness.fresh
            ? 'Event stream is fresh'
            : `No SSE data for ${$sseFreshness.staleSeconds}s`}
        >
          {$sseFreshness.fresh ? 'LIVE' : 'STALE, reconnecting'}
        </span>
      </div>
      <div class="limitation">
        {$_('operation.sessionOnly')}
      </div>
      {#if events.length === 0}
        <p class="empty">{$_('operation.noEvents')}</p>
      {:else}
        <div class="events">
          {#each events as e (e._seq)}
            <div class="event" id="event-{e._seq}">
              <span class="tick">·</span>
              <span class="ts">{formatTimeUtc(e.occurred_at)}</span>
              <span class="type">{eventTypeLabel(e)}</span>
              <span class="summary-text">{eventSummaryText(e)}</span>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {:else}
    <div class="loading">
      <div class="eyebrow">{$_('operation.loading')}</div>
    </div>
  {/if}
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
  .limitation {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--fg-3);
    letter-spacing: var(--track-wide);
  }
  .sse-badge {
    margin-left: var(--s-2);
    padding: var(--s-1) var(--s-2);
    border-radius: var(--radius-sm);
    border: 1px solid transparent;
  }
  .sse-badge.live {
    color: var(--ok);
    border-color: var(--ok);
    background: rgba(127, 183, 126, 0.08);
  }
  .sse-badge.stale {
    color: var(--warn, #f59e0b);
    border-color: var(--warn, #f59e0b);
    background: rgba(245, 158, 11, 0.08);
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
    grid-template-columns: 16px 130px 220px 1fr;
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
