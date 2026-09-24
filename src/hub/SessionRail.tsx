import { askSummary } from "./AskList";
import { useEffect, useState } from "react";
import { getDarkModeAccentColor } from "../color";
import { LANES, STATE_LABELS, effortsOf, headlineOf, sinceLabel, type Lane, type SessionView } from "./api";

export function StateDot({ session }: { session: SessionView }) {
  const state = session.status.state;
  const cls = session.attention && state === "your_turn" ? "your_turn unseen" : state;
  return (
    <span
      className={`state-dot state-${cls}`}
      title={`${STATE_LABELS[state]}${session.status.detail ? `: ${session.status.detail}` : ""}`}
    />
  );
}

/** "blocked 3d · checked 2h" for a blocked session. */
export function blockedLabel(s: SessionView, now: number): string | null {
  if (s.lane !== "blocked" || !s.blocked_since) return null;
  const blocked = `blocked ${sinceLabel(s.blocked_since, now)}`;
  if (!s.checked_at || s.checked_at === s.blocked_since) return blocked;
  return `${blocked} · checked ${sinceLabel(s.checked_at, now)} ago`;
}

interface Props {
  sessions: SessionView[];
  selected: string | null;
  overviewActive: boolean;
  isDark: boolean;
  now: number;
  /** `rail`: a full column. `switcher`: the compact list in a sidebar. */
  variant: "rail" | "switcher";
  collapsedLanes: Lane[];
  onToggleLane: (lane: Lane) => void;
  onSelect: (key: string) => void;
  onOverview: () => void;
  /** New order for every session, and the lane the moved one landed in. */
  onMove: (keys: string[], moved: string, lane: Lane) => void;
  onSetLane: (key: string, lane: Lane) => void;
  onNew: () => void;
  onPalette: () => void;
  onCollapse: () => void;
  onLayout: () => void;
  /** The side of the window this list sits on, for the collapse arrow. */
  side: "left" | "right";
  /** Render only the header (the sidebar's toolbar) or only the list. */
  part?: "all" | "header" | "list";
}

/** Sessions' efforts, computed once per render of the list. */
let effortCache: { sessions: SessionView[]; efforts: Map<string, string> } | null = null;
function effortMap(sessions: SessionView[]) {
  if (effortCache?.sessions !== sessions) effortCache = { sessions, efforts: effortsOf(sessions) };
  return effortCache.efforts;
}

