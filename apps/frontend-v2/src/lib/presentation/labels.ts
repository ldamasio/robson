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
  if (state === 'Armed') return true;
  if (typeof state === 'object') {
    const key = Object.keys(state)[0];
    return key === 'Entering' || key === 'Active';
  }
  return false;
}
