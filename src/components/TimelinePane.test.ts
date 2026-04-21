import { describe, it, expect } from "vitest";
import {
  chipLabel,
  filterByHandle,
  mergeEvents,
  sortNewestFirst,
  type TimelineEvent,
} from "./TimelinePane";

const evt = (o: Partial<TimelineEvent> & { ts: string; handle: string }): TimelineEvent => ({
  kind: "spawn",
  description: "",
  ...o,
});

describe("chipLabel", () => {
  it("returns known kind labels", () => {
    expect(chipLabel("spawn")).toBe("spawn");
    expect(chipLabel("claim")).toBe("claim");
    expect(chipLabel("release")).toBe("release");
    expect(chipLabel("reclaim")).toBe("reclaim");
    expect(chipLabel("offboard")).toBe("offboard");
    expect(chipLabel("dead")).toBe("dead");
  });

  it("passes unknown kinds through", () => {
    expect(chipLabel("future-kind")).toBe("future-kind");
  });
});

describe("sortNewestFirst", () => {
  it("sorts by ts descending with handle tiebreak", () => {
    const sorted = sortNewestFirst([
      evt({ ts: "2026-04-20T10:00:00Z", handle: "b" }),
      evt({ ts: "2026-04-21T10:00:00Z", handle: "a" }),
      evt({ ts: "2026-04-20T10:00:00Z", handle: "a" }),
    ]);
    expect(sorted.map((e) => `${e.ts} ${e.handle}`)).toEqual([
      "2026-04-21T10:00:00Z a",
      "2026-04-20T10:00:00Z a",
      "2026-04-20T10:00:00Z b",
    ]);
  });

  it("does not mutate the input", () => {
    const input = [
      evt({ ts: "2026-04-20T10:00:00Z", handle: "b" }),
      evt({ ts: "2026-04-21T10:00:00Z", handle: "a" }),
    ];
    const before = [...input];
    sortNewestFirst(input);
    expect(input).toEqual(before);
  });
});

describe("filterByHandle", () => {
  const events = [
    evt({ ts: "2026-04-21T10:00:00Z", handle: "Impl-Parser" }),
    evt({ ts: "2026-04-21T10:00:00Z", handle: "qa-regression" }),
    evt({ ts: "2026-04-21T10:00:00Z", handle: "impl-renderer" }),
  ];

  it("returns all events when filter is empty", () => {
    expect(filterByHandle(events, "").length).toBe(3);
    expect(filterByHandle(events, "   ").length).toBe(3);
  });

  it("filters case-insensitive substring on handle", () => {
    const out = filterByHandle(events, "IMPL");
    expect(out.map((e) => e.handle)).toEqual(["Impl-Parser", "impl-renderer"]);
  });
});

describe("mergeEvents", () => {
  it("dedups by (ts, handle, kind, description) and sorts newest first", () => {
    const page1 = [
      evt({ ts: "2026-04-21T10:00:00Z", handle: "a", kind: "spawn" }),
      evt({ ts: "2026-04-21T09:00:00Z", handle: "b", kind: "spawn" }),
    ];
    const page2 = [
      evt({ ts: "2026-04-21T10:00:00Z", handle: "a", kind: "spawn" }),
      evt({ ts: "2026-04-21T08:00:00Z", handle: "c", kind: "claim", description: "PR-1" }),
    ];
    const merged = mergeEvents(page1, page2);
    expect(merged.map((e) => `${e.ts} ${e.handle} ${e.kind}`)).toEqual([
      "2026-04-21T10:00:00Z a spawn",
      "2026-04-21T09:00:00Z b spawn",
      "2026-04-21T08:00:00Z c claim",
    ]);
  });

  it("keeps two events with the same handle but different descriptions", () => {
    // Re-claim after stale — different kind, same handle.
    const merged = mergeEvents(
      [evt({ ts: "2026-04-21T10:00:00Z", handle: "a", kind: "claim", description: "claimed PR-1" })],
      [evt({ ts: "2026-04-21T10:00:00Z", handle: "a", kind: "reclaim", description: "reclaimed PR-1 from stale owner b" })],
    );
    expect(merged).toHaveLength(2);
  });
});
