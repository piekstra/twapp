/** Format a ticket key for the collapsed badge. Jira keys pass through as-is.
 *  GitHub keys like "org/repo#123" become "repo#123", truncated if still long. */
export function formatTicketBadge(key: string): string {
  if (key.includes("#")) {
    // GitHub: strip org prefix
    const hashIdx = key.indexOf("#");
    const beforeHash = key.substring(0, hashIdx);
    const number = key.substring(hashIdx);
    const parts = beforeHash.split("/");
    const repo = parts[parts.length - 1];
    if (repo.length > 20) {
      return repo.substring(0, 18) + ".." + number;
    }
    return repo + number;
  }
  return key;
}

export function formatRelativeTime(isoString: string | null): string {
  if (!isoString) return "never";
  const date = new Date(isoString);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMinutes = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMinutes < 1) return "just now";
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  if (diffDays < 30) return `${Math.floor(diffDays / 7)}w ago`;
  return date.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function formatTime(ts: number): string {
  const now = new Date();
  const date = new Date(ts);
  const diffMs = now.getTime() - date.getTime();
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

  if (diffDays < 1 && now.getDate() === date.getDate()) {
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  if (diffDays < 7) {
    const days = diffDays || 1; // crossed midnight but < 24h
    return `${days}d ago`;
  }
  if (now.getFullYear() === date.getFullYear()) {
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  }
  return date.toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" });
}

export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function shortenPath(path: string, homeDir: string): string {
  if (homeDir && path.startsWith(homeDir)) {
    return "~" + path.slice(homeDir.length);
  }
  return path;
}
