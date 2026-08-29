import { writable, derived } from "svelte/store";
import { browser } from "$app/environment";
import { env } from "$env/dynamic/public";
import { resolveClientMode } from "$lib/config/clientMode";

export type Session = {
  authenticated: boolean;
  tokenSource: "stored" | "session" | "none";
};

const STORAGE_KEY = "robson_api_token";
const persistToken =
  resolveClientMode(env.PUBLIC_ROBSON_CLIENT_MODE) === "operator";

function createAuthStore() {
  const token = writable<string | null>(null);
  const session = derived(token, ($token): Session => {
    if (!$token) return { authenticated: false, tokenSource: "none" };
    return { authenticated: true, tokenSource: "stored" };
  });

  function init() {
    if (!browser || !persistToken) return;
    const stored = sessionStorage.getItem(STORAGE_KEY);
    if (stored) {
      token.set(stored);
    }
  }

  function setToken(t: string) {
    if (browser && persistToken) sessionStorage.setItem(STORAGE_KEY, t);
    token.set(t);
  }

  function clear() {
    if (browser && persistToken) sessionStorage.removeItem(STORAGE_KEY);
    token.set(null);
  }

  return { token, session, init, setToken, clear };
}

export const {
  token: authToken,
  session,
  init: initAuth,
  setToken,
  clear: clearAuth,
} = createAuthStore();
