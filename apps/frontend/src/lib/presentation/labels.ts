import type { Position, PositionState, HaltState, SseEvent } from '$api/robson';

export function positionLabel(p: Position): string {
  return `${p.symbol} · ${sideLabel(p.side)}`;
}

export function sideLabel(side: string): string {
  const map: Record<string, string> = { Long: 'Long', Short: 'Short' };
  return map[side] ?? side;
}

export function positionStateLabel(state: PositionState): string {
  if (typeof state === 'string') return state;
  const key = Object.keys(state)[0];
  return key;
}

export function positionSummaryLines(p: Position): string[] {
  const lines: string[] = [];
  const state = p.state;
  const label = (tag: string, detail: string) => `${tag.padEnd(10)}${detail}`;

  if (typeof state === 'string') {
    if (state === 'Armed') {
      lines.push(label('ARMED', 'awaiting entry signal'));
    } else if (state === 'Active') {
      const details = [];
      if (p.entry_price != null) details.push(`entry ${fmtNum(p.entry_price)}`);
      if (p.trailing_stop != null) details.push(`stop ${fmtNum(p.trailing_stop)}`);
      lines.push(label('ACTIVE', details.join(' · ') || 'position open'));
    }
  } else {
    const key = Object.keys(state)[0];
    const val = (state as Record<string, Record<string, unknown>>)[key];

    if (key === 'Entering' && val) {
      lines.push(label('ENTERING', `expected entry ${fmtNum(val.expected_entry as number)}`));
    } else if (key === 'Active' && val) {
      lines.push(label('ACTIVE', `price ${fmtNum(val.current_price as number)} · stop ${fmtNum(val.trailing_stop as number)}`));
      if (val.favorable_extreme) lines.push(label('EXTREME', fmtNum(val.favorable_extreme as number)));
    } else if (key === 'Exiting' && val) {
      lines.push(label('EXITING', `${val.exit_reason}`));
    } else if (key === 'Closed' && val) {
      lines.push(label('CLOSED', `exit ${fmtNum(val.exit_price as number)} · reason ${val.exit_reason}`));
      lines.push(label('PnL', `${fmtPnl(val.realized_pnl as number)}%`));
    } else if (key === 'Error' && val) {
      lines.push(label('ERROR', `${val.error}`));
    }
  }

  if (p.entry_price != null) {
    lines.push(label('ENTRY', fmtNum(p.entry_price)));
  }
  if (p.quantity != null && p.quantity > 0) {
    lines.push(label('SIZE', String(p.quantity)));
  }

  return lines;
}

export function positionMetaLine(p: Position): string {
  const state = positionStateLabel(p.state);
  const created = formatDateUtc(p.created_at);
  const parts = [`State ${state}`, `Created ${created}`];
  if (p.closed_at) parts.push(`Closed ${formatDateUtc(p.closed_at)}`);
  return parts.join(' · ');
}

export function eventSummaryText(event: SseEvent): string {
  const p = event.payload;
  const parts: string[] = [];
  if (p.symbol) parts.push(String(p.symbol));
  if (p.side) parts.push(String(p.side));
  if (p.entry_price) parts.push(`entry ${fmtNum(p.entry_price as number)}`);
  if (p.stop_price) parts.push(`stop ${fmtNum(p.stop_price as number)}`);
  if (p.exit_price) parts.push(`exit ${fmtNum(p.exit_price as number)}`);
  if (p.realized_pnl != null) parts.push(`pnl ${fmtPnl(p.realized_pnl as number)}%`);
  if (p.reason) parts.push(String(p.reason));
  if (p.new_state) parts.push(String(p.new_state));
  return parts.join(' · ');
}

export function haltStateLabel(state: HaltState): string {
  if (state === 'active') return 'Active';
  return 'Monthly Halt';
}

export function haltActionLabel(state: HaltState): string {
  return state === 'active' ? 'Kill Switch' : 'Re-enable';
}

export function eventTypeLabel(event: SseEvent): string {
  return event.event_type.replace(/\./g, ' ').toUpperCase();
}

export function isPositionActive(state: PositionState): boolean {
  if (state === 'Armed' || state === 'Entering' || state === 'Active') return true;
  if (typeof state === 'object') {
    const key = Object.keys(state)[0];
    return key === 'Entering' || key === 'Active';
  }
  return false;
}

function fmtNum(n: number): string {
  return n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function fmtPnl(n: number): string {
  const prefix = n > 0 ? '+' : '';
  return `${prefix}${n.toFixed(2)}`;
}

function formatDateUtc(iso: string): string {
  const d = new Date(iso);
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())} UTC`;
}

function pad(n: number): string {
  return String(n).padStart(2, '0');
}
