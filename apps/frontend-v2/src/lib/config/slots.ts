import type { PositionState } from '$api/robson';
import { isPositionActive } from '$lib/presentation/labels';

export const SLOT_COUNT = 6;

export type SlotCell = {
  index: number;
  occupied: boolean;
  positionId: string | null;
  state: PositionState | null;
};

export function deriveSlots(positions: { id: string; state: PositionState }[]): SlotCell[] {
  const active = positions.filter((p) => isPositionActive(p.state));
  const cells: SlotCell[] = [];
  for (let i = 0; i < SLOT_COUNT; i++) {
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
