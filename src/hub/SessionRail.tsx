import { useState } from "react";
import { getDarkModeAccentColor } from "../color";
import { STATE_LABELS, headlineOf, sinceLabel, type SessionView } from "./api";

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

interface Props {
  sessions: SessionView[];
  selected: string | null;
  overviewActive: boolean;
  isDark: boolean;
  now: number;
  /** `rail`: a full column. `switcher`: the compact list atop a sidebar. */
  variant: "rail" | "switcher";
  onSelect: (key: string) => void;
  onOverview: () => void;
  onReorder: (keys: string[]) => void;
  onNew: () => void;
  onPalette: () => void;
  onCollapse: () => void;
  onLayout: () => void;
  /** The side of the window this list sits on, for the collapse arrow. */
  side: "left" | "right";
  /** Render only the header (the sidebar's toolbar) or only the list. */
  part?: "all" | "header" | "list";
}

export default function SessionRail({
  sessions,
  selected,
  overviewActive,
  isDark,
  now,
  variant,
  onSelect,
  onOverview,
  onReorder,
  onNew,
  onPalette,
  onCollapse,
  onLayout,
  side,
  part = "all",
}: Props) {
  const [dragKey, setDragKey] = useState<string | null>(null);
  const [dropKey, setDropKey] = useState<string | null>(null);
  const needing = sessions.filter((s) => s.attention);
  const compact = variant === "switcher";

  const drop = (targetKey: string) => {
    if (!dragKey || dragKey === targetKey) return;
    const keys = sessions.map((s) => s.key).filter((k) => k !== dragKey);
    keys.splice(keys.indexOf(targetKey), 0, dragKey);
    onReorder(keys);
  };

  return (
    <nav className={`rail rail-${variant} rail-part-${part}`}>
      {part !== "list" && (
      <div className="rail-header">
        <button
          className={`rail-home${overviewActive ? " active" : ""}`}
          onClick={onOverview}
          title="Overview (⌘0)"
        >
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
        {sessions.map((s, index) => {
          const accent = s.color ? (isDark ? getDarkModeAccentColor(s.color) : s.color) : undefined;
          const suspended = s.status.state === "suspended";
          const headline = headlineOf(s);
          return (
            <div
              key={s.key}
              className={`rail-row${s.key === selected && !overviewActive ? " selected" : ""}${suspended ? " suspended" : ""}${s.attention ? " attention" : ""}${dropKey === s.key ? " drop-target" : ""}`}
              onClick={() => onSelect(s.key)}
              draggable
              onDragStart={() => setDragKey(s.key)}
              onDragOver={(e) => {
                e.preventDefault();
                setDropKey(s.key);
              }}
              onDragLeave={() => setDropKey((k) => (k === s.key ? null : k))}
              onDrop={(e) => {
                e.preventDefault();
                drop(s.key);
                setDragKey(null);
                setDropKey(null);
              }}
              onDragEnd={() => {
                setDragKey(null);
                setDropKey(null);
              }}
              title={compact && headline ? headline : undefined}
            >
              <span className="rail-swatch" style={{ background: accent }} />
              <div className="rail-row-body">
                <div className="rail-row-top">
                  <StateDot session={s} />
                  <span className="rail-name">{s.name}</span>
                  {!suspended && <span className="rail-since">{sinceLabel(s.status.since, now)}</span>}
                  {index < 9 && <span className="rail-shortcut">⌘{index + 1}</span>}
                </div>
                {!compact && (
                  <div className="rail-row-meta">
                    {s.ticket_key && <span className="chip chip-mono">{s.ticket_key}</span>}
                    <span className="rail-state">{STATE_LABELS[s.status.state]}</span>
                  </div>
                )}
                {headline && <div className={`rail-headline${compact ? " one-line" : ""}`}>{headline}</div>}
              </div>
            </div>
          );
        })}
      </div>
      )}
    </nav>
  );
}
