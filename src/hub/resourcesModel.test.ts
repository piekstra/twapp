import { describe, expect, it } from "vitest";
import type { DiskUsage, SessionView } from "./api";
import { byResources, formatCpu, formatKb, idleDays, staleReason } from "./resourcesModel";

const NOW = Date.parse("2026-11-03T12:00:00Z");

function session(name: string, over: Partial<SessionView> & { state?: SessionView["status"]["state"]; since?: string; rss?: number } = {}): SessionView {
  const { state = "your_turn", since = "2026-11-03T11:00:00Z", rss, ...rest } = over;
  return {
    key: `/work/${name}`,
    name,
    color: "",
    provider: "claude",
    session_id: null,
    ticket_key: null,
    chrome: false,
    override_terminal_theme: false,
    tabs: [],
    status: { state, since, detail: null, title: null, last_message: null, transcript_path: null, transcript_len: 0, harness_pid: null },
    usage: rss === undefined ? null : { rss_kb: rss, cpu_tenths: 0, processes: 1 },
    summary: null,
    last_viewed: null,
    attention: false,
    lane: "priority",
    blocked_since: null,
    checked_at: null,
    ...rest,
  };
}

describe("formatting", () => {
  it("shows memory in the unit a person reads", () => {
    expect(formatKb(512)).toBe("512 KB");
    expect(formatKb(600 * 1024)).toBe("600 MB");
    expect(formatKb(1.5 * 1024 * 1024)).toBe("1.5 GB");
    expect(formatCpu(123)).toBe("12%");
  });
});

describe("staleness", () => {
  it("counts days from the later of the last state change and the last visit", () => {
    const s = session("a", { since: "2026-10-29T12:00:00Z", last_viewed: "2026-11-01T12:00:00Z" });
    expect(idleDays(s, NOW)).toBe(2);
  });

  it("flags parked and long-idle sessions that still run", () => {
    expect(staleReason(session("parked", { lane: "parked", rss: 500_000 }), NOW)).toBe("Parked, still running");
    expect(staleReason(session("old", { since: "2026-10-30T11:00:00Z", rss: 500_000 }), NOW)).toBe("Idle 4d");
    expect(staleReason(session("recent", { rss: 500_000 }), NOW)).toBeNull();
  });

  it("never flags a session that is not running or is at work", () => {
    expect(staleReason(session("stopped", { lane: "parked", state: "suspended" }), NOW)).toBeNull();
    expect(staleReason(session("no usage yet", { lane: "parked" }), NOW)).toBeNull();
    for (const state of ["working", "starting", "needs_approval"] as const) {
      expect(staleReason(session(state, { lane: "parked", state, since: "2026-10-01T00:00:00Z", rss: 1 }), NOW)).toBeNull();
    }
  });
});

describe("order", () => {
  it("puts running sessions first by memory, then the rest by disk", () => {
    const disk = new Map<string, DiskUsage>([
      ["/work/big-disk", { key: "/work/big-disk", dir_kb: 9_000_000, temp_kb: null }],
      ["/work/small-disk", { key: "/work/small-disk", dir_kb: 10, temp_kb: 5 }],
    ]);
    const order = byResources(
      [
        session("small-disk", { state: "suspended" }),
        session("light", { rss: 100 }),
        session("big-disk", { state: "suspended" }),
        session("heavy", { rss: 900 }),
      ],
      disk,
    ).map((s) => s.name);
    expect(order).toEqual(["heavy", "light", "big-disk", "small-disk"]);
  });
});
