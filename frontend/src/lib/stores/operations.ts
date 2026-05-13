import { writable } from 'svelte/store';
import type { Position } from '$api/robson';

export const activePositions = writable<Position[]>([]);

export function upsertPosition(position: Position) {
  activePositions.update((prev) => {
    const idx = prev.findIndex((p) => p.id === position.id);
    if (idx >= 0) {
      const next = [...prev];
      next[idx] = position;
      return next;
    }
    return [position, ...prev];
  });
}

export function removePosition(id: string) {
  activePositions.update((prev) => prev.filter((p) => p.id !== id));
}