export default function SessionRail({
  sessions,
  selected,
  overviewActive,
  isDark,
  now,
  variant,
  collapsedLanes,
  onToggleLane,
  onSelect,
  onOverview,
  onMove,
  onSetLane,
  onNew,
  onPalette,
  onCollapse,
  onLayout,
  side,
  part = "all",
}: Props) {
  const [dragKey, setDragKey] = useState<string | null>(null);
  const [drop, setDrop] = useState<{ key: string | null; lane: Lane } | null>(null);
  const [menu, setMenu] = useState<{ key: string; x: number; y: number } | null>(null);
  const needing = sessions.filter((s) => s.attention);
  const compact = variant === "switcher";

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("blur", close);
    };
  }, [menu]);

  const lanes = LANES.map(({ lane, label }) => ({
    lane,
    label,
    sessions: sessions.filter((s) => (s.lane ?? "background") === lane),
  }));
  const shortcutIndex = new Map<string, number>();
  let n = 0;
  for (const l of lanes) {
    if (collapsedLanes.includes(l.lane)) continue;
    for (const s of l.sessions) shortcutIndex.set(s.key, n++);
  }

  const finishDrop = (targetKey: string | null, lane: Lane) => {
    if (!dragKey) return;
    const ordered = lanes.flatMap((l) => l.sessions.map((s) => s.key)).filter((k) => k !== dragKey);
    let at: number;
    if (targetKey && targetKey !== dragKey) {
      at = ordered.indexOf(targetKey);
    } else {
      // Dropped on a lane's header or its empty space: the end of that lane.
      const laneKeys = lanes.find((l) => l.lane === lane)!.sessions.map((s) => s.key).filter((k) => k !== dragKey);
      const last = laneKeys[laneKeys.length - 1];
      const before = LANES.findIndex((l) => l.lane === lane);
      at = last
        ? ordered.indexOf(last) + 1
        : ordered.filter((k) => {
            const s = sessions.find((x) => x.key === k)!;
            return LANES.findIndex((l) => l.lane === (s.lane ?? "background")) < before;
          }).length;
    }
    ordered.splice(Math.max(0, at), 0, dragKey);
    onMove(ordered, dragKey, lane);
  };

  const row = (s: SessionView) => {
    const accent = s.color ? (isDark ? getDarkModeAccentColor(s.color) : s.color) : undefined;
    const suspended = s.status.state === "suspended";
    const headline = headlineOf(s);
    const blocked = blockedLabel(s, now);
    const shortcut = shortcutIndex.get(s.key);
    return (
      <div
        key={s.key}
        className={`rail-row${s.key === selected && !overviewActive ? " selected" : ""}${suspended ? " suspended" : ""}${s.attention ? " attention" : ""}${s.lane === "blocked" ? " blocked" : ""}${drop?.key === s.key ? " drop-target" : ""}`}
        onClick={() => onSelect(s.key)}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ key: s.key, x: e.clientX, y: e.clientY });
        }}
        draggable
        onDragStart={() => setDragKey(s.key)}
        onDragOver={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setDrop({ key: s.key, lane: s.lane ?? "background" });
        }}
        onDrop={(e) => {
          e.preventDefault();
          e.stopPropagation();
          finishDrop(s.key, s.lane ?? "background");
          setDragKey(null);
          setDrop(null);
        }}
        onDragEnd={() => {
          setDragKey(null);
          setDrop(null);
        }}
        title={compact && headline ? headline : undefined}
      >
        <span className="rail-swatch" style={{ background: accent }} />
        <div className="rail-row-body">
          <div className="rail-row-top">
            <StateDot session={s} />
            <span className="rail-name">{s.name}</span>
            {!suspended && !blocked && <span className="rail-since">{sinceLabel(s.status.since, now)}</span>}
            {shortcut !== undefined && shortcut < 9 && <span className="rail-shortcut">⌘{shortcut + 1}</span>}
          </div>
          {blocked && <div className="rail-blocked">{blocked}</div>}
          {(s.asks?.length ?? 0) > 0 && (
            <div className={`rail-waiting rail-asks${s.asks!.some((a) => a.kind === "decision") ? " decision" : ""}`}>{askSummary(s.asks!)}</div>
          )}
          {(s.blockers?.length ?? 0) > 0 && (
            <div className={`rail-waiting${s.blockers!.some((b) => b.status === "updated") ? " updated" : ""}`}>
              {s.blockers!.some((b) => b.status === "updated")
                ? `Update on: ${s.blockers!.find((b) => b.status === "updated")!.title}`
                : `Waiting on ${s.blockers!.length === 1 ? s.blockers![0].party || s.blockers![0].title : `${s.blockers!.length} things`}`}
            </div>
          )}
          {!compact && !blocked && (
            <div className="rail-row-meta">
              {s.ticket_key && <span className="chip chip-mono">{s.ticket_key}</span>}
              <span className="rail-state">{STATE_LABELS[s.status.state]}</span>
              {effortMap(sessions).get(s.key) && <span className="rail-effort">{effortMap(sessions).get(s.key)}</span>}
            </div>
          )}
          {headline && <div className={`rail-headline${compact ? " one-line" : ""}`}>{headline}</div>}
        </div>
      </div>
    );
  };

  return (
    <nav className={`rail rail-${variant} rail-part-${part}`}>
      {part !== "list" && (
        <div className="rail-header">
          <button className={`rail-home${overviewActive ? " active" : ""}`} onClick={onOverview} title="Overview (⌘0)">
            <span className="rail-home-title">Sessions</span>
            <span className="rail-home-count">{sessions.length}</span>
            {needing.length > 0 && (
              <span className="rail-attention-count" title="Need you">
                {needing.length}
              </span>
            )}
          </button>
          <div className="rail-header-actions">
            <button className="icon-button" onClick={onPalette} title="Switch or open (⌘K)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                <circle cx="7" cy="7" r="4.5" />
                <path d="M10.5 10.5L14 14" />
              </svg>
            </button>
            <button className="icon-button" onClick={onNew} title="New session (⌘N)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                <path d="M8 3v10M3 8h10" />
              </svg>
            </button>
            <button className="icon-button" onClick={onLayout} title="Layout">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3">
                <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
                <path d="M6 2.5v11" />
              </svg>
            </button>
            <button className="icon-button" onClick={onCollapse} title="Collapse to a thin bar (⌘\)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
                {side === "left" ? <path d="M9.5 4.5L6 8l3.5 3.5" /> : <path d="M6.5 4.5L10 8l-3.5 3.5" />}
              </svg>
            </button>
          </div>
        </div>
      )}

      {part !== "header" && (
        <div className="rail-list">
          {sessions.length === 0 && (
            <div className="rail-empty">
              No sessions open.
              <span>⌘N starts one, ⌘K opens an existing one.</span>
            </div>
          )}
          {sessions.length > 0 &&
            lanes.map((l) => {
              const collapsed = collapsedLanes.includes(l.lane);
              const needs = l.sessions.filter((s) => s.attention).length;
              return (
                <div
                  key={l.lane}
                  className={`lane lane-${l.lane}${drop && drop.key === null && drop.lane === l.lane ? " drop-target" : ""}`}
                  onDragOver={(e) => {
                    e.preventDefault();
                    setDrop({ key: null, lane: l.lane });
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    finishDrop(null, l.lane);
                    setDragKey(null);
                    setDrop(null);
                  }}
                >
                  <button className="lane-head" onClick={() => onToggleLane(l.lane)}>
                    <svg className={`section-chevron${collapsed ? "" : " open"}`} width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M3.5 2l3 3-3 3" />
                    </svg>
                    <span className="lane-title">{l.label}</span>
                    <span className="count">{l.sessions.length}</span>
                    {collapsed && needs > 0 && <span className="rail-attention-count">{needs}</span>}
                  </button>
                  {!collapsed && l.sessions.map(row)}
                  {!collapsed && l.sessions.length === 0 && <div className="lane-empty">Drag a session here</div>}
                </div>
              );
            })}
        </div>
      )}

      {menu && (
        <div className="context-menu" style={{ left: menu.x, top: menu.y }} onClick={(e) => e.stopPropagation()}>
          <div className="menu-label">Move to</div>
          {LANES.map(({ lane, label }) => {
            const current = sessions.find((s) => s.key === menu.key)?.lane === lane;
            return (
              <button
                key={lane}
                className={`menu-item${current ? " checked" : ""}`}
                onClick={() => {
                  onSetLane(menu.key, lane);
                  setMenu(null);
                }}
              >
                <span className={`lane-dot lane-dot-${lane}`} />
                {label}
              </button>
            );
          })}
        </div>
      )}
    </nav>
  );
}
