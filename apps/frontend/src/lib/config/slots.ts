import type { PositionState } from '$api/robson';
import { isPositionActive } from '$lib/presentation/labels';

export type SlotCell = {
  index: number;
  occupied: boolean;
  positionId: string | null;
  state: PositionState | null;
};

type SlotPosition = {
  id: string;
  state: PositionState;
  created_at?: string | null;
};

function timestampOrInfinity(value?: string | null): number {
  if (!value) return Number.POSITIVE_INFINITY;
  const ts = Date.parse(value);
  return Number.isFinite(ts) ? ts : Number.POSITIVE_INFINITY;
}

export function sortPositionsOldestFirst<T extends { created_at?: string | null }>(positions: T[]): T[] {
  return [...positions].sort((a, b) => timestampOrInfinity(a.created_at) - timestampOrInfinity(b.created_at));
}

export function deriveSlots(positions: SlotPosition[], slotCellsTotal: number): SlotCell[] {
  const active = sortPositionsOldestFirst(positions.filter((p) => isPositionActive(p.state)));
  const count = Math.max(slotCellsTotal, active.length);
  const cells: SlotCell[] = [];

  for (let i = 0; i < count; i++) {
    const pos = active[i];
    cells.push({
      index: i,
      occupied: i < active.length,
      positionId: pos?.id ?? null,
      state: pos?.state ?? null
    });
  }
  return cells;
}

export function occupiedCount(positions: { state: PositionState }[]): number {
  return positions.filter((p) => isPositionActive(p.state)).length;
}
