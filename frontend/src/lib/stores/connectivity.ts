import { browser } from "$app/environment";
import { writable } from "svelte/store";

export type ConnectivityState = {
  initialized: boolean;
  online: boolean;
};

export function createConnectivityStore() {
  const { subscribe, set } = writable<ConnectivityState>({
    initialized: false,
    online: true,
  });
  let started = false;

  function update() {
    if (!browser) return;
    set({ initialized: true, online: navigator.onLine });
  }

  function start() {
    if (!browser || started) return;
    started = true;
    update();
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
  }

  function stop() {
    if (!browser || !started) return;
    window.removeEventListener("online", update);
    window.removeEventListener("offline", update);
    started = false;
  }

  return { subscribe, start, stop, refresh: update };
}

export const connectivity = createConnectivityStore();
export const startConnectivity = connectivity.start;
export const stopConnectivity = connectivity.stop;
export const refreshConnectivity = connectivity.refresh;
