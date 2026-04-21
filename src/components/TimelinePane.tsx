import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type TimelineEventKind =
  | "spawn"
  | "claim"
  | "reclaim"
  | "release"
  | "offboard"
  | "dead";

export type TimelineEvent = {
  ts: string;
  handle: string;
  kind: TimelineEventKind | string;
  description: string;
  lane_id?: string | null;
};

type TimelinePaneProps = {
  /** True when the current session has role === "coordinator" — pane is hidden otherwise. */
  isCoordinator: boolean;
  /** If non-null, scope the timeline to this colab_group (spawn events only; claims/offboard/dead come from the shared mailbox). */
  colabGroup?: string | null;
  /** Injected in tests; defaults to the Tauri invoke bridge. */
  fetcher?: (args: TimelineFetchArgs) => Promise<TimelineEvent[]>;
  /** Poll interval while visible. 30s per briefing. */
  pollMs?: number;
  /** Initial window in days. 7 per design. */
  windowDays?: number;
};

export type TimelineFetchArgs = {
  colabGroup?: string | null;
  sinceTs?: string | null;
  beforeTs?: string | null;
  limit?: number | null;
};

const tauriFetcher = (args: TimelineFetchArgs): Promise<TimelineEvent[]> =>
  invoke<TimelineEvent[]>("list_timeline_events", {
    args: {
      colabGroup: args.colabGroup ?? null,
      sinceTs: args.sinceTs ?? null,
      beforeTs: args.beforeTs ?? null,
      limit: args.limit ?? null,
    },
  });

/** Format timestamp for row label. Short local date + HH:MM. */
export function formatTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const today = new Date();
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate();
  const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (sameDay) return hm;
  const md = d.toLocaleDateString([], { month: "short", day: "numeric" });
  return `${md} ${hm}`;
}

/** Label for a kind chip. Short so the chip stays compact in dense rows. */
export function chipLabel(kind: string): string {
  switch (kind) {
    case "spawn":
      return "spawn";
    case "claim":
      return "claim";
    case "reclaim":
      return "reclaim";
    case "release":
      return "release";
    case "offboard":
      return "offboard";
    case "dead":
      return "dead";
    default:
      return kind;
  }
}

/** Sort events newest-first; handle-tiebreak for stable ordering. */
export function sortNewestFirst(events: TimelineEvent[]): TimelineEvent[] {
  return [...events].sort((a, b) => {
    if (a.ts === b.ts) return a.handle.localeCompare(b.handle);
    return b.ts.localeCompare(a.ts);
  });
}

/** Apply a case-insensitive substring filter on handle. */
export function filterByHandle(
  events: TimelineEvent[],
  needle: string,
): TimelineEvent[] {
  const n = needle.trim().toLowerCase();
  if (!n) return events;
  return events.filter((e) => e.handle.toLowerCase().includes(n));
}

/** Merge a newer page into an existing list, dedup'd by (ts, handle, kind, description).
 *  The backend sort is authoritative, but the UI may hold older pages that
 *  should stay visible across refreshes. */
