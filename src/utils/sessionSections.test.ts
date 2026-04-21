import { describe, it, expect } from "vitest";
import { partitionSessions, colabGroupHue, colabGroupBorderColor } from "./sessionSections";
import type { LauncherSession } from "../types";

function mkSession(partial: Partial<LauncherSession> & { name: string }): LauncherSession {
  return {
    session_id: partial.session_id ?? `sid-${partial.name}`,
    provider: "claude",
    provider_session_id: null,
    needs_migration: false,
    name: partial.name,
    color: "#ccc",
    ticket_key: null,
    directory: `/tmp/${partial.name}`,
    claude_cwd: `/tmp/${partial.name}`,
    last_active: partial.last_active ?? "2026-04-21T00:00:00Z",
    created: "2026-04-20T00:00:00Z",
    is_running: false,
    message_count: null,
    imported: false,
    forked_from: null,
    role: partial.role ?? null,
    provenance: partial.provenance ?? null,
    colab_group: partial.colab_group ?? null,
  };
}

describe("partitionSessions", () => {
  it("returns only 'My sessions' when no colab sessions exist", () => {
    const sessions = [
      mkSession({ name: "alpha" }),
      mkSession({ name: "beta", provenance: "user" }),
    ];
    const sections = partitionSessions(sessions, "recent");
    expect(sections).toHaveLength(1);
    expect(sections[0].kind).toBe("mine");
    expect(sections[0].sessions.map((s) => s.name).sort()).toEqual(["alpha", "beta"]);
  });

  it("groups colab sessions by colab_group with coordinator first", () => {
    const sessions = [
      mkSession({ name: "human-1" }),
      mkSession({ name: "worker-a", provenance: "spawned", colab_group: "feature-x" }),
      mkSession({
        name: "coord-x",
        provenance: "spawned",
        role: "coordinator",
        colab_group: "feature-x",
      }),
      mkSession({ name: "worker-b", provenance: "spawned", colab_group: "feature-x" }),
    ];
    const sections = partitionSessions(sessions, "recent");
    expect(sections.map((s) => s.kind)).toEqual(["mine", "colab"]);
    const colab = sections[1];
    expect(colab.label).toBe("Co-lab: feature-x");
    expect(colab.sessions[0].name).toBe("coord-x");
    expect(colab.sessions.slice(1).map((s) => s.name).sort()).toEqual(["worker-a", "worker-b"]);
  });

  it("separates orphan co-lab sessions (spawned but colab_group=None) into their own section", () => {
    const sessions = [
      mkSession({ name: "human-1" }),
      mkSession({ name: "orphan-1", provenance: "spawned" }),
      mkSession({ name: "orphan-2", provenance: "spawned", colab_group: "" }),
    ];
    const sections = partitionSessions(sessions, "recent");
    expect(sections.map((s) => s.kind)).toEqual(["mine", "orphans"]);
    expect(sections[0].sessions.map((s) => s.name)).toEqual(["human-1"]);
    expect(sections[1].sessions.map((s) => s.name).sort()).toEqual(["orphan-1", "orphan-2"]);
  });

  it("orders colab sections alphabetically by group name", () => {
    const sessions = [
      mkSession({ name: "a", provenance: "spawned", colab_group: "zebra" }),
      mkSession({ name: "b", provenance: "spawned", colab_group: "alpha" }),
      mkSession({ name: "c", provenance: "spawned", colab_group: "mango" }),
    ];
    const sections = partitionSessions(sessions, "recent");
    expect(sections.filter((s) => s.kind === "colab").map((s) => s.groupName)).toEqual([
      "alpha",
      "mango",
      "zebra",
    ]);
  });

  it("applies sort mode within sections", () => {
    const sessions = [
      mkSession({ name: "zeta", last_active: "2026-04-21T01:00:00Z" }),
      mkSession({ name: "alpha", last_active: "2026-04-21T03:00:00Z" }),
      mkSession({ name: "mango", last_active: "2026-04-21T02:00:00Z" }),
    ];
    const recent = partitionSessions(sessions, "recent");
    expect(recent[0].sessions.map((s) => s.name)).toEqual(["alpha", "mango", "zeta"]);
    const alpha = partitionSessions(sessions, "alpha");
    expect(alpha[0].sessions.map((s) => s.name)).toEqual(["alpha", "mango", "zeta"]);
  });

  it("treats colab_group='' and null as equivalent 'not-in-a-group'", () => {
    const sessions = [
      mkSession({ name: "a", colab_group: "" }),
      mkSession({ name: "b", colab_group: null }),
    ];
    const sections = partitionSessions(sessions, "recent");
    expect(sections).toHaveLength(1);
    expect(sections[0].kind).toBe("mine");
  });
});

describe("colabGroupHue / colabGroupBorderColor", () => {
  it("is deterministic for a given group name", () => {
    expect(colabGroupHue("feature-x")).toBe(colabGroupHue("feature-x"));
    expect(colabGroupBorderColor("feature-x")).toBe(colabGroupBorderColor("feature-x"));
  });

  it("spreads different group names across the hue range", () => {
    const hues = new Set(["alpha", "bravo", "charlie", "delta", "echo"].map(colabGroupHue));
    expect(hues.size).toBeGreaterThan(1);
  });

  it("returns a CSS hsl() string", () => {
    expect(colabGroupBorderColor("feature-x")).toMatch(/^hsl\(\d+, 55%, 55%\)$/);
  });
});
