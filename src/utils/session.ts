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

export type SessionFieldValues = {
  name: string;
  session_id: string;
  claude_cwd: string;
  ticket_key: string;
};

/**
 * Build the argument object for `invoke("update_session_fields", …)`,
 * including only the fields that changed from `original`.
 *
 * Keys MUST be camelCase. Tauri v2 maps camelCase JS invoke keys onto the
 * snake_case Rust command parameters (`sessionId` -> `session_id`, etc.).
 * Sending snake_case keys silently drops them: the command receives `None`,
 * `write_session` rewrites the file with the OLD value, and the optimistic
 * UI update makes it look like the change took even though disk is untouched.
 */
export function buildSessionFieldsArgs(
  directory: string,
  fields: SessionFieldValues,
  original: SessionFieldValues | null,
): Record<string, string> {
  const args: Record<string, string> = { directory };
  if (fields.name !== original?.name) args.name = fields.name;
  if (fields.session_id !== original?.session_id) args.sessionId = fields.session_id;
  if (fields.claude_cwd !== original?.claude_cwd) args.claudeCwd = fields.claude_cwd;
  if (fields.ticket_key !== original?.ticket_key) args.ticketKey = fields.ticket_key;
  return args;
}
