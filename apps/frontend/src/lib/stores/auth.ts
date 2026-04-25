import { writable, derived } from 'svelte/store';
import { browser } from '$app/environment';

export type Session = {
  authenticated: boolean;
  tokenSource: 'stored' | 'session' | 'none';
};

const STORAGE_KEY = 'robson_api_token';

function createAuthStore() {
  const token = writable<string | null>(null);
  const session = derived(token, ($token): Session => {
    if (!$token) return { authenticated: false, tokenSource: 'none' };
    return { authenticated: true, tokenSource: 'stored' };
  });

  function init() {
    if (!browser) return;
    const stored = sessionStorage.getItem(STORAGE_KEY);
    if (stored) {
      token.set(stored);
    }
  }

  function setToken(t: string) {
    if (browser) sessionStorage.setItem(STORAGE_KEY, t);
    token.set(t);
  }

  function clear() {
    if (browser) sessionStorage.removeItem(STORAGE_KEY);
    token.set(null);
  }

  return { token, session, init, setToken, clear };
}

export const { token: authToken, session, init: initAuth, setToken, clear: clearAuth } = createAuthStore();
