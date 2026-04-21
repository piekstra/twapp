import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type FleetAgent = {
  handle: string;
  status: string;
  role?: string | null;
  provenance?: string | null;
  colab_group?: string | null;
  current_task?: string | null;
  last_heartbeat: string;
  last_heartbeat_age_sec?: number | null;
  poll_interval_sec: number;
  dormant: boolean;
  unread_count: number;
  urgent_count: number;
  directory?: string | null;
};

type FleetPaneProps = {
  /** True when the current session has role === "coordinator" — pane is hidden otherwise. */
  isCoordinator: boolean;
  /** If non-null, scope the fleet to this colab_group. */
  colabGroup?: string | null;
  /** Injected in tests; defaults to the Tauri invoke bridge. */
  fetcher?: (args: { colabGroup?: string | null }) => Promise<FleetAgent[]>;
  /** Called when the user clicks a row. Defaults to the Tauri launch_session bridge. */
  opener?: (agent: FleetAgent) => Promise<void>;
  /** Poll interval while visible. 5s per briefing. */
  pollMs?: number;
};

const tauriFetcher = (args: { colabGroup?: string | null }): Promise<FleetAgent[]> =>
  invoke<FleetAgent[]>("list_fleet", { args: { colabGroup: args.colabGroup ?? null } });

const tauriOpener = async (agent: FleetAgent): Promise<void> => {
  if (!agent.directory) return;
  await invoke("launch_session", { sessionId: "", directory: agent.directory });
};

