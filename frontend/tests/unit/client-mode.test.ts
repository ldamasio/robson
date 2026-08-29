import { describe, expect, it } from "vitest";
import {
  MobileReadOnlyError,
  assertMethodAllowed,
  methodAllowed,
  resolveClientMode,
} from "../../src/lib/config/clientMode";

describe("Android client mode", () => {
  it("defaults unknown builds to the existing operator mode", () => {
    expect(resolveClientMode(undefined)).toBe("operator");
    expect(resolveClientMode("unexpected")).toBe("operator");
  });

  it("allows only safe HTTP methods in mobile read-only mode", () => {
    const mode = resolveClientMode("mobile-readonly");
    expect(methodAllowed(mode, "GET")).toBe(true);
    expect(methodAllowed(mode, "head")).toBe(true);
    expect(methodAllowed(mode, "POST")).toBe(false);
    expect(methodAllowed(mode, "DELETE")).toBe(false);
  });

  it("fails closed before a mobile mutation reaches fetch", () => {
    expect(() =>
      assertMethodAllowed("mobile-readonly", "POST", "/positions"),
    ).toThrow(MobileReadOnlyError);
  });
});
