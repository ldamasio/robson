import { browser } from "$app/env";
import * as env from "$app/env/public";
import { get, writable } from "svelte/store";
import {
  clearNativeAuthSession,
  getNativeAuthSession,
  isNativeRuntime,
  onNativeAuthExpired,
  onNativeAuthSession,
} from "#lib/auth/native.ts";

export type AuthSource = "native-oidc" | "web-oidc" | "bff-cookie" | "none";

export type Session = {
  authenticated: boolean;
  initialized: boolean;
  tokenSource: AuthSource;
  expiresAt: number | null;
};

const initialSession: Session = {
  authenticated: false,
  initialized: false,
  tokenSource: "none",
  expiresAt: null,
};

/** Access tokens exist here only for the browser public-client flow.
 *
 * Tauri keeps its token exclusively in Rust memory and BFF mode uses an
 * HttpOnly cookie. Nothing in this store writes a credential to Web Storage.
 */
export const authToken = writable<string | null>(null);
export const session = writable<Session>(initialSession);

let expirationTimer: ReturnType<typeof setTimeout> | null = null;
let initPromise: Promise<void> | null = null;

export function webAuthMode(): "oidc" | "bff" {
  return env.PUBLIC_ROBSON_WEB_AUTH_MODE === "bff" ? "bff" : "oidc";
}

export function initAuth(): Promise<void> {
  if (!browser) return Promise.resolve();
  if (initPromise) return initPromise;
  initPromise = initialize().catch(() => {
    setUnauthenticated(true);
  });
  return initPromise;
}

async function initialize(): Promise<void> {
  if (isNativeRuntime()) {
    await Promise.all([
      onNativeAuthSession((nativeSession) => {
        session.set({
          authenticated: nativeSession.authenticated,
          initialized: true,
          tokenSource: nativeSession.authenticated ? "native-oidc" : "none",
          expiresAt: nativeSession.expiresAt,
        });
        scheduleExpiration(nativeSession.expiresAt);
      }),
      onNativeAuthExpired(() => setUnauthenticated(true)),
    ]);
    const nativeSession = await getNativeAuthSession();
    session.set({
      authenticated: nativeSession.authenticated,
      initialized: true,
      tokenSource: nativeSession.authenticated ? "native-oidc" : "none",
      expiresAt: nativeSession.expiresAt,
    });
    scheduleExpiration(nativeSession.expiresAt);
    return;
  }

  if (webAuthMode() === "bff") {
    const base = env.PUBLIC_ROBSON_API_BASE ?? "";
    const response = await fetch(`${base}/auth/session`, {
      credentials: "include",
      headers: { Accept: "application/json" },
    });
    if (response.ok) {
      session.set({
        authenticated: true,
        initialized: true,
        tokenSource: "bff-cookie",
        expiresAt: null,
      });
      return;
    }
  }

  // Browser PKCE access tokens are deliberately memory-only. A reload starts
  // a new authorization flow; the ZITADEL/Google SSO session makes it cheap.
  setUnauthenticated(true);
}

export function setWebOidcToken(token: string, expiresInSeconds: number): void {
  if (
    !token ||
    !Number.isSafeInteger(expiresInSeconds) ||
    expiresInSeconds <= 0
  ) {
    throw new Error("OIDC access token lifetime is invalid");
  }
  const expiresAt = Date.now() + expiresInSeconds * 1_000;
  if (!Number.isSafeInteger(expiresAt)) {
    throw new Error("OIDC access token lifetime is invalid");
  }
  authToken.set(token);
  session.set({
    authenticated: true,
    initialized: true,
    tokenSource: "web-oidc",
    expiresAt,
  });
  scheduleExpiration(expiresAt);
}

export function markBffAuthenticated(): void {
  authToken.set(null);
  session.set({
    authenticated: true,
    initialized: true,
    tokenSource: "bff-cookie",
    expiresAt: null,
  });
  scheduleExpiration(null);
}

export function currentAccessToken(): string | null {
  return get(authToken);
}

export function clearAuth(): void {
  if (browser && isNativeRuntime()) {
    void clearNativeAuthSession().catch(() => {});
  }
  setUnauthenticated(true);
}

function setUnauthenticated(initialized: boolean): void {
  if (expirationTimer !== null) {
    clearTimeout(expirationTimer);
    expirationTimer = null;
  }
  authToken.set(null);
  session.set({
    authenticated: false,
    initialized,
    tokenSource: "none",
    expiresAt: null,
  });
}

function scheduleExpiration(expiresAt: number | null): void {
  if (expirationTimer !== null) clearTimeout(expirationTimer);
  expirationTimer = null;
  if (!browser || expiresAt === null) return;
  const remaining = expiresAt - Date.now();
  if (remaining <= 0) {
    setUnauthenticated(true);
    return;
  }
  expirationTimer = setTimeout(() => setUnauthenticated(true), remaining);
}
