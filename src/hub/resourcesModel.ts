import type { SessionDisk, SessionResources, SessionState, SessionView } from "./api";

/** Days a running session can go without a turn or a visit before it counts as idle. */
export const IDLE_DAYS = 3;
const DAY_MS = 86_400_000;
/** States where stopping the harness could cut off work. */
const BUSY: SessionState[] = ["starting", "working", "needs_approval"];

export type ResourceMap = Record<string, SessionResources | null>;

export function formatKb(kb: number): string {
  if (kb >= 1024 * 1024) return `${(kb / (1024 * 1024)).toFixed(1)} GB`;
  if (kb >= 1024) return `${Math.round(kb / 1024)} MB`;
  return `${kb} KB`;
}

export function formatCpu(tenths: number): string {
  return `${Math.round(tenths / 10)}%`;
}

/** Whether the session's main terminal is up; its other tabs can outlive it. */
export function mainRunning(s: SessionView): boolean {
  return s.status.state !== "suspended" && s.status.state !== "exited";
}

export function isBusy(s: SessionView): boolean {
  return BUSY.includes(s.status.state);
}

/** Whether Stop is offered: the main terminal runs and nothing is in flight. */
export function canStop(s: SessionView): boolean {
  return mainRunning(s) && !isBusy(s);
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
  if (!canStop(s)) return null;
  if (s.lane === "parked") return "Parked, still running";
  const days = idleDays(s, now);
  return days >= IDLE_DAYS ? `Idle ${days}d` : null;
}

/** A session's measured disk, or null when it was not measured or a walk was given up. */
export function diskKb(d: SessionDisk | undefined): number | null {
  if (!d || d.gave_up || (d.dir_kb === null && d.temp_kb === null)) return null;
  return (d.dir_kb ?? 0) + (d.temp_kb ?? 0);
}

/**
 * Sessions using memory, largest first, then the others by disk, then by
 * name. A walk too big to finish ranks above every finished one.
 */
export function byResources(sessions: SessionView[], resources: ResourceMap, disk: Map<string, SessionDisk> | null): SessionView[] {
  const mem = (s: SessionView) => resources[s.key]?.rss_kb ?? -1;
  const used = (s: SessionView) => (disk?.get(s.key)?.gave_up ? Number.MAX_SAFE_INTEGER : diskKb(disk?.get(s.key)) ?? -1);
  return [...sessions].sort((a, b) => mem(b) - mem(a) || used(b) - used(a) || a.name.localeCompare(b.name));
}
