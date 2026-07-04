<script lang="ts">
  import { untrack } from "svelte";
  import Card from "$design/components/Card.svelte";
  import Stack from "$design/components/Stack.svelte";
  import Row from "$design/components/Row.svelte";
  import Grid from "$design/components/Grid.svelte";
  import TickRuler from "$design/components/TickRuler.svelte";
  import ArmModal from "$design/components/ArmModal.svelte";
  import {
    robsonApi,
    connectEventStream,
    type Position,
    type SseEvent,
    type PendingApproval,
  } from "$api/robson";
  import { activePositions } from "$stores/operations";
  import { haltStatus } from "$stores/slots";
  import { recentEvents, pushEvent } from "$stores/events";
  import { toasts, showToast } from "$stores/toast";
  import { status as sharedStatus, refreshStatus } from "$stores/status";
  import {
    deriveMonthSlots,
    sortPositionsOldestFirst,
  } from "$lib/config/slots";
  import { formatTimeUtc, isTodayUtc } from "$lib/utils/time";
  import {
    positionLabel,
    positionStateLabel,
    positionMetaLine,
    positionSummaryLines,
    isRenderableLivePosition,
    isPositionCancelled,
    haltStateLabel,
    eventTypeLabel,
  } from "$lib/presentation/labels";
  import { _ } from "svelte-i18n";

  let error = $state<string | null>(null);
  let connected = $state(false);
  let closeSse: (() => void) | null = null;
  let showArmModal = $state(false);
  let pendingApprovals = $state<PendingApproval[]>([]);
  let historyError = $state<string | null>(null);
  let selectedMonth = $state(currentMonthKey());
  let monthlyPositions = $state<Position[]>([]);
  let monthlySlotCellsTotal = $state<number | null>(null);
  let currentStatus = $derived($sharedStatus);
  let approvalTick = $state(Date.now());
  let approvalTickTimer: ReturnType<typeof setInterval> | null = null;
  let sseRefreshTimer: ReturnType<typeof setTimeout> | null = null;

  let currentMonth = $derived(currentMonthKey());
  let monthOps = $derived(sortPositionsOldestFirst(monthlyPositions));
  let isHistoricalMonth = $derived(selectedMonth !== currentMonth);
  let liveOps = $derived((currentStatus?.positions ?? []).filter((op) => isRenderableLivePosition(op)));
  let historicalOps = $derived(monthOps.filter((op) => !isPositionCancelled(op.state)));
  let displayOps = $derived(isHistoricalMonth ? historicalOps : liveOps);
  let slotPositions = $derived(
    isHistoricalMonth ? displayOps : liveOps,
  );
  let slots = $derived(
    deriveMonthSlots(
      slotPositions,
      isHistoricalMonth
        ? (monthlySlotCellsTotal ?? slotPositions.length)
        : (currentStatus?.slot_cells_total ?? slotPositions.length),
      isHistoricalMonth ? "expired" : "free",
    ),
  );
  let occupied = $derived(slots.filter((s) => s.kind === "occupied").length);
  let displayedSlots = $derived(slots.length);
  let free = $derived(slots.filter((s) => s.kind !== "occupied").length);
  let todayEvents = $derived(
    $recentEvents.filter((e) => isTodayUtc(e.occurred_at)),
  );
  let haltState = $derived($haltStatus?.state ?? "active");
  // "Cannot operate" only when nothing is open AND nothing can open.
  // new_slots_available === 0 with an open position just means the capital
  // is fully at work (the open position's latent risk occupies the slot) —
  // that must not read as an error.
  let insufficientCapital = $derived(
    !isHistoricalMonth && currentStatus
      ? currentStatus.new_slots_available === 0 &&
          currentStatus.occupied_slots === 0
      : false,
  );
  let monthlyBudgetLimitPct = 4;
  // With no open positions (latent risk 0), slots hit zero either because
  // monthly losses left less than one 1%-risk slot in the 4% budget, or
  // because the capital itself is too small. Only the latter is fixable by
  // adding funds.
  let monthlyBudgetExhausted = $derived(
    insufficientCapital &&
      (currentStatus?.monthly_realized_loss_pct ?? 0) >
        monthlyBudgetLimitPct - 1,
  );
  let budgetUsedPct = $derived(
    Math.min(
      100,
      Math.max(
        0,
        ((currentStatus?.monthly_realized_loss_pct ?? 0) /
          monthlyBudgetLimitPct) *
          100,
      ),
    ),
  );

  function countdownRemaining(expiresAt: string): string {
    const ms = new Date(expiresAt).getTime() - approvalTick;
    if (ms <= 0) return "expired";
    const totalSec = Math.floor(ms / 1000);
    const m = Math.floor(totalSec / 60);
    const s = totalSec % 60;
    return `${m}m ${s}s`;
  }

  function variationFor(p: Position): number | null {
    if (p.variation_pct !== undefined) return p.variation_pct;
    if (typeof p.state === "object" && "Closed" in p.state) {
      const entry = p.entry_price;
      const exit = Number(p.state.Closed.exit_price);
      if (entry && Number.isFinite(exit)) {
        const diff = p.side === "Short" ? entry - exit : exit - entry;
        return (diff / entry) * 100;
      }
    }
    return null;
  }

  function monthLabel(): string {
    return monthDisplayLabel(selectedMonth);
  }

  function currentMonthKey(): string {
    const now = new Date();
    return `${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, "0")}`;
  }

  function monthDisplayLabel(monthKey: string): string {
    const [year, month] = monthKey.split("-").map(Number);
    const date = new Date(Date.UTC(year, month - 1, 1));
    const monthName = date.toLocaleDateString("en-US", {
      month: "long",
      timeZone: "UTC",
    });
    return `${monthName.toUpperCase()} ${year}`;
  }

  function parseMonthKey(monthKey: string): Date {
    const [year, month] = monthKey.split("-").map(Number);
    return new Date(Date.UTC(year, month - 1, 1));
  }

  function monthKeyFromDateLike(value?: string | null): string | null {
    if (!value) return null;
    const d = new Date(value);
    if (isNaN(d.getTime())) return null;
    return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
  }

  function shiftMonth(monthKey: string, delta: number): string {
    const d = parseMonthKey(monthKey);
    d.setUTCMonth(d.getUTCMonth() + delta);
    return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
  }

  function isInheritedForMonth(p: Position, monthKey: string): boolean {
    const createdMonth = monthKeyFromDateLike(p.created_at);
    return createdMonth !== null && createdMonth !== monthKey;
  }

  function monthStateLabel(p: Position): string {
    if (isInheritedForMonth(p, selectedMonth)) return "INHERITED";
    return "NEW";
  }

  function topBarLabel(): string {
    return isHistoricalMonth ? "SNAPSHOT" : haltStateLabel(haltState);
  }

  function topBarActionLabel(): string {
    return isHistoricalMonth ? "NOW" : "ENTRY";
  }

  function formatMoney(value: number | null | undefined): string {
    const amount = Number(value ?? 0);
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency: "USD",
      maximumFractionDigits: 2,
    }).format(Number.isFinite(amount) ? amount : 0);
  }

  function formatPct(value: number | null | undefined): string {
    const pct = Number(value ?? 0);
    return `${(Number.isFinite(pct) ? pct : 0).toFixed(1)}%`;
  }

  function returnToCurrentMonth() {
    if (!isHistoricalMonth) return;
    selectedMonth = currentMonthKey();
    void load();
  }

  async function loadStatus() {
    error = null;
    try {
      const [status, halt] = await Promise.all([
        refreshStatus(),
        robsonApi.getHaltStatus(),
      ]);
      if (status) {
        activePositions.set(status.positions.filter((op) => isRenderableLivePosition(op)));
        pendingApprovals = status.pending_approvals;
      }
      haltStatus.set(halt);
      connected = true;
    } catch (e) {
      error =
        e instanceof Error ? e.message : "Failed to connect to Robson backend";
      connected = false;
    }
  }

  async function loadHistory() {
    historyError = null;
    monthlySlotCellsTotal = null;
    try {
      const response = await robsonApi.getMonthlyPositions(selectedMonth);
      monthlyPositions = response.positions;
      const maybeTotal = Number(
        (response as { slot_cells_total?: unknown }).slot_cells_total,
      );
      monthlySlotCellsTotal = Number.isFinite(maybeTotal) ? maybeTotal : null;
    } catch (e) {
      historyError =
        e instanceof Error ? e.message : "Failed to load month history";
    }
  }

  async function load() {
    await Promise.all([loadStatus(), loadHistory()]);
  }

  function scheduleRefresh(): void {
    if (sseRefreshTimer) clearTimeout(sseRefreshTimer);
    sseRefreshTimer = setTimeout(() => {
      sseRefreshTimer = null;
      void Promise.all([refreshStatus().catch(() => {}), loadHistory().catch(() => {})]);
    }, 1_500);
  }

  function startSse() {
    stopSse();
    closeSse = connectEventStream(
      (event: SseEvent) => {
        pushEvent(event);
        const payload = event.payload as Record<string, unknown>;
        const posId = payload.position_id as string | undefined;
        if (posId && event.event_type === "position.changed") {
          scheduleRefresh();
        }
        if (event.event_type.startsWith("query.")) {
          scheduleRefresh();
        }
      },
      () => {
        connected = false;
      },
      () => {
        connected = true;
        void loadStatus();
      },
    );
  }

  function stopSse() {
    if (closeSse) {
      closeSse();
      closeSse = null;
    }
    if (sseRefreshTimer) {
      clearTimeout(sseRefreshTimer);
      sseRefreshTimer = null;
    }
  }

  function retry() {
    void load();
    startSse();
  }

  function prevMonth() {
    selectedMonth = shiftMonth(selectedMonth, -1);
    void loadHistory();
  }

  function nextMonth() {
    if (selectedMonth === currentMonth) return;
    selectedMonth = shiftMonth(selectedMonth, 1);
    void loadHistory();
  }

  $effect(() => {
    untrack(() => {
      void load();
      startSse();
      approvalTickTimer = setInterval(() => {
        approvalTick = Date.now();
      }, 1000);
    });
    return () => {
      stopSse();
      if (approvalTickTimer) {
        clearInterval(approvalTickTimer);
        approvalTickTimer = null;
      }
    };
  });

  async function approve(queryId: string) {
    try {
      await robsonApi.approveQuery(queryId);
      void load();
    } catch {
      // error handled by next poll refresh
    }
  }

  async function disarm(positionId: string) {
    try {
      await robsonApi.closePosition(positionId);
      void load();
    } catch {
      // next poll will refresh state
    }
  }
