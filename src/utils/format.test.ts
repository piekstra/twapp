import { describe, it, expect, vi, afterEach } from "vitest";
import { formatTicketBadge, formatRelativeTime, formatTime, formatBytes, shortenPath } from "./format";

describe("formatTicketBadge", () => {
  it("passes Jira keys through as-is", () => {
    expect(formatTicketBadge("MON-1234")).toBe("MON-1234");
  });

  it("strips org prefix from GitHub keys", () => {
    expect(formatTicketBadge("org/repo#123")).toBe("repo#123");
  });

  it("handles GitHub keys without org", () => {
    expect(formatTicketBadge("repo#42")).toBe("repo#42");
  });

  it("truncates long repo names", () => {
    const longRepo = "a-very-long-repository-name";
    const result = formatTicketBadge(`org/${longRepo}#99`);
    expect(result).toBe("a-very-long-reposi..#99");
  });

  it("does not truncate repo names at 20 chars or under", () => {
    const repo = "exactly-twenty-chars";
    expect(repo.length).toBe(20);
    expect(formatTicketBadge(`org/${repo}#1`)).toBe(`${repo}#1`);
  });

  it("handles plain strings without hash", () => {
    expect(formatTicketBadge("PROJ-99")).toBe("PROJ-99");
  });
});

describe("formatRelativeTime", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns 'never' for null", () => {
    expect(formatRelativeTime(null)).toBe("never");
  });

  it("returns 'just now' for less than a minute ago", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T12:00:30Z"));
    expect(formatRelativeTime("2025-06-15T12:00:00Z")).toBe("just now");
  });

  it("returns minutes ago", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T12:05:00Z"));
    expect(formatRelativeTime("2025-06-15T12:00:00Z")).toBe("5m ago");
  });

  it("returns hours ago", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T15:00:00Z"));
    expect(formatRelativeTime("2025-06-15T12:00:00Z")).toBe("3h ago");
  });

  it("returns days ago", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-18T12:00:00Z"));
    expect(formatRelativeTime("2025-06-15T12:00:00Z")).toBe("3d ago");
  });

  it("returns weeks ago", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-07-06T12:00:00Z"));
    expect(formatRelativeTime("2025-06-15T12:00:00Z")).toBe("3w ago");
  });

  it("returns formatted date for 30+ days", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-08-15T12:00:00Z"));
    const result = formatRelativeTime("2025-06-15T12:00:00Z");
    // Should be a date string like "Jun 15"
    expect(result).toMatch(/Jun/);
    expect(result).toMatch(/15/);
  });
});

describe("formatTime", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns time string for today", () => {
    vi.useFakeTimers();
    const now = new Date("2025-06-15T14:30:00");
    vi.setSystemTime(now);
    const ts = new Date("2025-06-15T10:15:00").getTime();
    const result = formatTime(ts);
    // Should be a time like "10:15 AM"
    expect(result).toMatch(/10/);
    expect(result).toMatch(/15/);
  });

  it("returns days ago for less than a week", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T12:00:00"));
    const ts = new Date("2025-06-12T12:00:00").getTime();
    expect(formatTime(ts)).toBe("3d ago");
  });

  it("returns month/day for same year", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T12:00:00"));
    const ts = new Date("2025-03-10T12:00:00").getTime();
    const result = formatTime(ts);
    expect(result).toMatch(/Mar/);
    expect(result).toMatch(/10/);
  });

  it("returns month/day/year for different year", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T12:00:00"));
    const ts = new Date("2024-03-10T12:00:00").getTime();
    const result = formatTime(ts);
    expect(result).toMatch(/Mar/);
    expect(result).toMatch(/10/);
    expect(result).toMatch(/2024/);
  });

  it("returns '1d ago' when crossing midnight but less than 24h", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2025-06-15T01:00:00"));
    const ts = new Date("2025-06-14T23:00:00").getTime();
    expect(formatTime(ts)).toBe("1d ago");
  });
});

describe("formatBytes", () => {
  it("formats zero bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
  });

  it("formats bytes under 1 KB", () => {
    expect(formatBytes(500)).toBe("500 B");
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(10240)).toBe("10.0 KB");
  });

  it("formats megabytes", () => {
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(1.5 * 1024 * 1024)).toBe("1.5 MB");
    expect(formatBytes(100 * 1024 * 1024)).toBe("100.0 MB");
  });
});

describe("shortenPath", () => {
  it("replaces home directory with ~", () => {
    expect(shortenPath("/Users/me/projects/foo", "/Users/me")).toBe("~/projects/foo");
  });

  it("returns path unchanged if not under home", () => {
    expect(shortenPath("/var/log/something", "/Users/me")).toBe("/var/log/something");
  });

  it("returns path unchanged if homeDir is empty", () => {
    expect(shortenPath("/Users/me/projects", "")).toBe("/Users/me/projects");
  });

  it("handles home dir that is the full path", () => {
    expect(shortenPath("/Users/me", "/Users/me")).toBe("~");
  });
});
