import { describe, it, expect } from "vitest";
import { formatTimeUtc, isTodayUtc, utcDateKey } from "$lib/utils/time";

describe("formatTimeUtc", () => {
  it("formats ISO8601 as HH:mm:ss.SSS UTC", () => {
    // 2026-04-23T14:22:18.441Z -> 14:22:18.441
    expect(formatTimeUtc("2026-04-23T14:22:18.441Z")).toBe("14:22:18.441");
  });

  it("pads single-digit hours/minutes/seconds", () => {
    expect(formatTimeUtc("2026-04-23T01:05:09.007Z")).toBe("01:05:09.007");
  });

  it("handles midnight", () => {
    expect(formatTimeUtc("2026-04-23T00:00:00.000Z")).toBe("00:00:00.000");
  });

  it("handles end of day", () => {
    expect(formatTimeUtc("2026-04-23T23:59:59.999Z")).toBe("23:59:59.999");
  });

  it("converts non-UTC input to UTC", () => {
    // 2026-04-23T00:00:00.000+02:00 = 2026-04-22T22:00:00.000Z
    expect(formatTimeUtc("2026-04-23T00:00:00.000+02:00")).toBe("22:00:00.000");
  });
});

describe("isTodayUtc", () => {
  it("returns true for current UTC moment", () => {
    const now = new Date();
    const iso = now.toISOString();
    expect(isTodayUtc(iso)).toBe(true);
  });

  it("returns false for yesterday UTC", () => {
    const yesterday = new Date();
    yesterday.setUTCDate(yesterday.getUTCDate() - 1);
    expect(isTodayUtc(yesterday.toISOString())).toBe(false);
  });

  it("returns false for tomorrow UTC", () => {
    const tomorrow = new Date();
    tomorrow.setUTCDate(tomorrow.getUTCDate() + 1);
    expect(isTodayUtc(tomorrow.toISOString())).toBe(false);
  });

  it("uses UTC rather than the timestamp offset calendar day", () => {
    const now = new Date("2026-08-01T23:00:00.000Z");
    expect(isTodayUtc("2026-08-02T00:30:00.000+02:00", now)).toBe(true);
  });
});

describe("utcDateKey", () => {
  it("formats the UTC day independently of the timestamp offset", () => {
    expect(utcDateKey(new Date("2026-08-02T00:30:00.000+02:00"))).toBe(
      "2026-08-01",
    );
  });
});
