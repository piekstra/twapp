import { describe, it, expect } from "vitest";
import {
  mergeByPriority,
  priorityRank,
  parseTs,
  relativeTime,
  rowPreview,
  type UrgentMessage,
} from "./UrgentInbox";

const msg = (o: Partial<UrgentMessage> & { id: string; priority: string; ts: string }): UrgentMessage => ({
  from: "coord",
  to: ["twapp-ui-urgent"],
  body: "",
  path: "/m/x.md",
  ...o,
});

describe("priorityRank", () => {
  it("sorts blocker < urgent < routine (lower = more important)", () => {
    expect(priorityRank("blocker")).toBeLessThan(priorityRank("urgent"));
    expect(priorityRank("urgent")).toBeLessThan(priorityRank("routine"));
  });

  it("treats unknown priorities as routine-level", () => {
    expect(priorityRank("weird")).toBe(priorityRank("routine"));
  });
});

describe("mergeByPriority", () => {
  it("renders 3 messages in blocker-first then newest-first order", () => {
    const urgent = [
      msg({ id: "A", priority: "urgent", ts: "20260421T090000Z" }),
      msg({ id: "B", priority: "urgent", ts: "20260421T091000Z" }),
    ];
    const blocker = [msg({ id: "C", priority: "blocker", ts: "20260421T085959Z" })];
    const out = mergeByPriority([urgent, blocker]);
    expect(out.map((m) => m.id)).toEqual(["C", "B", "A"]);
  });

  it("dedupes by id, preferring the stronger priority", () => {
    // Real-world: the `inbox/urgent/` lane contains symlinks that surface both
    // priority=urgent AND priority=blocker messages. Either query can return
    // the same id with a different `priority` string; keep the strongest one.
    const urgent = [msg({ id: "X", priority: "urgent", ts: "20260421T090000Z" })];
    const blocker = [msg({ id: "X", priority: "blocker", ts: "20260421T090000Z" })];
    const out = mergeByPriority([urgent, blocker]);
    expect(out).toHaveLength(1);
    expect(out[0].priority).toBe("blocker");
  });

  it("returns empty array when both inputs are empty", () => {
    expect(mergeByPriority([[], []])).toEqual([]);
  });

  it("handles a single empty list alongside populated", () => {
    const blocker = [msg({ id: "Z", priority: "blocker", ts: "20260421T100000Z" })];
    expect(mergeByPriority([[], blocker]).map((m) => m.id)).toEqual(["Z"]);
  });
});

describe("parseTs", () => {
  it("parses the CLI timestamp format", () => {
    const n = parseTs("20260421T090400Z");
    expect(n).toBe(Date.parse("2026-04-21T09:04:00Z"));
  });

  it("returns null on malformed input", () => {
    expect(parseTs("not-a-timestamp")).toBeNull();
    expect(parseTs("")).toBeNull();
  });
});

describe("relativeTime", () => {
  const now = Date.parse("2026-04-21T10:00:00Z");
  it("formats seconds / minutes / hours / days", () => {
    expect(relativeTime("20260421T095959Z", now)).toBe("1s");
    expect(relativeTime("20260421T094500Z", now)).toBe("15m");
    expect(relativeTime("20260421T060000Z", now)).toBe("4h");
    expect(relativeTime("20260418T100000Z", now)).toBe("3d");
  });

  it("clamps negative deltas to 0s so future-dated messages don't render nonsense", () => {
    expect(relativeTime("20260421T100100Z", now)).toBe("0s");
  });

  it("passes through unparseable timestamps verbatim", () => {
    expect(relativeTime("weird", now)).toBe("weird");
  });
});

describe("rowPreview", () => {
  it("prefers subject when set", () => {
    expect(rowPreview(msg({ id: "1", priority: "urgent", ts: "T", subject: "PR #55 DIRTY" }))).toBe(
      "PR #55 DIRTY",
    );
  });

  it("falls back to the first non-blank body line", () => {
    expect(rowPreview(msg({ id: "1", priority: "urgent", ts: "T", body: "\n\nreal line\nnext" }))).toBe(
      "real line",
    );
  });

  it("truncates long previews", () => {
    const long = "x".repeat(200);
    const out = rowPreview(msg({ id: "1", priority: "urgent", ts: "T", body: long }));
    expect(out.length).toBeLessThanOrEqual(120);
  });

  it("returns a placeholder when both subject and body are empty", () => {
    expect(rowPreview(msg({ id: "1", priority: "urgent", ts: "T" }))).toBe("(no subject)");
  });
});
