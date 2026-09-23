import { Fragment } from "react";
import { STATE_LABELS, headlineOf, type SessionView } from "./api";

interface Props {
  sessions: SessionView[];
  selected: string | null;
  side: "left" | "right";
  onSelect: (key: string) => void;
  onExpand: () => void;
  /** Hovering the bar shows the full sidebar over the terminal. */
  onPeek: (peeking: boolean) => void;
}

/**
 * A collapsed sidebar: a strip a few pixels wide with one tick per session,
 * colored by what the session needs. Ticks select; the strip's end expands.
 */
export default function ThinBar({ sessions, selected, side, onSelect, onExpand, onPeek }: Props) {
  return (
    <div
      className={`thinbar thinbar-${side}`}
      onMouseEnter={() => onPeek(true)}
      onMouseLeave={() => onPeek(false)}
    >
      <div className="thinbar-ticks">
        {sessions.map((s, i) => {
          const tone = s.attention
            ? s.status.state === "needs_approval"
              ? "approval"
              : "turn"
            : s.status.state === "working" || s.status.state === "starting"
              ? "working"
              : s.status.state === "errored"
                ? "error"
                : "quiet";
          // A gap marks where one lane ends and the next begins.
          const laneBreak = i > 0 && (sessions[i - 1].lane ?? "background") !== (s.lane ?? "background");
          return (
            <Fragment key={s.key}>
            {laneBreak && <div className="thinbar-gap" />}
            <button
              className={`thinbar-tick tone-${tone} lane-${s.lane ?? "background"}${s.key === selected ? " selected" : ""}`}
              title={`${s.name}: ${STATE_LABELS[s.status.state]}${headlineOf(s) ? `\n${headlineOf(s)}` : ""}`}
              onClick={() => onSelect(s.key)}
            />
            </Fragment>
          );
        })}
      </div>
      <button className="thinbar-expand" onClick={onExpand} title="Expand (⌘\)">
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
          {side === "right" ? <path d="M6.5 2L3.5 5l3 3" /> : <path d="M3.5 2l3 3-3 3" />}
        </svg>
      </button>
    </div>
  );
}
