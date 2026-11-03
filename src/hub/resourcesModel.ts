import type { DiskUsage, SessionState, SessionView } from "./api";

/** Days a running session can go without a turn or a visit before it counts as idle. */
export const IDLE_DAYS = 3;
const DAY_MS = 86_400_000;
/** States where stopping the harness could cut off work. */
const BUSY: SessionState[] = ["starting", "working", "needs_approval"];

export function formatKb(kb: number): string {
  if (kb >= 1024 * 1024) return `${(kb / (1024 * 1024)).toFixed(1)} GB`;
  if (kb >= 1024) return `${Math.round(kb / 1024)} MB`;
  return `${kb} KB`;
}

export function formatCpu(tenths: number): string {
  return `${Math.round(tenths / 10)}%`;
}

export function isRunning(s: SessionView): boolean {
  return !!s.usage && s.status.state !== "suspended";
}

export function isBusy(s: SessionView): boolean {
  return BUSY.includes(s.status.state);
}

/** Whole days since the session's last state change or the user's last visit, whichever is later. */
export function idleDays(s: SessionView, now: number): number {
  const times = [s.status.since, s.last_viewed]
    .map((t) => (t ? Date.parse(t) : NaN))
    .filter((t) => !Number.isNaN(t));
  return times.length ? Math.max(0, Math.floor((now - Math.max(...times)) / DAY_MS)) : 0;
}

/** Why a running session looks like it holds memory for nothing, or null. */
export function staleReason(s: SessionView, now: number): string | null {
  if (!isRunning(s) || isBusy(s)) return null;
  if (s.lane === "parked") return "Parked, still running";
  const days = idleDays(s, now);
  return days >= IDLE_DAYS ? `Idle ${days}d` : null;
}

export function diskKb(d: DiskUsage | undefined): number | null {
  if (!d || (d.dir_kb === null && d.temp_kb === null)) return null;
  return (d.dir_kb ?? 0) + (d.temp_kb ?? 0);
}

/** Running sessions by memory, largest first, then the others by disk, then by name. */
export function byResources(sessions: SessionView[], disk: Map<string, DiskUsage> | null): SessionView[] {
  const mem = (s: SessionView) => (isRunning(s) ? s.usage!.rss_kb : -1);
  const used = (s: SessionView) => diskKb(disk?.get(s.key)) ?? -1;
  return [...sessions].sort((a, b) => mem(b) - mem(a) || used(b) - used(a) || a.name.localeCompare(b.name));
}
