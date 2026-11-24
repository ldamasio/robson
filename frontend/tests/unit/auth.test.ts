// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";

vi.mock("$app/env/public", () => ({}));
vi.mock("$app/env", () => ({ browser: true }));
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => false,
  invoke: vi.fn(),
}));

import { authToken, clearAuth, session, setWebOidcToken } from "$stores/auth";

describe("credential lifecycle", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-24T12:00:00Z"));
    clearAuth();
    localStorage.clear();
    sessionStorage.clear();
  });

  afterEach(() => {
    clearAuth();
    vi.useRealTimers();
  });

  it("keeps a browser OIDC token in memory only", () => {
    setWebOidcToken("signed-access-token", 2);

    expect(get(authToken)).toBe("signed-access-token");
    expect(get(session)).toMatchObject({
      authenticated: true,
      initialized: true,
      tokenSource: "web-oidc",
    });
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);

    vi.advanceTimersByTime(2_000);
    expect(get(authToken)).toBeNull();
    expect(get(session).authenticated).toBe(false);
  });

  it("rejects an invalid provider lifetime", () => {
    expect(() => setWebOidcToken("token", 0)).toThrow("invalid");
    expect(() => setWebOidcToken("token", 1.5)).toThrow("invalid");
    expect(get(session).authenticated).toBe(false);
  });
});
