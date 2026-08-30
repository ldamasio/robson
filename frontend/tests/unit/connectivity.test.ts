// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";

vi.mock("$app/environment", () => ({ browser: true }));

import { createConnectivityStore } from "$stores/connectivity";

describe("connectivity store", () => {
  let online = true;

  beforeEach(() => {
    online = true;
    vi.spyOn(window.navigator, "onLine", "get").mockImplementation(
      () => online,
    );
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("initializes from the browser network hint", () => {
    const store = createConnectivityStore();
    store.start();

    expect(get(store)).toEqual({ initialized: true, online: true });
    store.stop();
  });

  it("tracks offline and online browser events", () => {
    const store = createConnectivityStore();
    store.start();

    online = false;
    window.dispatchEvent(new Event("offline"));
    expect(get(store).online).toBe(false);

    online = true;
    window.dispatchEvent(new Event("online"));
    expect(get(store).online).toBe(true);
    store.stop();
  });

  it("refreshes the hint when the application resumes", () => {
    const store = createConnectivityStore();
    store.start();

    online = false;
    store.refresh();
    expect(get(store).online).toBe(false);
    store.stop();
  });

  it("removes browser listeners when stopped", () => {
    const store = createConnectivityStore();
    store.start();
    store.stop();

    online = false;
    window.dispatchEvent(new Event("offline"));
    expect(get(store).online).toBe(true);
  });
});