</script>

<svelte:head>
  <title>{$_("dashboard.pageTitle")}</title>
</svelte:head>

<div class="dashboard">
  <header class="header">
    <Row justify="between" align="center">
      <Row gap={4} align="center">
        <div class="status-strip">
          {#if error}
            <span class="dot err"></span> {$_("dashboard.offline")} · {error}
          {:else if !connected}
            <span class="dot warn"></span> {$_("dashboard.connecting")}
          {:else}
            <span class="dot live"></span>
            {topBarLabel()} · SLOT {occupied}/{displayedSlots}
          {/if}
        </div>
      </Row>
      <Row gap={3} align="center">
        {#if isHistoricalMonth}
          <button class="btn-entry" onclick={returnToCurrentMonth}>NOW</button>
        {:else if haltState === "monthly_halt"}
          <button class="btn-entry" disabled>HALT</button>
        {:else}
          <button class="btn-entry" onclick={() => (showArmModal = true)}
            >{topBarActionLabel()}</button
          >
        {/if}
      </Row>
    </Row>
  </header>

  {#if error}
    <Card padding={5}>
      <Stack gap={3}>
        <div class="eyebrow">{$_("dashboard.connectionError")}</div>
        <p class="err-text">{error}</p>
        <button class="btn-retry" onclick={retry}
          >{$_("dashboard.retry")}</button
        >
      </Stack>
    </Card>
  {:else}
    {#if insufficientCapital}
      {#if monthlyBudgetExhausted}
        <div class="capital-banner">
          <Row justify="between" align="center">
            <span>{$_("dashboard.monthlyBudgetExhaustedBanner")}</span>
          </Row>
        </div>
      {:else}
        <a href="/funding" class="capital-banner">
          <Row justify="between" align="center">
            <span>{$_("dashboard.insufficientCapitalBanner")}</span>
            <span class="mono">{$_("dashboard.addFunds")} →</span>
          </Row>
        </a>
      {/if}
    {/if}
    <section>
      <Stack gap={4}>
        <div class="eyebrow">RISK DASHBOARD · {monthLabel()}</div>
        {#if !isHistoricalMonth && currentStatus}
          <div class="risk-grid">
            <Card padding={4}>
              <Stack gap={3}>
                <Row justify="between" align="center">
                  <span class="label">MONTHLY LIMIT</span>
                  <span class="mono"
                    >{formatPct(currentStatus.monthly_realized_loss_pct)} of a
                    {monthlyBudgetLimitPct.toFixed(1)}% limit</span
                  >
                <span class="meta dim">Excludes out-of-band drift</span>
                </Row>
                <div
                  class="budget-bar"
                  aria-label="Monthly realized loss budget"
                >
                  <div
                    class="budget-fill"
                    style:width={`${budgetUsedPct}%`}
                  ></div>
                </div>
              </Stack>
            </Card>
            <Card padding={4}>
              <Stack gap={2}>
                <span class="label">GOVERNED REALIZED LOSS</span>
                <span class="loss-value"
                  >{formatMoney(currentStatus.monthly_realized_loss)}</span
                >
                <span class="meta dim"
                  >{formatPct(currentStatus.monthly_realized_loss_pct)} OF
                  {formatMoney(currentStatus.capital_base)}</span
                >
                <span class="meta dim">Governed Robson flow only</span>
              </Stack>
            </Card>
            <Card padding={4}>
              <Stack gap={2}>
                <span class="label">WALLET BALANCE</span>
                <span class="loss-value">{formatMoney(currentStatus.wallet_balance)}</span>
                <span class="meta dim">Current futures wallet on exchange</span>
              </Stack>
            </Card>
          </div>
        {/if}
        <div class="eyebrow">SLOTS · {monthLabel()}</div>
        <div class="slots-grid">
          {#each slots as slot}
            {#if slot.kind === "occupied" && slot.positionId}
              <a
                href={`/operation/${slot.positionId}`}
                class="slot occupied"
                title="Occupied Slot"
                aria-label="Occupied Slot"
              >
                ●
              </a>
            {:else if slot.kind === "free"}
              <div class="slot free" title="Free Slot" aria-label="Free Slot">
                ○
              </div>
            {:else}
              <div
                class="slot expired"
                title="Expired Slot"
                aria-label="Expired Slot"
              >
                ×
              </div>
            {/if}
          {/each}
        </div>
        <div class="eyebrow dim">
          {$_("dashboard.occupied", { values: { count: occupied } })} ·
          {#if isHistoricalMonth}
            EXPIRED {free}
          {:else}
            {$_("dashboard.freeCount", { values: { count: free } })}
          {/if}
        </div>
      </Stack>
    </section>

    <section>
      <Stack gap={4}>
        <Row justify="between" align="center">
          <div class="eyebrow">
            {isHistoricalMonth ? "OPERATIONS" : $_("dashboard.activeOps")} ·
            {monthDisplayLabel(selectedMonth)}
          </div>
          <Row gap={2} align="center">
            <button class="btn-nav" onclick={prevMonth}>←</button>
            <button
              class="btn-nav"
              onclick={nextMonth}
              disabled={selectedMonth === currentMonth}>→</button
            >
          </Row>
        </Row>
        {#if historyError}
          <Card>
            <p class="err-text">{historyError}</p>
          </Card>
        {:else if displayOps.length === 0}
          <Card>
            <p class="empty">
              No positions alive in {monthDisplayLabel(selectedMonth)}
            </p>
          </Card>
        {:else}
          <Grid cols={2} gap={4}>
            {#each displayOps as op}
              {#if op.state === "Armed"}
                <div class="op-card-link">
                  <Card>
                    <Stack gap={2}>
                      <Row justify="between" align="start">
                        <div class="eyebrow">{positionLabel(op)}</div>
                        <span
                          class="state-pill"
                          class:inherited={isInheritedForMonth(
                            op,
                            selectedMonth,
                          )}
                        >
                          {monthStateLabel(op)}
                        </span>
                      </Row>
                      <Row justify="between">
                        <span class="meta">{positionStateLabel(op)}</span>
                      </Row>
                      <div class="meta dim">{positionMetaLine(op)}</div>
                      <pre class="history-summary">{positionSummaryLines(
                          op,
                        ).join("\n")}</pre>
                      <Row justify="end">
                        <button class="btn-disarm" onclick={() => disarm(op.id)}
                          >DISARM</button
                        >
                      </Row>
                    </Stack>
                  </Card>
                </div>
              {:else}
                <a href="/operation/{op.id}" class="op-card-link">
                  <Card>
                    <Stack gap={2}>
                      <Row justify="between" align="start">
                        <div class="eyebrow">{positionLabel(op)}</div>
                        <span
                          class="state-pill"
                          class:inherited={isInheritedForMonth(
                            op,
                            selectedMonth,
                          )}
                        >
                          {monthStateLabel(op)}
                        </span>
                      </Row>
                      <Row justify="between">
                        <span class="meta">{positionStateLabel(op)}</span>
                        {#if variationFor(op) !== null}
                          <span
                            class="mono"
                            class:ok={(variationFor(op) ?? 0) > 0}
                            class:err={(variationFor(op) ?? 0) < 0}
                          >
                            {(variationFor(op) ?? 0) > 0
                              ? "+"
                              : ""}{variationFor(op)?.toFixed(2)}%
                          </span>
                        {/if}
                      </Row>
                      <div class="meta dim">{positionMetaLine(op)}</div>
                      <pre class="history-summary">{positionSummaryLines(
                          op,
                        ).join("\n")}</pre>
                    </Stack>
                  </Card>
                </a>
              {/if}
            {/each}
          </Grid>
        {/if}
      </Stack>
    </section>

    {#if !isHistoricalMonth && pendingApprovals.length > 0}
      <section>
        <Stack gap={4}>
          <div class="eyebrow">
            PENDING APPROVALS · {pendingApprovals.length}
          </div>
          {#each pendingApprovals as approval (approval.query_id)}
            <Card>
              <Row justify="between" align="center">
                <Stack gap={1}>
                  <span class="mono"
                    >{approval.position_id
                      ? approval.position_id.slice(0, 8)
                      : "--------"}</span
                  >
                  <span class="meta">{approval.reason}</span>
                  <span class="meta dim"
                    >expires in {countdownRemaining(approval.expires_at)}</span
                  >
                </Stack>
                <button
                  class="btn-approve"
                  onclick={() => approve(approval.query_id)}>APPROVE</button
                >
              </Row>
            </Card>
          {/each}
        </Stack>
      </section>
    {/if}

    {#if !isHistoricalMonth}
      <section>
        <Stack gap={4}>
          <div class="eyebrow">{$_("dashboard.todayEventsLabel")}</div>
          <Card>
            {#if todayEvents.length === 0}
              <p class="empty">{$_("dashboard.noEventsToday")}</p>
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
  {/if}

  {#if showArmModal}
    <ArmModal
      onclose={() => {
        showArmModal = false;
        void load();
      }}
      onresult={(r) => {
        showToast(`${r.symbol} ${r.side} armed — detector active`, "ok");
      }}
    />
  {/if}

  {#if $toasts.length > 0}
    <div class="toast-container">
      {#each $toasts as t (t.id)}
        <div
          class="toast"
          class:ok={t.kind === "ok"}
          class:err-toast={t.kind === "err"}
        >
          {t.message}
        </div>
      {/each}
    </div>
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
  .capital-banner {
    display: block;
    padding: var(--s-3) var(--s-4);
    background: var(--err-subtle);
    border: 1px solid var(--err);
    border-radius: var(--radius-sm);
    color: var(--fg-0);
    font-family: var(--font-sans);
    font-size: var(--text-sm);
    text-decoration: none;
    transition: background var(--dur) var(--ease);
  }
  .capital-banner:hover {
    background: color-mix(in srgb, var(--err) 12%, var(--err-subtle));
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
  .label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    font-weight: 500;
  }
  .risk-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: var(--s-3);
  }
  .budget-bar {
    height: 12px;
    overflow: hidden;
    background: var(--bg-0);
    border: 1px solid var(--border);
    border-radius: 999px;
  }
  .budget-fill {
    height: 100%;
    min-width: 0;
    background: linear-gradient(90deg, var(--cyan-dim), var(--cyan-brand));
    transition: width var(--dur) var(--ease);
  }
  .loss-value {
    font-family: var(--font-mono);
    font-size: var(--text-lg);
    color: var(--fg-0);
    font-variant-numeric: tabular-nums;
  }
  .slots-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(64px, 64px));
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
  .slot.free {
    color: var(--fg-3);
  }
  .slot.expired {
    color: var(--fg-4);
    border-style: dashed;
    border-color: var(--border-2);
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
  .state-pill.inherited {
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
  .history-summary {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--fg-1);
    white-space: pre-wrap;
    margin: 0;
    line-height: var(--lead-snug);
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
  .btn-entry {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--cyan-brand);
    background: transparent;
    border: 1px solid var(--cyan-dim);
    border-radius: var(--radius-sm);
    padding: var(--s-1) var(--s-3);
    cursor: pointer;
    transition: background var(--dur) var(--ease);
  }
  .btn-entry:hover:not(:disabled) {
    background: var(--cyan-subtle);
  }
  .btn-entry:disabled {
    color: var(--err);
    border-color: var(--err);
    cursor: not-allowed;
    opacity: 0.8;
  }
  /* Sober, always-present treasury link (Zurich): quieter than the primary
     action button — no border, dim until hover. */
  .nav-link {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--fg-2);
    text-decoration: none;
    transition: color var(--dur) var(--ease);
  }
  .nav-link:hover {
    color: var(--cyan-brand);
  }
  .btn-approve {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
    color: var(--cyan-brand);
    background: transparent;
    border: 1px solid var(--cyan-dim);
    border-radius: var(--radius-sm);
    padding: var(--s-1) var(--s-3);
    cursor: pointer;
    transition: background var(--dur) var(--ease);
    white-space: nowrap;
  }
  .btn-approve:hover {
    background: var(--cyan-subtle);
  }
  .btn-disarm {
    background: transparent;
    border: 1px solid var(--color-err, #e55);
    color: var(--color-err, #e55);
    font-family: var(--font-mono);
    font-size: 0.75rem;
    letter-spacing: 0.08em;
    padding: 4px 10px;
    cursor: pointer;
    border-radius: 2px;
    transition: background 0.15s;
  }
  .btn-disarm:hover {
    background: color-mix(in srgb, var(--color-err, #e55) 15%, transparent);
  }
  .btn-nav {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    width: 32px;
    height: 32px;
    border: 1px solid var(--cyan-dim);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--cyan-brand);
    cursor: pointer;
  }
  .btn-nav:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .meta.dim {
    color: var(--fg-3);
  }
  .toast-container {
    position: fixed;
    bottom: var(--s-6);
    right: var(--s-6);
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    z-index: 200;
  }
  .toast {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    letter-spacing: var(--track-wide);
    padding: var(--s-3) var(--s-5);
    border-radius: var(--radius-sm);
    animation: toast-in var(--dur) var(--ease);
  }
  .toast.ok {
    color: var(--ok);
    border: 1px solid var(--ok);
    background: rgba(127, 183, 126, 0.08);
  }
  .toast.err-toast {
    color: var(--err);
    border: 1px solid var(--err);
    background: rgba(197, 106, 106, 0.08);
  }
  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
