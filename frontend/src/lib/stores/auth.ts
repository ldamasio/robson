import { writable, derived } from 'svelte/store';
import { browser } from '$app/environment';

export type Session = {
  authenticated: boolean;
  tokenSource: 'stored' | 'session' | 'none';
};

// ADR-0054: the stored credential is now a Google-issued ID token (a JWT),
// not an opaque bearer token issued by robsond. Renamed so a stale token
// from before the cutover isn't mistaken for a valid one.
const STORAGE_KEY = 'robson_google_id_token';

// Google ID tokens are typically valid for ~1h. Trigger a silent refresh
// this many seconds before expiry, matching the plan's "~10 minutes left"
// recommendation.
export const SILENT_REFRESH_THRESHOLD_SECS = 600;

/** Google Identity Services' shape for the pieces this app uses. Declared
 *  narrowly here rather than pulling in a full type package for one global. */
type GoogleCredentialResponse = { credential?: string };
type GoogleIdConfig = {
  client_id: string;
  callback: (response: GoogleCredentialResponse) => void;
};
type GooglePromptMoment = {
  isNotDisplayed?: () => boolean;
  isSkippedMoment?: () => boolean;
  isDismissedMoment?: () => boolean;
};
type GoogleAccountsId = {
  initialize: (config: GoogleIdConfig) => void;
  renderButton: (parent: HTMLElement, options: Record<string, unknown>) => void;
  prompt: (momentListener?: (notification: GooglePromptMoment) => void) => void;
  cancel: () => void;
};

declare global {
  interface Window {
    google?: { accounts: { id: GoogleAccountsId } };
  }
}

/** Base64url-decode a JWT's payload segment and parse the `exp` claim.
 *  No signature verification is performed here — this is purely
 *  informational, used only to drive the client-side silent-refresh timer.
 *  robsond independently re-verifies the signature and every claim on
 *  every request (see `robsond/src/auth.rs`); a forged `exp` client-side
 *  gains an attacker nothing. */
export function decodeTokenExpiry(token: string): number | null {
  try {
    const payload = token.split('.')[1];
    if (!payload) return null;
    const base64 = payload.replace(/-/g, '+').replace(/_/g, '/');
    const padded = base64 + '='.repeat((4 - (base64.length % 4)) % 4);
    const json = atob(padded);
    const claims = JSON.parse(json) as { exp?: unknown };
    return typeof claims.exp === 'number' ? claims.exp : null;
  } catch {
    return null;
  }
}

/** True when `token`'s `exp` claim is within `thresholdSecs` of now, or
 *  can't be determined at all (fail safe: treat as needing a refresh). */
export function isNearExpiry(
  token: string,
  thresholdSecs: number = SILENT_REFRESH_THRESHOLD_SECS,
): boolean {
  const exp = decodeTokenExpiry(token);
  if (exp === null) return true;
  return exp - Date.now() / 1000 <= thresholdSecs;
}

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

  // Google only allows one `navigator.credentials.get()` (FedCM) call
  // outstanding at a time — a second concurrent call rejects with
  // NotAllowedError, and re-calling `initialize()` while one is pending
  // logs GIS's own "called multiple times" warning. `apiFetch`'s 401
  // handler, `FetchEventSource`'s 401 handler, and the periodic
  // near-expiry timer in `(authed)/+layout.svelte` can all independently
  // decide to refresh around the same time (e.g. several dashboard
  // requests landing together right after mount) — without sharing one
  // in-flight attempt, each caller raced its own `initialize()`/`prompt()`
  // against the others', producing a self-sustaining loop of these
  // warnings. This holds the one in-flight attempt so every concurrent
  // caller awaits the same result instead of starting a new one.
  let inFlightRefresh: Promise<string | null> | null = null;

  /** Ask Google Identity Services for a fresh credential in the background
   *  (`google.accounts.id.prompt()`), without forcing a visible click.
   *  Resolves with the new token on success — via the same `setToken` path
   *  as the login page's button flow — or `null` if GIS is unavailable or
   *  declines to issue one silently (e.g. third-party cookies blocked,
   *  session revoked, no active Google session). Callers should treat
   *  `null` as "let the normal expired-token/401 handling take over"
   *  rather than as an error. Safe to call concurrently from multiple call
   *  sites — see `inFlightRefresh` above. */
  function silentRefresh(clientId: string): Promise<string | null> {
    if (!browser || !window.google?.accounts?.id) return Promise.resolve(null);
    if (inFlightRefresh) return inFlightRefresh;

    inFlightRefresh = new Promise<string | null>((resolve) => {
      let settled = false;
      const settle = (value: string | null) => {
        if (settled) return;
        settled = true;
        resolve(value);
      };

      window.google!.accounts.id.initialize({
        client_id: clientId,
        callback: (response) => {
          if (response?.credential) {
            setToken(response.credential);
            settle(response.credential);
          } else {
            settle(null);
          }
        },
      });

      window.google!.accounts.id.prompt((notification) => {
        if (
          notification?.isNotDisplayed?.() ||
          notification?.isSkippedMoment?.() ||
          notification?.isDismissedMoment?.()
        ) {
          settle(null);
        }
      });

      // Safety timeout: GIS's moment listener isn't guaranteed to fire in
      // every browser/state combination — never hang the caller forever.
      // Waiting it out passively isn't enough, though: Google's own docs
      // say the underlying FedCM `navigator.credentials.get()` this
      // triggers can take up to a minute to notify (or never notify at
      // all), so merely *timing out* on our end doesn't mean that
      // browser-level call has actually finished — releasing
      // `inFlightRefresh` at that point could let a second concurrent
      // caller start another `initialize()`/`prompt()` cycle into it,
      // reproducing the exact NotAllowedError race this lock exists to
      // prevent (confirmed by a Codex review round on an earlier version
      // of this fix that just extended the timeout instead). So this
      // explicitly cancels the pending GIS operation first — the
      // documented way to actually terminate it — before releasing the
      // lock, rather than assuming it settled on its own.
      setTimeout(() => {
        window.google?.accounts?.id?.cancel?.();
        settle(null);
      }, 5000);
    }).finally(() => {
      inFlightRefresh = null;
    });

    return inFlightRefresh;
  }

  return { token, session, init, setToken, clear, silentRefresh };
}

export const {
  token: authToken,
  session,
  init: initAuth,
  setToken,
  clear: clearAuth,
  silentRefresh: silentRefreshGoogleToken,
} = createAuthStore();
