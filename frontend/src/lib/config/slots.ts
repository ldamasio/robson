import type { PositionState } from '$api/robson';
import { isPositionActive } from '$lib/presentation/labels';

export type SlotCell = {
  index: number;
  kind: 'occupied' | 'free' | 'expired';
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

export function deriveLiveSlots(positions: SlotPosition[], slotCellsTotal: number): SlotCell[] {
  const active = sortPositionsOldestFirst(positions.filter((p) => isPositionActive(p.state)));
  const count = Math.max(slotCellsTotal, active.length);
  const cells: SlotCell[] = [];

  for (let i = 0; i < count; i++) {
    const pos = active[i];
    const occupied = i < active.length;
    cells.push({
      index: i,
      kind: occupied ? 'occupied' : 'free',
      occupied,
      positionId: pos?.id ?? null,
      state: pos?.state ?? null
    });
  }
  return cells;
}

export function deriveHistoricalSlots(
  positions: SlotPosition[],
  slotCellsTotal: number,
): SlotCell[] {
  return deriveMonthSlots(positions, slotCellsTotal, "expired");
}

export function deriveMonthSlots(
  positions: SlotPosition[],
  slotCellsTotal: number,
  emptyKind: "free" | "expired",
): SlotCell[] {
  const sorted = sortPositionsOldestFirst(positions);
  const cells: SlotCell[] = [];
  const expiredCount = Math.max(slotCellsTotal - sorted.length, 0);

  for (let i = 0; i < sorted.length; i++) {
    const pos = sorted[i];
    cells.push({
      index: i,
      kind: 'occupied',
      occupied: true,
      positionId: pos?.id ?? null,
      state: pos?.state ?? null,
    });
  }

  for (let i = 0; i < expiredCount; i++) {
    cells.push({
      index: sorted.length + i,
      kind: emptyKind,
      occupied: false,
      positionId: null,
      state: null,
    });
  }

  return cells;
}

export function deriveSlots(positions: SlotPosition[], slotCellsTotal: number): SlotCell[] {
  return deriveLiveSlots(positions, slotCellsTotal);
}

export function occupiedCount(positions: { state: PositionState }[]): number {
  return positions.filter((p) => isPositionActive(p.state)).length;
}
