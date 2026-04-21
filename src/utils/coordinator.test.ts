import { describe, it, expect } from "vitest";
import { buildClaimArgs, buildLaunchArgs, cleanFormArg } from "./coordinator";

describe("cleanFormArg", () => {
  it("returns null for an empty string", () => {
    expect(cleanFormArg("")).toBeNull();
  });

  it("returns null for whitespace-only input", () => {
    expect(cleanFormArg("   \t\n")).toBeNull();
  });

  it("trims and keeps non-empty input", () => {
    expect(cleanFormArg("  claude-opus-4-7  ")).toBe("claude-opus-4-7");
  });
});

describe("buildLaunchArgs", () => {
  it("passes every filled field through trimmed", () => {
    expect(
      buildLaunchArgs({
        name: "my-coord",
        briefing: "/abs/path/brief.md",
        sharedDir: "/abs/path/mailbox",
        model: "claude-opus-4-7",
      }),
    ).toEqual({
      name: "my-coord",
      briefing: "/abs/path/brief.md",
      sharedDir: "/abs/path/mailbox",
      colabGroup: null,
      model: "claude-opus-4-7",
    });
  });

  it("converts empty fields to null so the CLI defaults apply", () => {
    expect(
      buildLaunchArgs({ name: "", briefing: "", sharedDir: "", model: "" }),
    ).toEqual({
      name: null,
      briefing: null,
      sharedDir: null,
      colabGroup: null,
      model: null,
    });
  });

  it("mixes null and real values per-field", () => {
    expect(
      buildLaunchArgs({
        name: "my-coord",
        briefing: "",
        sharedDir: "",
        model: "claude-sonnet-4-6",
      }),
    ).toEqual({
      name: "my-coord",
      briefing: null,
      sharedDir: null,
      colabGroup: null,
      model: "claude-sonnet-4-6",
    });
  });
});

describe("buildClaimArgs", () => {
  it("passes through a picked session name", () => {
    expect(buildClaimArgs({ name: "worker-a", force: false })).toEqual({
      name: "worker-a",
      force: false,
      colabGroup: null,
    });
  });

  it("trims the picked name", () => {
    expect(buildClaimArgs({ name: "  worker-a  ", force: true })).toEqual({
      name: "worker-a",
      force: true,
      colabGroup: null,
    });
  });

  it("throws when no session is picked", () => {
    expect(() => buildClaimArgs({ name: "", force: false })).toThrow(
      /Pick a session/,
    );
    expect(() => buildClaimArgs({ name: "   ", force: false })).toThrow(
      /Pick a session/,
    );
  });
});
