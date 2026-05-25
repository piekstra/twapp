import { describe, expect, it } from "vitest";
import {
  buildResumeCommand,
  buildSessionFieldsArgs,
  maskProviderSessionId,
  shellEscapeSingleQuoted,
  type SessionFieldValues,
} from "./session";

describe("shellEscapeSingleQuoted", () => {
  it("escapes single quotes for shell-safe single-quoted strings", () => {
    expect(shellEscapeSingleQuoted("a'b")).toBe("a'\\''b");
  });
});

describe("buildResumeCommand", () => {
  it("builds a claude resume command with quoted session id", () => {
    expect(buildResumeCommand("claude", "abc'123", "/tmp/demo")).toBe(
      "claude --resume 'abc'\\''123'",
    );
  });

  it("builds a codex resume command with quoted session id and cwd", () => {
    expect(buildResumeCommand("codex", "abc'123", "/tmp/it's-demo")).toBe(
      "codex resume 'abc'\\''123' -C '/tmp/it'\\''s-demo'",
    );
  });

  it("builds a codex fresh command without a session id", () => {
    expect(buildResumeCommand("codex", null, "/tmp/demo")).toBe(
      "codex -C '/tmp/demo'",
    );
  });
});

describe("maskProviderSessionId", () => {
  it("masks long provider session ids", () => {
    expect(maskProviderSessionId("1234567890abcdef")).toBe("1234...cdef");
  });

  it("leaves short ids unchanged", () => {
    expect(maskProviderSessionId("123456789012")).toBe("123456789012");
  });
});

describe("buildSessionFieldsArgs", () => {
  const fields = (o: Partial<SessionFieldValues> = {}): SessionFieldValues => ({
    name: "demo",
    session_id: "6b8e7694-7969-475b-ae9f-5abd07fbd16a",
    claude_cwd: "/tmp/demo",
    ticket_key: "",
    ...o,
  });

  it("emits camelCase keys for a changed session id (regression: snake_case keys never reach disk)", () => {
    const original = fields();
    const next = fields({ session_id: "25c8ffd1-131a-47ac-9eef-eab69429f96e" });
    const args = buildSessionFieldsArgs("/tmp/demo", next, original);

    expect(args.sessionId).toBe("25c8ffd1-131a-47ac-9eef-eab69429f96e");
    // The Tauri command param is `session_id`; a snake_case key here would be
    // dropped and the change would silently fail to persist.
    expect(args).not.toHaveProperty("session_id");
  });

  it("includes only changed fields plus the directory", () => {
    const original = fields();
    const next = fields({ session_id: "new-id" });
    const args = buildSessionFieldsArgs("/tmp/demo", next, original);

    expect(args).toEqual({ directory: "/tmp/demo", sessionId: "new-id" });
  });

  it("maps every editable field to its camelCase invoke key", () => {
    const original = fields();
    const next = fields({
      name: "renamed",
      session_id: "new-id",
      claude_cwd: "/tmp/other",
      ticket_key: "JTK-1",
    });
    const args = buildSessionFieldsArgs("/tmp/demo", next, original);

    expect(args).toEqual({
      directory: "/tmp/demo",
      name: "renamed",
      sessionId: "new-id",
      claudeCwd: "/tmp/other",
      ticketKey: "JTK-1",
    });
  });

  it("treats a null original as all-changed", () => {
    const next = fields({ ticket_key: "JTK-9" });
    const args = buildSessionFieldsArgs("/tmp/demo", next, null);

    expect(args).toEqual({
      directory: "/tmp/demo",
      name: "demo",
      sessionId: "6b8e7694-7969-475b-ae9f-5abd07fbd16a",
      claudeCwd: "/tmp/demo",
      ticketKey: "JTK-9",
    });
  });
});
