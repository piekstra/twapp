import { describe, it, expect } from "vitest";
import {
  formatAge,
  summarize,
  statusDotClass,
  truncateTask,
  type FleetAgent,
} from "./FleetPane";

const agent = (o: Partial<FleetAgent> & { handle: string }): FleetAgent => ({
  status: "processing",
  last_heartbeat: "2026-04-21T12:00:00Z",
  last_heartbeat_age_sec: 5,
  poll_interval_sec: 90,
  dormant: false,
  unread_count: 0,
  urgent_count: 0,
  ...o,
});

describe("formatAge", () => {
  it("formats seconds / minutes / hours / days", () => {
    expect(formatAge(5)).toBe("5s ago");
    expect(formatAge(65)).toBe("1m ago");
    expect(formatAge(3600)).toBe("1h ago");
    expect(formatAge(86400 * 2)).toBe("2d ago");
  });

  it("clamps negative and null", () => {
    expect(formatAge(-10)).toBe("0s ago");
    expect(formatAge(null)).toBe("—");
    expect(formatAge(undefined)).toBe("—");
    expect(formatAge(Number.NaN)).toBe("—");
  });
});

describe("summarize", () => {
  it("counts total / active / dormant / urgent", () => {
    const s = summarize([
      agent({ handle: "a" }),
      agent({ handle: "b", dormant: true }),
      agent({ handle: "c", urgent_count: 2 }),
      agent({ handle: "d", dormant: true, urgent_count: 1 }),
    ]);
    expect(s).toEqual({ total: 4, active: 2, dormant: 2, urgent: 2 });
  });

  it("returns zeroes for empty fleet", () => {
    expect(summarize([])).toEqual({ total: 0, active: 0, dormant: 0, urgent: 0 });
  });
});

describe("statusDotClass", () => {
  it("routes by derived status", () => {
    expect(statusDotClass(agent({ handle: "a", status: "processing" }))).toContain("fleet-dot-processing");
    expect(statusDotClass(agent({ handle: "a", status: "idle" }))).toContain("fleet-dot-idle");
    expect(statusDotClass(agent({ handle: "a", status: "dormant" }))).toContain("fleet-dot-dormant");
  });

  it("dormant overrides a stale processing status", () => {
    // The Rust builder already does this flip, but guard the UI against a
    // hand-crafted response that left status=processing with dormant=true.
    expect(
      statusDotClass(agent({ handle: "a", status: "processing", dormant: true })),
    ).toContain("fleet-dot-dormant");
  });
});

describe("truncateTask", () => {
  it("returns trimmed string when under the cap", () => {
    expect(truncateTask("  rebasing onto main  ")).toBe("rebasing onto main");
  });

  it("appends ellipsis when over the cap", () => {
    const long = "x".repeat(100);
    const out = truncateTask(long, 20);
    expect(out).toHaveLength(20);
    expect(out.endsWith("…")).toBe(true);
  });

  it("handles null / empty", () => {
    expect(truncateTask(null)).toBe("");
    expect(truncateTask(undefined)).toBe("");
    expect(truncateTask("   ")).toBe("");
  });
});
