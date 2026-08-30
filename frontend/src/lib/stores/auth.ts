import { writable, derived } from "svelte/store";
import { browser } from "$app/environment";
import { env } from "$env/dynamic/public";
import { resolveClientMode } from "$lib/config/clientMode";

export type Session = {
  authenticated: boolean;
  tokenSource: "legacy" | "oidc" | "none";
  expiresAt: number | null;
};

const STORAGE_KEY = "robson_api_token";
const MAX_TIMER_DELAY_MS = 2_147_483_647;
const persistToken =
  resolveClientMode(env.PUBLIC_ROBSON_CLIENT_MODE) === "operator";

export function createAuthStore() {
  const token = writable<string | null>(null);
  const source = writable<Session["tokenSource"]>("none");
  const expiresAt = writable<number | null>(null);
  let expirationDeadline: number | null = null;
  let expirationTimer: ReturnType<typeof setTimeout> | null = null;
  const session = derived(
    [token, source, expiresAt],
    ([$token, $source, $expiresAt]): Session => {
      if (!$token) {
        return {
          authenticated: false,
          tokenSource: "none",
          expiresAt: null,
        };
      }
      return {
        authenticated: true,
        tokenSource: $source,
        expiresAt: $expiresAt,
      };
    },
  );

  function clearExpirationTimer() {
    if (expirationTimer !== null) {
      clearTimeout(expirationTimer);
      expirationTimer = null;
    }
  }

  function scheduleExpiration() {
    clearExpirationTimer();
    if (!browser || expirationDeadline === null) return;
    const delay = expirationDeadline - Date.now();
    if (delay <= 0) {
      clear();
      return;
    }
    expirationTimer = setTimeout(
      () => {
        expirationTimer = null;
        if (!expireIfNeeded()) scheduleExpiration();
      },
      Math.min(delay, MAX_TIMER_DELAY_MS),
    );
  }

  function init() {
    if (!browser || !persistToken) return;
    clearExpirationTimer();
    expirationDeadline = null;
    expiresAt.set(null);
    const stored = sessionStorage.getItem(STORAGE_KEY);
    if (stored) {
      token.set(stored);
      source.set("legacy");
    }
  }

  function setToken(
    t: string,
    tokenSource: "legacy" | "oidc" = "legacy",
    expiresInSeconds?: number,
  ) {
    let nextExpiration: number | null = null;
    if (tokenSource === "oidc") {
      if (
        !Number.isSafeInteger(expiresInSeconds) ||
        (expiresInSeconds ?? 0) <= 0
      ) {
        throw new Error("OIDC access token lifetime is invalid");
      }
      const lifetimeMs = (expiresInSeconds as number) * 1_000;
      nextExpiration = Date.now() + lifetimeMs;
      if (!Number.isSafeInteger(nextExpiration)) {
        throw new Error("OIDC access token lifetime is invalid");
      }
    }

    clearExpirationTimer();
    if (browser && persistToken) {
      if (tokenSource === "legacy") sessionStorage.setItem(STORAGE_KEY, t);
      else sessionStorage.removeItem(STORAGE_KEY);
    }
    expirationDeadline = nextExpiration;
    token.set(t);
    source.set(tokenSource);
    expiresAt.set(nextExpiration);
    scheduleExpiration();
  }

  function clear() {
    clearExpirationTimer();
    if (browser && persistToken) sessionStorage.removeItem(STORAGE_KEY);
    expirationDeadline = null;
    token.set(null);
    source.set("none");
    expiresAt.set(null);
  }

  function expireIfNeeded(now = Date.now()): boolean {
    if (expirationDeadline === null || now < expirationDeadline) return false;
    clear();
    return true;
  }

  return { token, session, init, setToken, clear, expireIfNeeded };
}

export const {
  token: authToken,
  session,
  init: initAuth,
  setToken,
  clear: clearAuth,
  expireIfNeeded: expireAuthIfNeeded,
} = createAuthStore();