export function mergeEvents(
  prior: TimelineEvent[],
  incoming: TimelineEvent[],
): TimelineEvent[] {
  const seen = new Set<string>();
  const out: TimelineEvent[] = [];
  for (const e of [...incoming, ...prior]) {
    const key = `${e.ts}|${e.handle}|${e.kind}|${e.description}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(e);
  }
  return sortNewestFirst(out);
}

/** Coordinator spawn/teardown timeline — collapsible panel that sits below
 *  the fleet pane. Polls `list_timeline_events` every 30s while expanded.
 *  Hidden entirely for non-coordinators so single-session users and regular
 *  workers see no new chrome. */
export default function TimelinePane({
  isCoordinator,
  colabGroup = null,
  fetcher = tauriFetcher,
  pollMs = 30_000,
  windowDays = 7,
}: TimelinePaneProps) {
  const [events, setEvents] = useState<TimelineEvent[]>([]);
  const [expanded, setExpanded] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [reachedEnd, setReachedEnd] = useState(false);
  const [handleFilter, setHandleFilter] = useState("");

  // Oldest ts currently loaded — used as `beforeTs` for the next page.
  const oldestTsRef = useRef<string | null>(null);
  // Monotonic request id so stale responses never overwrite fresh state.
  const fetchSeqRef = useRef(0);

  const defaultSinceTs = useCallback(() => {
    const since = new Date(Date.now() - windowDays * 24 * 60 * 60 * 1000);
    return since.toISOString();
  }, [windowDays]);

  const doRefresh = useCallback(async () => {
    const seq = ++fetchSeqRef.current;
    try {
      const result = await fetcher({
        colabGroup,
        sinceTs: defaultSinceTs(),
        beforeTs: null,
        limit: null,
      });
      if (seq !== fetchSeqRef.current) return;
      const sorted = sortNewestFirst(result || []);
      setEvents(sorted);
      oldestTsRef.current = sorted.length > 0 ? sorted[sorted.length - 1].ts : null;
      setReachedEnd(false);
      setError(null);
      setLoaded(true);
    } catch (e) {
      if (seq !== fetchSeqRef.current) return;
      setError(String(e));
      setLoaded(true);
    }
  }, [fetcher, colabGroup, defaultSinceTs]);

  const doLoadMore = useCallback(async () => {
    if (loadingMore || reachedEnd) return;
    const beforeTs = oldestTsRef.current;
    if (!beforeTs) return;
    setLoadingMore(true);
    const seq = ++fetchSeqRef.current;
    try {
      // Extend the window back by the same number of days each click.
      const nextSince = new Date(
        new Date(beforeTs).getTime() - windowDays * 24 * 60 * 60 * 1000,
      ).toISOString();
      const result = await fetcher({
        colabGroup,
        sinceTs: nextSince,
        beforeTs,
        limit: null,
      });
      if (seq !== fetchSeqRef.current) return;
      if (!result || result.length === 0) {
        setReachedEnd(true);
      } else {
        setEvents((prev) => {
          const merged = mergeEvents(prev, result);
          const last = merged[merged.length - 1];
          oldestTsRef.current = last?.ts ?? oldestTsRef.current;
          return merged;
        });
      }
      setError(null);
    } catch (e) {
      if (seq !== fetchSeqRef.current) return;
      setError(String(e));
    } finally {
      setLoadingMore(false);
    }
  }, [fetcher, colabGroup, loadingMore, reachedEnd, windowDays]);

  useEffect(() => {
    if (!isCoordinator) return;
    doRefresh();
    const h = window.setInterval(doRefresh, pollMs);
    return () => window.clearInterval(h);
  }, [isCoordinator, doRefresh, pollMs]);

  const visible = useMemo(
    () => filterByHandle(events, handleFilter),
    [events, handleFilter],
  );

  if (!isCoordinator) return null;

  return (
    <div className="timeline-panel">
      <div
        className="timeline-header"
        onClick={() => setExpanded((v) => !v)}
        role="button"
        aria-expanded={expanded}
      >
        <h2>
          <span className={`prompt-chevron ${expanded ? "expanded" : ""}`}>
            &#9654;
          </span>
          <span className="timeline-title">Timeline</span>
          {loaded && (
            <span className="timeline-summary">
              {visible.length}
              {handleFilter && visible.length !== events.length
                ? ` / ${events.length}`
                : ""}{" "}
              event{visible.length === 1 ? "" : "s"}
            </span>
          )}
        </h2>
        <button
          className="section-refresh-btn"
          onClick={(e) => {
            e.stopPropagation();
            doRefresh();
          }}
          title="Refresh timeline"
        >
          &#8635;
        </button>
      </div>

      {expanded && (
        <div className="timeline-body">
          <input
            type="text"
            className="timeline-filter"
            placeholder="Filter by handle…"
            value={handleFilter}
            onChange={(e) => setHandleFilter(e.target.value)}
            aria-label="Filter timeline by handle"
          />

          {error && <div className="timeline-error">{error}</div>}

          {!error && loaded && visible.length === 0 && (
            <div className="timeline-empty">
              {handleFilter ? "No matching events." : "No events yet."}
            </div>
          )}

          {!error && visible.length > 0 && (
            <ul className="timeline-list" role="list">
              {visible.map((e, idx) => (
                <li
                  key={`${e.ts}-${e.handle}-${e.kind}-${idx}`}
                  className={`timeline-row timeline-row-${e.kind}`}
                  title={`${e.ts} — ${e.handle}`}
                >
                  <span
                    className="timeline-ts"
                    title={e.ts}
                  >
                    {formatTs(e.ts)}
                  </span>
                  <span className={`timeline-chip timeline-chip-${e.kind}`}>
                    {chipLabel(e.kind)}
                  </span>
                  <span className="timeline-handle">{e.handle}</span>
                  <span className="timeline-desc">{e.description}</span>
                </li>
              ))}
            </ul>
          )}

          {!error && visible.length > 0 && (
            <div className="timeline-footer">
              <button
                className="timeline-load-more"
                onClick={doLoadMore}
                disabled={loadingMore || reachedEnd}
              >
                {reachedEnd
                  ? "No more events"
                  : loadingMore
                  ? "Loading…"
                  : "Load more"}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
