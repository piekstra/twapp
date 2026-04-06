import { describe, expect, it } from "vitest";
import { buildResumeCommand, maskProviderSessionId, shellEscapeSingleQuoted } from "./session";

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
