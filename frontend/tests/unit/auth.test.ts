// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";

vi.mock("$env/dynamic/public", () => ({
  env: { PUBLIC_ROBSON_CLIENT_MODE: "mobile-readonly" },
}));
vi.mock("$app/environment", () => ({ browser: true }));

import { clearAuth, expireAuthIfNeeded, session, setToken } from "$stores/auth";

describe("auth session lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-30T12:00:00Z"));
    clearAuth();
    sessionStorage.clear();
  });

  afterEach(() => {
    clearAuth();
    vi.useRealTimers();
  });

  it("expires an OIDC access token at the provider lifetime", () => {
    setToken("test-access-token", "oidc", 2);
    const deadline = Date.now() + 2_000;

    expect(get(session)).toEqual({
      authenticated: true,
      tokenSource: "oidc",
      expiresAt: deadline,
    });

    vi.advanceTimersByTime(1_999);
    expect(get(session).authenticated).toBe(true);
    vi.advanceTimersByTime(1);
    expect(get(session)).toEqual({
      authenticated: false,
      tokenSource: "none",
      expiresAt: null,
    });
  });

  it("expires a suspended session when the client becomes active", () => {
    setToken("test-access-token", "oidc", 2);
    const deadline = get(session).expiresAt as number;

    expect(expireAuthIfNeeded(deadline - 1)).toBe(false);
    expect(expireAuthIfNeeded(deadline)).toBe(true);
    expect(get(session).authenticated).toBe(false);
  });

  it("rejects an OIDC token without a positive integer lifetime", () => {
    expect(() => setToken("test-access-token", "oidc")).toThrow("invalid");
    expect(() => setToken("test-access-token", "oidc", 0)).toThrow("invalid");
    expect(() => setToken("test-access-token", "oidc", 1.5)).toThrow("invalid");
    expect(get(session).authenticated).toBe(false);
  });

  it("keeps the migration-only legacy token without an inferred lifetime", () => {
    setToken("test-legacy-token", "legacy");
    vi.advanceTimersByTime(60_000);

    expect(get(session)).toEqual({
      authenticated: true,
      tokenSource: "legacy",
      expiresAt: null,
    });
  });
});
