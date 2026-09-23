import { describe, expect, it } from "vitest";
import { byLane, effortsOf, headlineOf, railGroup, sinceLabel, type SessionView } from "./api";
import { blockedLabel } from "./SessionRail";
import { fuzzyScore } from "./CommandPalette";

function session(over: Partial<SessionView> = {}): SessionView {
  return {
    key: "/w/a",
    name: "A",
    color: "",
    provider: "claude",
    session_id: null,
    ticket_key: null,
    chrome: false,
    override_terminal_theme: false,
    tabs: [],
    status: {
      state: "working",
      since: new Date().toISOString(),
      detail: null,
      title: null,
      last_message: null,
      transcript_path: null,
      transcript_len: 0,
      harness_pid: null,
    },
    summary: null,
    last_viewed: null,
    attention: false,
    lane: "background",
    blocked_since: null,
    checked_at: null,
    ...over,
  };
}

describe("headlineOf", () => {
  it("prefers the model summary, then the harness title, then the last message", () => {
    const base = session();
    expect(headlineOf({ ...base, status: { ...base.status, last_message: "line one\nline two" } })).toBe("line one");
    expect(headlineOf({ ...base, status: { ...base.status, title: "Harness title", last_message: "x" } })).toBe("Harness title");
    expect(
      headlineOf({
        ...base,
        summary: { headline: "Summary", doing: "", needs_user: null, generated_at: "", transcript_len: 0, source: "model" },
      }),
    ).toBe("Summary");
  });

  it("ignores the shell's own title when no harness is running", () => {
    const base = session();
    expect(headlineOf({ ...base, status: { ...base.status, state: "shell", title: "me@host:~/w" } })).toBe("");
  });
});

describe("railGroup", () => {
  it("groups by what the user has to do", () => {
    expect(railGroup(session({ attention: true }))).toBe("needs");
    expect(railGroup(session())).toBe("working");
    const s = session();
    expect(railGroup({ ...s, status: { ...s.status, state: "your_turn" } })).toBe("quiet");
  });
});

describe("sinceLabel", () => {
  it("formats elapsed time compactly", () => {
    const now = Date.parse("2026-01-02T00:00:00Z");
    expect(sinceLabel("2026-01-01T23:59:30Z", now)).toBe("30s");
    expect(sinceLabel("2026-01-01T23:15:00Z", now)).toBe("45m");
    expect(sinceLabel("2026-01-01T02:00:00Z", now)).toBe("22h");
    expect(sinceLabel("2025-12-28T00:00:00Z", now)).toBe("5d");
    expect(sinceLabel("not a date", now)).toBe("");
  });
});

describe("fuzzyScore", () => {
  it("ranks substrings above scattered matches and rejects non-matches", () => {
    expect(fuzzyScore("inv", "ABC-398 Invoice export")).toBeGreaterThan(fuzzyScore("inv", "infra review"));
    expect(fuzzyScore("xyz", "Invoice export")).toBe(-1);
    expect(fuzzyScore("", "anything")).toBe(0);
  });
});

describe("lanes", () => {
  it("orders sessions priority, background, blocked, keeping the order within each", () => {
    const list = [
      session({ key: "b1", lane: "blocked" }),
      session({ key: "g1" }),
      session({ key: "p1", lane: "priority" }),
      session({ key: "g2" }),
      session({ key: "p2", lane: "priority" }),
    ];
    expect(byLane(list).map((s) => s.key)).toEqual(["p1", "p2", "g1", "g2", "b1"]);
  });

  it("shows how long a session has been blocked and when it was last checked", () => {
    const now = Date.parse("2026-09-23T12:00:00Z");
    const since = "2026-09-20T12:00:00Z";
    expect(blockedLabel(session({ lane: "blocked", blocked_since: since, checked_at: since }), now)).toBe("blocked 3d");
    expect(
      blockedLabel(session({ lane: "blocked", blocked_since: since, checked_at: "2026-09-23T10:00:00Z" }), now),
    ).toBe("blocked 3d · checked 2h ago");
    expect(blockedLabel(session({ lane: "priority", blocked_since: since }), now)).toBeNull();
  });
});

describe("effortsOf", () => {
  it("links sessions by epic, ticket and fork, and needs two to make a group", () => {
    const list = [
      session({ key: "a", name: "A", epic: "ABC-1 Payments" }),
      session({ key: "b", name: "B", epic: "ABC-1 Payments" }),
      session({ key: "c", name: "C", session_id: "c-id" }),
      session({ key: "d", name: "D", forked_from: "c-id" }),
      session({ key: "e", name: "E", ticket_key: "ABC-9" }),
      session({ key: "f", name: "F" }),
    ];
    const efforts = effortsOf(list);
    expect(efforts.get("a")).toBe("ABC-1 Payments");
    expect(efforts.get("b")).toBe("ABC-1 Payments");
    expect(efforts.get("c")).toBe("C");
    expect(efforts.get("d")).toBe("C");
    expect(efforts.has("e")).toBe(false);
    expect(efforts.has("f")).toBe(false);
  });

  it("puts an effort the user set before any link, and a fork follows its parent's", () => {
    const list = [
      session({ key: "a", epic: "ABC-1 Payments", effort: { name: "Checkout revamp", source: "user" } }),
      session({ key: "b", epic: "ABC-1 Payments" }),
      session({ key: "c", session_id: "c-id", effort: { name: "Checkout revamp", source: "auto" } }),
      session({ key: "d", forked_from: "c-id" }),
    ];
    const efforts = effortsOf(list);
    expect(efforts.get("a")).toBe("Checkout revamp");
    expect(efforts.has("b")).toBe(false);
    expect(efforts.get("d")).toBe("Checkout revamp");
  });
});
