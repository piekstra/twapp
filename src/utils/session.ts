export type SessionProvider = "claude" | "codex";

export function shellEscapeSingleQuoted(value: string): string {
  return value.replace(/'/g, "'\\''");
}

export function buildResumeCommand(
  provider: SessionProvider,
  sessionId: string | null | undefined,
  cwd: string | null | undefined,
): string {
  const safeCwd = shellEscapeSingleQuoted(cwd || ".");

  if (provider === "codex") {
    if (sessionId) {
      const safeSessionId = shellEscapeSingleQuoted(sessionId);
      return `codex resume '${safeSessionId}' -C '${safeCwd}'`;
    }
    return `codex -C '${safeCwd}'`;
  }

  if (sessionId) {
    const safeSessionId = shellEscapeSingleQuoted(sessionId);
    return `claude --resume '${safeSessionId}'`;
  }

  return "claude -c";
}

export function maskProviderSessionId(sessionId: string): string {
  if (sessionId.length <= 12) {
    return sessionId;
  }
  return `${sessionId.slice(0, 4)}...${sessionId.slice(-4)}`;
}