/** Format heartbeat age. `null` / unknown → "—". Caps at days. */
export function formatAge(ageSec: number | null | undefined): string {
  if (ageSec === null || ageSec === undefined || Number.isNaN(ageSec)) return "—";
  const s = Math.max(0, Math.floor(ageSec));
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

/** Summary counts used by the collapsed header. */
export function summarize(agents: FleetAgent[]): {
  total: number;
  active: number;
  dormant: number;
  urgent: number;
} {
  const total = agents.length;
  let dormant = 0;
  let urgent = 0;
  for (const a of agents) {
    if (a.dormant) dormant += 1;
    if (a.urgent_count > 0) urgent += 1;
  }
  return { total, active: total - dormant, dormant, urgent };
}

/** Pick a status-dot CSS class from the agent's derived status. */
export function statusDotClass(a: FleetAgent): string {
  if (a.dormant) return "fleet-dot fleet-dot-dormant";
  switch (a.status) {
    case "processing":
      return "fleet-dot fleet-dot-processing";
    case "idle":
      return "fleet-dot fleet-dot-idle";
    default:
      return "fleet-dot fleet-dot-dormant";
  }
}

/** Truncate a current_task line for dense rows. Never hard-breaks a token mid-run. */
export function truncateTask(task: string | null | undefined, max = 80): string {
  if (!task) return "";
  const trimmed = task.trim();
  if (trimmed.length <= max) return trimmed;
  return trimmed.slice(0, max - 1) + "…";
}

/** Coordinator fleet pane — collapsible sidebar section that lists every
 *  handle with a live `presence/<handle>.json`. Polls `list_fleet` every 5s
 *  while expanded; hidden entirely for non-coordinators so single-session
 *  users and regular workers see no new chrome. */
export default function FleetPane({
  isCoordinator,
  colabGroup = null,
  fetcher = tauriFetcher,
  opener = tauriOpener,
  pollMs = 5_000,
}: FleetPaneProps) {
  const [agents, setAgents] = useState<FleetAgent[]>([]);
  const [expanded, setExpanded] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const fetchSeqRef = useRef(0);

  const doFetch = useCallback(async () => {
    const seq = ++fetchSeqRef.current;
    try {
      const result = await fetcher({ colabGroup });
      if (seq !== fetchSeqRef.current) return;
      setAgents(result || []);
      setError(null);
      setLoaded(true);
    } catch (e) {
      if (seq !== fetchSeqRef.current) return;
      setError(String(e));
      setLoaded(true);
    }
  }, [fetcher, colabGroup]);

  useEffect(() => {
    if (!isCoordinator) return;
    doFetch();
    const h = window.setInterval(doFetch, pollMs);
    return () => window.clearInterval(h);
  }, [isCoordinator, doFetch, pollMs]);

  const summary = useMemo(() => summarize(agents), [agents]);

  if (!isCoordinator) return null;

  const handleRowClick = async (a: FleetAgent) => {
    if (!a.directory) return;
    try {
      await opener(a);
    } catch (e) {
      setError(`Failed to open ${a.handle}: ${e}`);
    }
  };

  return (
    <div className="fleet-panel">
      <div
        className="fleet-header"
        onClick={() => setExpanded((v) => !v)}
        role="button"
        aria-expanded={expanded}
      >
        <h2>
          <span className={`prompt-chevron ${expanded ? "expanded" : ""}`}>&#9654;</span>
          <span className="fleet-title">Fleet</span>
          {loaded && (
            <span className="fleet-summary">
              <span className="fleet-summary-active" title="processing + idle">
                {summary.active} active
              </span>
              {summary.dormant > 0 && (
                <span className="fleet-summary-dormant" title="dormant">
                  · {summary.dormant} dormant
                </span>
              )}
              {summary.urgent > 0 && (
                <span className="fleet-summary-urgent" title="agents with urgent mail">
                  · {summary.urgent} urgent
                </span>
              )}
            </span>
          )}
        </h2>
        <button
          className="section-refresh-btn"
          onClick={(e) => {
            e.stopPropagation();
            doFetch();
          }}
          title="Refresh fleet"
        >
          &#8635;
        </button>
      </div>

      {expanded && (
        <div className="fleet-body">
          {error && <div className="fleet-error">{error}</div>}
          {!error && loaded && agents.length === 0 && (
            <div className="fleet-empty">No workers online.</div>
          )}
          {!error && agents.length > 0 && (
            <ul className="fleet-list" role="list">
              {agents.map((a) => (
                <li
                  key={a.handle}
                  className={`fleet-row${a.dormant ? " fleet-row-dormant" : ""}${
                    a.urgent_count > 0 ? " fleet-row-urgent" : ""
                  }${a.directory ? " fleet-row-clickable" : ""}`}
                  onClick={() => handleRowClick(a)}
                  role={a.directory ? "button" : undefined}
                  title={
                    a.directory
                      ? `Open ${a.handle}'s window`
                      : `${a.handle} — no on-disk session yet`
                  }
                >
                  <div className="fleet-row-top">
                    <span className={statusDotClass(a)} aria-hidden="true" />
                    <span className="fleet-handle">{a.handle}</span>
                    {a.role && (
                      <span className="fleet-role-chip" title={`role: ${a.role}`}>
                        {a.role}
                      </span>
                    )}
                    {a.provenance === "spawned" && (
                      <span
                        className="fleet-provenance"
                        title="spawned by another session"
                        aria-label="spawned"
                      >
                        &#9656;
                      </span>
                    )}
                    {a.provenance === "user" && (
                      <span
                        className="fleet-provenance"
                        title="user-created"
                        aria-label="user-created"
                      >
                        &#9702;
                      </span>
                    )}
                    <span className="fleet-counts">
                      {a.urgent_count > 0 && (
                        <span
                          className="priority-chip priority-urgent fleet-count-chip"
                          title={`${a.urgent_count} urgent`}
                        >
                          {a.urgent_count}!
                        </span>
                      )}
                      {a.unread_count > 0 && (
                        <span className="fleet-count-chip fleet-unread" title={`${a.unread_count} unread`}>
                          {a.unread_count}
                        </span>
                      )}
                    </span>
                    <span className="fleet-age" title={a.last_heartbeat}>
                      {formatAge(a.last_heartbeat_age_sec)}
                    </span>
                  </div>
                  {a.current_task && (
                    <div className="fleet-task">{truncateTask(a.current_task)}</div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
