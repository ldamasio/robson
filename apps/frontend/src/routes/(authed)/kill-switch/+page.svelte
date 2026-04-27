<script lang="ts">
  import Card from '$design/components/Card.svelte';
  import Stack from '$design/components/Stack.svelte';
  import Row from '$design/components/Row.svelte';
  import { robsonApi, type MonthlyHaltStatus, type StatusResponse, type ApiError } from '$api/robson';
  import { haltStatus } from '$stores/slots';
  import { _ } from 'svelte-i18n';
  import { positionLabel, positionStateLabel } from '$lib/presentation/labels';
  import { INITIAL_MONTHLY_SLOT_BUDGET } from '$lib/config/slots';

  type PageState = 'loading' | 'active' | 'halted' | 'error';

  let pageState: PageState = $state('loading');
  let halt: MonthlyHaltStatus | null = $state(null);
  let status: StatusResponse | null = $state(null);
  let errorMsg = $state<string | null>(null);
  let confirmInput = $state('');
  let reasonInput = $state('');
  let submitting = $state(false);

  let keyword = $derived($_('killSwitch.disableKeyword') ?? 'DESLIGAR');

  let canConfirm = $derived(
    confirmInput === keyword && reasonInput.trim().length > 0 && !submitting
  );
  let activeCount: number = $derived.by(() => {
    if (status) return status.active_positions;
    return 0;
  });
  let affectedPositions = $derived.by(() => {
    if (!status) return [];
    return status.positions;
  });

  async function loadState() {
    errorMsg = null;
    try {
      const [haltData, statusData] = await Promise.all([
        robsonApi.getHaltStatus(),
        robsonApi.getStatus()
      ]);
      halt = haltData;
      status = statusData;
      haltStatus.set(haltData);
      pageState = haltData.state === 'monthly_halt' ? 'halted' : 'active';
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : 'Failed to load state';
      pageState = 'error';
    }
  }

  async function triggerHalt() {
    if (!canConfirm) return;
    submitting = true;
    errorMsg = null;
    try {
      const result = await robsonApi.triggerHalt(reasonInput.trim());
      halt = result;
      haltStatus.set(result);
      pageState = 'halted';
    } catch (e) {
      const apiErr = e as ApiError;
      errorMsg = apiErr?.message ?? 'Failed to trigger MonthlyHalt';
    } finally {
      submitting = false;
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

  $effect(() => {
    void loadState();
  });
</script>

<svelte:head>
  <title>Kill Switch — RBX Robson</title>
</svelte:head>

<div class="ks-page">
  {#if pageState === 'loading'}
    <Card padding={7}>
      <div class="eyebrow">KILL SWITCH</div>
      <p class="muted">{$_('killSwitch.loading') ?? 'Loading...'}</p>
    </Card>

  {:else if pageState === 'error'}
    <Card padding={7}>
      <Stack gap={4}>
        <div class="eyebrow">KILL SWITCH</div>
        <p class="err-text">{errorMsg}</p>
        <button class="btn-retry" onclick={loadState}>Retry</button>
      </Stack>
    </Card>

  {:else if pageState === 'active'}
    <Card padding={7}>
      <Stack gap={5}>
        <div class="eyebrow">KILL SWITCH</div>
        <h1>{$_('killSwitch.disableTitle')}</h1>

        <div class="current-state">
          <div class="eyebrow">{$_('killSwitch.currentState')}</div>
          <div class="state-line">
            <span class="dot live"></span>
            ACTIVE · {$_('killSwitch.slotLabel')} {activeCount}/{Math.max(INITIAL_MONTHLY_SLOT_BUDGET, activeCount)} · {activeCount} {activeCount === 1 ? ($_('killSwitch.openPositions_one') ?? 'OPEN POSITION') : ($_('killSwitch.openPositions_other') ?? 'OPEN POSITIONS')}
          </div>
        </div>

        {#if affectedPositions.length > 0}
          <div class="positions-preview">
            <div class="eyebrow">{$_('killSwitch.affectedPositions') ?? 'AFFECTED POSITIONS'}</div>
            <div class="pos-list">
              {#each affectedPositions as pos}
                <div class="pos-row">
                  <span class="pos-label">{positionLabel(pos)}</span>
                  <span class="pos-state">{positionStateLabel(pos.state)}</span>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <div class="warning-block">
          <p>{$_('killSwitch.triggerWarning')}</p>
          <p>{$_('killSwitch.persistNote')}</p>
        </div>

        <Stack gap={3}>
          <label class="eyebrow" for="reason-input">
            {$_('killSwitch.reasonLabel')}
          </label>
          <textarea
            id="reason-input"
            bind:value={reasonInput}
            placeholder={$_('killSwitch.reasonPlaceholder') ?? ''}
            rows={3}
            class="reason-input"
          ></textarea>
        </Stack>

        <Stack gap={2}>
          <label class="eyebrow" for="confirm-input">
            {$_('killSwitch.typeToConfirm')}
          </label>
          <input
            id="confirm-input"
            type="text"
            bind:value={confirmInput}
            autocomplete="off"
            spellcheck="false"
            class="confirm-input"
          />
        </Stack>

        {#if errorMsg}
          <p class="err-text">{errorMsg}</p>
        {/if}

        <Row gap={3} justify="end">
          <a href="/dashboard" class="btn-cancel">{$_('killSwitch.cancel')}</a>
          <button
            class="btn-confirm"
            class:ready={canConfirm}
            disabled={!canConfirm}
            onclick={triggerHalt}
          >
            {#if submitting}
              {$_('killSwitch.submitting') ?? '...'}
            {:else}
              {$_('killSwitch.disable')}
            {/if}
          </button>
        </Row>
      </Stack>
    </Card>

  {:else if pageState === 'halted'}
    <Card padding={7}>
      <Stack gap={5}>
        <div class="eyebrow">KILL SWITCH</div>
        <h1>{$_('killSwitch.haltedTitle')}</h1>

        <div class="halted-state">
          <div class="eyebrow">{$_('killSwitch.currentState')}</div>
          <div class="state-line">
            <span class="dot halted"></span>
            {$_('killSwitch.monthlyHaltActive') ?? 'MONTHLY HALT ACTIVE'}
          </div>
        </div>

        <div class="halted-detail">
          <Stack gap={3}>
            {#if halt?.triggered_at}
              <div>
                <span class="eyebrow">{$_('killSwitch.triggeredAt')}</span>
                <div class="mono ts">{formatTimestamp(halt.triggered_at)}</div>
              </div>
            {/if}
            {#if halt?.reason}
              <div>
                <span class="eyebrow">{$_('killSwitch.reason')}</span>
                <div class="reason-text">{halt.reason}</div>
              </div>
            {/if}
            <div>
              <span class="eyebrow">{$_('killSwitch.description')}</span>
              <div class="reason-text">{halt?.description ?? '—'}</div>
            </div>
            <div>
              <span class="eyebrow">{$_('killSwitch.blocks')}</span>
              <div class="mono meta">
                New entries: {halt?.blocks_new_entries ? 'YES' : 'NO'}
                · Signals: {halt?.blocks_signals ? 'YES' : 'NO'}
              </div>
            </div>
          </Stack>
        </div>

        <div class="info-block">
          <p>{$_('killSwitch.haltedInfo')}</p>
        </div>
      </Stack>
    </Card>
  {/if}
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
  .current-state,
  .halted-state {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    padding: var(--s-4);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .halted-state {
    border-color: var(--err);
    background: var(--err-subtle);
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
    display: inline-block;
  }
  .dot.live {
    background: var(--ok);
  }
  .dot.halted {
    background: var(--err);
  }
  .muted {
    color: var(--fg-3);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
  }
  .positions-preview {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    padding: var(--s-4);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .pos-list {
    display: flex;
    flex-direction: column;
    gap: var(--s-1);
  }
  .pos-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--fg-1);
  }
  .pos-label {
    color: var(--fg-0);
  }
  .pos-state {
    color: var(--fg-2);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: var(--track-label);
  }
  .warning-block {
    padding: var(--s-4);
    background: var(--err-subtle);
    border: 1px solid var(--err);
    border-radius: var(--radius-sm);
  }
  .warning-block p {
    color: var(--fg-1);
    font-size: var(--text-sm);
    margin: 0;
  }
  .warning-block p + p {
    margin-top: var(--s-2);
  }
  .info-block p {
    color: var(--fg-2);
    font-size: var(--text-sm);
    margin: 0;
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .ts {
    font-size: var(--text-sm);
    color: var(--fg-0);
    letter-spacing: var(--track-wide);
    margin-top: var(--s-1);
  }
  .meta {
    font-size: var(--text-xs);
    color: var(--fg-2);
    letter-spacing: var(--track-wide);
    margin-top: var(--s-1);
  }
  .reason-text {
    color: var(--fg-1);
    font-size: var(--text-sm);
    margin-top: var(--s-1);
  }
  .halted-detail {
    padding: var(--s-4);
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  p {
    color: var(--fg-1);
    font-size: var(--text-base);
  }
  .reason-input {
    font-family: var(--font-sans);
    font-size: var(--text-sm);
    padding: var(--s-3);
    background: var(--bg-3);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--fg-0);
    outline: none;
    resize: vertical;
    width: 100%;
    box-sizing: border-box;
  }
  .reason-input:focus {
    border-color: var(--cyan-brand);
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
  .err-text {
    color: var(--err, #ff4444);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    word-break: break-word;
  }
</style>
