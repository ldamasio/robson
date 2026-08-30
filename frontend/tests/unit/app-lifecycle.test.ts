import { beforeEach, describe, expect, it, vi } from "vitest";

const { getPlatform, minimizeApp } = vi.hoisted(() => ({
  getPlatform: vi.fn(),
  minimizeApp: vi.fn(),
}));

vi.mock("@capacitor/core", () => ({
  Capacitor: { getPlatform },
}));
vi.mock("@capacitor/app", () => ({
  App: { minimizeApp },
}));

import { isAndroidNativeApp, leaveAndroidApp } from "$lib/mobile/appLifecycle";

describe("native application lifecycle", () => {
  beforeEach(() => {
    getPlatform.mockReset();
    minimizeApp.mockReset();
  });

  it("minimizes the Android app through the Capacitor lifecycle API", async () => {
    getPlatform.mockReturnValue("android");

    await expect(isAndroidNativeApp()).resolves.toBe(true);
    await expect(leaveAndroidApp()).resolves.toBe(true);
    expect(minimizeApp).toHaveBeenCalledOnce();
  });

  it("does not expose the Android action on the web", async () => {
    getPlatform.mockReturnValue("web");

    await expect(isAndroidNativeApp()).resolves.toBe(false);
    await expect(leaveAndroidApp()).resolves.toBe(false);
    expect(minimizeApp).not.toHaveBeenCalled();
  });
});
