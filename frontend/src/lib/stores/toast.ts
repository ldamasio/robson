import { writable } from 'svelte/store';

export type Toast = {
  id: number;
  message: string;
  kind: 'ok' | 'err';
};

let counter = 0;

export const toasts = writable<Toast[]>([]);

export function showToast(message: string, kind: Toast['kind'] = 'ok') {
  const id = ++counter;
  toasts.update((prev) => [...prev, { id, message, kind }]);
  setTimeout(() => {
    toasts.update((prev) => prev.filter((t) => t.id !== id));
  }, 4000);
}
