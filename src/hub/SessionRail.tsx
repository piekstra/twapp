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
  onSelect: (key: string) => void;
  onOverview: () => void;
  onReorder: (keys: string[]) => void;
  onNew: () => void;
  onPalette: () => void;
}

export default function SessionRail({
  sessions,
  selected,
  overviewActive,
  isDark,
  now,
  onSelect,
  onOverview,
  onReorder,
  onNew,
  onPalette,
}: Props) {
  const [dragKey, setDragKey] = useState<string | null>(null);
  const [dropKey, setDropKey] = useState<string | null>(null);
  const needing = sessions.filter((s) => s.attention);

  const drop = (targetKey: string) => {
    if (!dragKey || dragKey === targetKey) return;
    const keys = sessions.map((s) => s.key).filter((k) => k !== dragKey);
    const idx = keys.indexOf(targetKey);
    keys.splice(idx, 0, dragKey);
    onReorder(keys);
  };

  return (
    <nav className="rail">
      <div className="rail-header">
        <button
          className={`rail-overview${overviewActive ? " active" : ""}`}
          onClick={onOverview}
          title="Overview (⌘0)"
        >
          <span className="rail-overview-title">twapp</span>
          {needing.length > 0 && <span className="rail-attention-count">{needing.length}</span>}
        </button>
        <div className="rail-header-actions">
          <button className="rail-icon-btn" onClick={onPalette} title="Switch or open (⌘K)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <circle cx="7" cy="7" r="4.5" />
              <path d="M10.5 10.5L14 14" />
            </svg>
          </button>
          <button className="rail-icon-btn" onClick={onNew} title="New session (⌘N)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <path d="M8 2v12M2 8h12" />
            </svg>
          </button>
        </div>
      </div>

      {needing.length > 0 && (
        <div className="rail-needs">
          <div className="rail-section-label">Needs you</div>
          {needing.map((s) => (
            <button key={s.key} className="rail-needs-chip" onClick={() => onSelect(s.key)} title={headlineOf(s)}>
              <StateDot session={s} />
              <span className="rail-needs-name">{s.name}</span>
              <span className="rail-since">{sinceLabel(s.status.since, now)}</span>
            </button>
          ))}
        </div>
      )}

      <div className="rail-list">
        {sessions.length === 0 && (
          <div className="rail-empty">
            No sessions open.
            <br />
            <span>⌘N for a new one, ⌘K to open an existing one.</span>
          </div>
        )}
        {sessions.map((s, index) => {
          const accent = s.color ? (isDark ? getDarkModeAccentColor(s.color) : s.color) : undefined;
          const suspended = s.status.state === "suspended";
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
            >
              <span className="rail-accent" style={{ background: accent }} />
              <div className="rail-row-body">
                <div className="rail-row-top">
                  <StateDot session={s} />
                  <span className="rail-name">{s.name}</span>
                  {index < 9 && <span className="rail-shortcut">⌘{index + 1}</span>}
                </div>
                <div className="rail-row-meta">
                  {s.ticket_key && <span className="rail-ticket">{s.ticket_key}</span>}
                  <span className="rail-state">{STATE_LABELS[s.status.state]}</span>
                  {!suspended && <span className="rail-since">{sinceLabel(s.status.since, now)}</span>}
                </div>
                {headlineOf(s) && <div className="rail-headline">{headlineOf(s)}</div>}
              </div>
            </div>
          );
        })}
      </div>
    </nav>
  );
}
