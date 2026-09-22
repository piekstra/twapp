import { describe, expect, it } from "vitest";
import { headlineOf, railGroup, sinceLabel, type SessionView } from "./api";
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
